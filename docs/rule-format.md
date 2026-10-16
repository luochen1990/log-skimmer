# Log Skimmer 规则格式与状态机规范 (v0)

> 职责边界: 本文是规则 YAML 格式、逐行状态机语义、fragment 输出协议与 L0 统计口径的**单一事实来源**, 是规则编写者与引擎实现者的共同契约。两者对语义的任何疑问以本文为准; 本文未定义的行为视为未指定 (实现不应依赖)。
> 版本: v0 (POC)。字段只增不删地向前演进; 语义变更须同步 bump 版本并迁移规则。

## 0. 设计灵魂 (约束一切后续演进)

- **LLM 只在学习期 (Adapter), 读取期 (Reader/引擎) 纯确定性**: 本规范描述的运行时零 LLM、零网络、可复现、可审计。
- **流式友好**: 状态机逐行消费, 引擎实现不得要求全文件载入 (POC 实现可以整体读入方便统计, 但状态机语义本身必须逐行可推导)。
- **线性复杂度结构保证**: 正则限定 RE2 语义 (见 §2.A8), 每行 × 每规则的匹配是 O(行长)。

## 1. 规则文件格式

- 路径: `rules/<tool-id>/<rule-id>.yaml`, 一文件一规则。
- `tool-id`: 小写字母/数字/连字符, folksonomy 约定名 (无中央注册表, 规则的存在即定义 ID)。
- `rule-id`: `<tool-id>-<语义名>`, 全局唯一 (同 id 重复加载为错误)。

```yaml
# ---- 必填 ----
id: nix-eval-error            # <tool-id>-<语义名>, 全局唯一
title: nix 求值错误树          # 一句话人类可读描述
tool: nix                     # 工具维度 (与目录名一致)
scope: global                 # global | repo
patterns:                     # 状态机 pattern 链, ≥1 项 (见 §2)
  - regex: '^error: '          # patterns[0] 是锚 (anchor)
  - regex: '^\s'              # 后续缩进行
    loop: true                 # 贪心重复消费
kind: signal                  # signal (提取) | noise (标记噪音)
# ---- 选填 (默认值如下) ----
repo: null                    # scope: repo 时必填, "owner/name"
category: null                # fmt|lint|typecheck|build|test|infra|other (片段继承)
confidence: medium            # high|medium|low (片段继承, 排序参考)
context: { before: 0, after: 0 }   # 输出边界扩展 (行数, 不参与状态机)
structured-hint: null         # 未来字段, POC 不实现, 保留占位
attribution: null             # POC 手写规则省略; Adapter 生成时必填
#   attribution: { contributor: str, generated-by: {harness: str, model: str}, adapter-version: str }
```

字段约束 (加载期校验, 违例 = 规则集非法, 整体拒绝加载):

| 字段 | 约束 |
| --- | --- |
| id | 匹配 `^[a-z0-9-]+$`, 且以 `tool` 值为前缀 |
| scope/repo | `scope: repo` 时 repo 必填; `scope: global` 时 repo 必须为 null |
| patterns | 非空数组; 每项 regex 必填; loop 默认 false |
| context.before/after | 0 ≤ 值 ≤ 10 |
| kind/confidence/category | 枚举值; category 允许 null |

## 2. 状态机语义 (公理 A1-A8)

**A0 行消费语义**: 引擎消费的「行」= 原始行剥去尾部 `\r` (CRLF 归一) 后的字符串; 行号 1-based。宿主前缀 (CI 平台时间戳、ANSI 色码) **不做引擎预处理** — 由规则正则自带容忍 (如锚写 `^(timestamp )?error: `; 实测怪形态见 docs/poc-report.md §4)。

**A1 独立扫描**: 每条规则维护独立状态机, 对日志逐行扫描; 规则间互不干扰。

**A2 锚 (anchor)**: `patterns[0].regex` 匹配当前行 → **开块**, 消费该行, 进入扩展态。锚是开块的唯一途径。

**A3 扩展 (贪心 + 有序透传)**: 开块后状态机持有一个**扩展指针** `p`, 初始指向 `patterns[1]`。对每一新行:

- 用 `patterns[p]` 尝试当前行: 命中 → 消费该行; 若 `loop: true` 则 `p` 不动 (下一行继续试同一 pattern, 贪心); 若 `loop: false` 则 `p += 1`。
- 不命中 → `p += 1`, 用 `patterns[p+1..]` 中**下一个 pattern 继续尝试同一行** (透传); 所有剩余 pattern 都不命中 (`p` 越过末尾) → **闭块** (该行不属于块)。
- 闭块产出 fragment: block = 本块消费的连续行区间 `[first, last]`。

推论: `patterns[1..]` 全部是可选扩展 — 仅锚匹配、无任何扩展命中也闭块产出 (单行块)。不存在"断链惩罚"。

**A4 context 扩展**: 闭块后, block 向前扩 `context.before` 行、向后扩 `context.after` 行得到 fragment 的输出区间 (clamp 到日志边界)。context 只影响输出, 不影响状态机, 不参与与其他块的合并判定。

**A5 合并**: 所有 signal fragment 按 `(start, end)` 排序; **重叠或相邻** (`next.start <= cur.block_end + 1`, 判定用 block 区间) 合并为一个输出块 (output block)。输出块的 `start/end` = 各成员 fragment **输出区间** (block ∪ context 扩展, clamp 后) 的 hull。块内保留各 fragment 元数据。合并只发生在输出聚合层, 状态机层无感知。

**A6 noise 规则**: `kind: noise` 的规则同样跑状态机; 其 fragment 覆盖的行从 signal 输出中**剔除** (与 signal 块重叠的行级剔除: 输出时 noise 行集做差集; signal fragment 元数据保留, 其文本按剔除后残行呈现, 若残行为空则整个 fragment 丢弃)。

**A7 输出序**: 输出块按行序排列; 引擎不排序 fragment 内容 (保持日志原序, 尊重时间线)。

**A8 正则约束**: Rust `regex` crate 语法 (RE2 语义: 无 lookahead/lookbehind/backreference/反向引用, 支持命名组 `(?P<name>...)` 但 POC 不消费组)。规则加载时用 regex crate 编译, 编译失败 = 规则非法。

### 语义示例

```text
日志:                          规则: patterns:
L1: build start                 - regex: '^error: '
L2: error: undefined variable   - regex: '^\s'
L3:   at /flake.nix:42:3          loop: true
L4: build failed
```

- L1: 锚不匹配, 无事。
- L2: 锚匹配 → 开块, 消费 L2。
- L3: patterns[1] (`^\s`, loop) 匹配 → 消费, 贪心停留。
- L4: 不匹配 patterns[1] → 闭块。block = [L2, L3], fragment = L2-L3 两行。
- L4 若改写为 `  more context` 则被贪心吞入 (regex 约束防过度贪心: 扩展正则须尽量带缩进/前缀/结构约束)。

## 3. fragment 输出协议 (JSON)

```json
{
  "file": "path/or:-",
  "input_lines": 1000,
  "output_lines": 42,
  "blocks": [
    {
      "start_line": 120, "end_line": 135,
      "fragments": [
        { "rule": "nix-eval-error", "kind": "signal", "category": "typecheck",
          "confidence": "high", "block": [120, 128],
          "text": "...(context 扩展后的完整文本)..." }
      ]
    }
  ],
  "stats": { "noise_lines_removed": 200 }
}
```

- `start/end_line`: 合并块输出区间 hull (各成员 fragment 输出区间并集, 含 context 扩展; 1-based, 含端点)。
- fragment 的 `block` 是该规则自身消费区间 (无 context); `text` 含 context 扩展。
- `output_lines` = 合并块总行数 (noise 剔除后)。
- 人类可读输出 (默认): 块间 `---` 分隔, 块头 `[L120-L135] rule1,rule2`, 后跟文本。

## 4. L0 统计口径 (POC)

| 指标 | 定义 |
| --- | --- |
| 压缩率 | `1 - Σ(输出块行数) / 输入总行数` (对失败 run 计算; 成功 run 若有 signal 块则计入 FP) |
| FP 代理 | success run 上产出的 signal fragment 计数 (CI 通过却被提取出"错误" = 规则假阳性) |
| FN 代理 | failure run 的失败 job 日志中 signal fragment 数为 0 的 run 计数 (完全漏检); 部分漏检需 LLM 对照 (POC 抽查雏形, 不自动化) |
| 边界完整性 (机械) | fragment 的 block 尾行必须仍被该规则任一 pattern 匹配 (锚或扩展), 违例计数 (防 context 污染与实现 bug) |

## 5. 样本与配对 (POC 约定)

- 样本存放: `data/logs/<repo-slug>/<run_id>-<job_id>.log` + 同名 `.json` sidecar: `{repo, run_id, job_id, conclusion, workflow_name, fetched_at}`。
- **样本不入 git** (`data/` 在 .gitignore): 原始日志体积大且含他人内容; 回测本地跑。未来规则 PR 配最小化样本 (错误片段截取, ≤50 行) 入库 — v0 不做。
- 规则文件头注释标注来源样本 (`# samples: <repo>/<run>-<job>, ...`) 供人工溯源。

## 6. POC 明确不做

MCP 服务 / Adapter (LLM 学习) / manifest 按需拉取 / WebPage / noise 前缀树优化 / 多字节行边界优化 / CI 集成。引擎 = 加载 + 扫描 + 输出 + 统计, 四件事。
