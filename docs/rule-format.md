# Log Skimmer 规则格式与状态机规范 (v0)

> 职责边界: 本文是规则 YAML 格式、逐行状态机语义、fragment 输出协议与 L0 统计口径的**单一事实来源**, 是规则编写者与引擎实现者的共同契约。两者对语义的任何疑问以本文为准; 本文未定义的行为视为未指定 (实现不应依赖)。
> 版本: v0.3 (POC)。v0.1 吸收 T2 实测反馈 (A0); v0.2 吸收第二轮增量建议 (intent/坐标恒带/双粒度/三态与 rerun); v0.3 intent 三分化 (failure|warning|info, 需求方反馈: 非 failure 信息区分问题倾向与纯环境快照)。字段只增不删地向前演进; 语义变更须同步 bump 版本并迁移规则。

## 0. 设计灵魂 (约束一切后续演进)

- **LLM 只在学习期 (Adapter), 读取期 (Reader/引擎) 纯确定性**: 本规范描述的运行时零 LLM、零网络、可复现、可审计。
- **召回优先 (需求方决策, 2026-10-16)**: FN (失败被漏检) 后果是 agent 误判成败或失去定位能力、被迫回退全量日志 — 工具失去存在意义; FP 后果只是多几行噪音; 压缩率是结果性指标。**优先级恒为: 漏检 = 0 > 假阳性容忍 > 压缩率** — 宁可规则覆盖过头牺牲压缩, 不可漏。结构配套: ① **双侧平台安全网恒居规则集底座** — 失败侧 (annotation/runner 失败信号) 兜住失败日志, 成功侧 (runner 版本等环境 info 规则) 兜住成功日志 (成功日志至少一条 info 是规则集义务); ② 零块警示无条件 (§3) — 双侧安全网使「任何 CI 日志 skim 非零」成为结构承诺, 零块 = 失明, 调用方无条件 fallback, 判定不依赖外部预期元数据。
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
repo: null                    # scope: repo 时必填, "host/owner/repo" (如 github.com/sst/opencode; 多 CI 平台 host 消歧)
category: unknown             # 封闭枚举 10 值 (见下表); 省缺 = unknown (装载入口统一注入, 宁缺勿错)
intent: failure               # failure|warning|info — 见下方语义表; noise 规则必须缺省
confidence: medium            # high|medium|low (片段继承, 排序参考)
context: { before: 0, after: 0 }   # 输出边界扩展 (行数, 不参与状态机)
structured-hint: null         # 未来字段, POC 不实现, 保留占位
attribution: null             # POC 手写规则省略; Adapter 生成时必填
#   attribution: { contributor: str, generated-by: {harness: str, model: str}, adapter-version: str }
```

`intent` 语义 (FP/FN 统计分层的依据, 与 `category` 正交 — category 说「什么类别的信息」, intent 说「命中对成败判定意味着什么」):

| 取值 | 语义 | 例子 | 统计口径 |
| --- | --- | --- | --- |
| `failure` | 提取失败证据; 在通过的 job 里命中 = FP | rustc `error[E0308]` / nix error 树 / pytest `FAILED` | **FP 只在这类规则上统计**; 参与 FN |
| `warning` | **有问题倾向**但非失败 — 将来会坏/值得注意的信号 | `warning: ... is deprecated` / `colliding subpath` | 不计 FP; 不参与 FN |
| `info` | **纯环境/上下文快照**, 无问题倾向 — 排除环境因素、复现定位时有用 | runner/工具链版本行 / 测试通过摘要 (`N pass / 0 fail`) | 不计 FP; 不参与 FN |

warning 与 info 的区分是「问题倾向」: 前者可能提示根因 (诊断时高优先), 后者是环境事实 (按需取用)。消费端呈现分层 (如 info 默认折叠) 由 MVP 定稿, 引擎/统计口径二者等价。

字段约束 (加载期校验, 违例 = 规则集非法, 整体拒绝加载):

| 字段 | 约束 |
| --- | --- |
| id | 匹配 `^[a-z0-9-]+$`, 且以 `tool` 值为前缀 |
| scope/repo | `scope: repo` 时 repo 必填; `scope: global` 时 repo 必须为 null |
| patterns | 非空数组; 每项 regex 必填; loop 默认 false |
| context.before/after | 0 ≤ 值 ≤ 10 |
| kind/confidence/category | 枚举值; category 省缺视为 unknown (装载入口统一注入) |
| repo | `host/owner/repo` 完整形态; 裸 `owner/repo` 缺 host 视为非法 |

`category` 封闭枚举 (调研定稿, 见 docs/category-survey.md; 两域混合, 域归属仅作文档/WebPage 分组, 不进枚举值):

| 值 | 域 | 定义 (判定基准: 工具在 CI 中的角色优先, 日志形态辅助) |
| --- | --- | --- |
| `fmt` | 质量门 | 格式化工具的漂移报告 |
| `lint` | 质量门 | 静态分析/风格/反模式告警 (不含类型诊断与安全扫描) |
| `typecheck` | 质量门 | 独立类型检查器/编译器的类型诊断 (tsc/mypy/rustc `error[E...]`) |
| `build` | 质量门 | 编译/链接/打包/构建系统失败 (非类型诊断) |
| `test` | 质量门 | 任何测试框架的断言/用例失败 (unit/integration/VM/e2e 统一; 测试类型由 tool 表达) |
| `security` | 质量门 | 漏洞/密钥泄漏/危险模式扫描告警 (含依赖漏洞**内容**; 依赖**获取**失败归 deps) |
| `deps` | 环境根因 | 依赖解析/下载/版本冲突/哈希校验失败 |
| `infra` | 环境根因 | 网络/凭据/资源 (OOM/磁盘)/超时/runner 故障/工具链崩溃/git 操作失败 |
| `config` | 环境根因 | 配置文件/声明式求值/生成物漂移 (nix eval, `nginx -t`, systemd verify, gen 哨兵) |
| `unknown` | 兜底 | 无法唯一判类 |

打标纪律 (**宁缺勿错**): 错标污染查询语义 (agent 程序化过滤, 不可检测), unknown 只损失该规则自身的过滤价值。多因混合/类别超出枚举 (集合演化信号, PR 记录样例)/样例不足/L0 回测发现规则命中多类日志 → 打 unknown。unknown 占比长期 >40% 触发集合审视; unknown 升级具名走 PR 人工确认。flaky 是属性不是类别, 不进 category (未来独立维度)。
| intent | `failure`(默认)/`warning`/`info`; **kind: noise 时必须缺省** |

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

输出定位为「**索引+摘要**」而非日志替代品 — 每个片段恒带坐标与元数据, 消费方 (agent) 据此决定是否展开上下文。

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
          "intent": "failure", "confidence": "high", "block": [120, 128], "out": [120, 135],
          "text": "...(context 扩展后的完整文本)..." }
      ]
    }
  ],
  "stats": { "noise_lines_removed": 200, "signature_output_lines": 12 }
}
```

- `start/end_line`: 合并块输出区间 hull (各成员 fragment 输出区间并集, 含 context 扩展; 1-based, 含端点)。
- fragment 的 `block` 是该规则自身消费区间 (无 context); `text` 含 context 扩展。
- `output_lines` = 合并块总行数 (noise 剔除后) — **块级粒度**。
- `signature_output_lines` = 各 fragment block 首行 (锚行/签名行, 去重行号) 计数 — **签名行级粒度**。压缩比按两档粒度分列统计 (两数字差异大, 验收线不可混谈)。
- CLI `--context N` (默认按规则自带 context): 显示层全局覆盖为 before=N/after=N, 纯机械按坐标切割, 不需规则参与。
- CLI 零块警示 (**无条件**, spec §0「召回优先」的运行时配套): 「任何真实 CI 日志 skim 非零」是规则集的**结构承诺**, 由双侧安全网兑现 — 失败侧 (CI 平台 annotation/runner 失败信号规则) + 成功侧 (平台环境 info 规则, 如 runner 版本行; 成功日志至少一条 info 是规则集义务而非建议)。零块 = 规则集对该日志**失明**, 与 job 成败无关 → stderr 醒目警告 + **退出码 3**, 调用方无条件回退 rg 基线。不需要调用方传入任何预期元数据 — 判定完全内生于规则集的覆盖承诺。

**消费方判定矩阵** (agent 使用契约; warning/info 与成败无关, 不参与判定):

| job 结论 (消费方已知) | skim 块 | failure-intent 块 | 判定 |
| --- | --- | --- | --- |
| 任意 | 零 | — | 结构失明 → 工具已 exit 3, 无条件 fallback |
| 成功 | 非零 | 0 | 正常 (目标态; 质量指标非不变量) |
| 成功 | 非零 | 非零 | FP (后果轻: 多几行噪音; 验收线容忍, 无须 fallback) |
| 失败 | 非零 | 非零 | 正常 (失败侧安全网结构保证) |
| 失败 | 非零 | 0 | **异常** — 已知边界: 日志截断 (runner 崩溃丢尾部失败信号) 或规则失明; 工具无 conclusion 无法自判, 消费方一行检查 `结论=failure 而 failure 块零 → 视同失明 → fallback` |
- 人类可读输出 (默认): 块间 `---` 分隔, 块头 `[L120-L135] rule1,rule2`, 后跟文本。

## 4. L0 统计口径 (POC)

Ground truth 单位 = **job 级 result** (API 元数据, 非推断)。语料标签贴在 (run, attempt) 级。

| 指标 | 定义 |
| --- | --- |
| 压缩率 | `1 - Σ(输出行数) / 输入总行数`, 按粒度分列 (块级 / 签名行级), 按工具分列 |
| FP | **仅 `intent: failure` 规则**: success job 上产出的此类 fragment 计数 (CI 通过却被提取出失败证据 = 假阳性)。`intent: warning` 规则命中属预期, 单列「warning 命中」不计 FP。**FP 率 = failure-intent fragment 的 block 区间行数总和 / success 样本输入总行数** (验收线 ≤2%, 分母口径 SSOT — 工具被污染程度只在 success 样本上度量; 分子取 block 区间不含 context 扩展, 免重叠双计) |
| FN 代理 | failure 家族 (failure/timed_out) job 日志中 `intent: failure` fragment 数为 0 的 job 计数 (完全漏检) — **验收线 = 0, 不变量** (召回优先, §0; 宁可覆盖过头牺牲压缩); 部分漏检需 LLM 对照 (POC 抽查雏形, 不自动化) |
| 边界完整性 (机械) | fragment 的 block 尾行必须仍被该规则任一 pattern 匹配 (锚或扩展), 违例计数 (防 context 污染与实现 bug) |

**cancelled/skipped 三态** (证据能力非对称):

- `skipped`: 无日志, 完全排除出统计。
- `cancelled`: 截断日志**只记命中, 不记缺失** — 命中 failure-intent 规则计为弱 TP (fail-fast 连坐场景下命中是真信号), 单独报告不进主指标; 未命中不计 FN (信息不完整)。
- `timed_out` (GHA 独立 conclusion): 单独留意 — timeout 日志本身是可规则化的失败模式, 语料标注时专门收集。

**rerun 处理** (取数层元数据, 不用正则): sidecar 必带 `attempt`; rerun-through (attempt>1 且最终成功) 排除出主指标; 其失败 attempt 是优质 failure 语料, 按 attempt 结论打标签收进语料库。

## 5. 样本与配对 (POC 约定)

- 样本存放: `data/logs/<host>/<owner>/<repo>/<YYYY-MM-DD>/<run_id>-<job_id>-<job_name>-<conclusion>.log` + 同名 `.json` sidecar: `{repo, run_id, job_id, conclusion, workflow_name, created_at, job_name, attempt, ci, fetched_at}` — **嵌套自然路径** (需求方裁决, 弃 `__` 扁平化: 机器遍历用递归 find 无成本, 人类可读性优先 — 日期/触发 job/成败进路径, 裸数字文件名不可读); `repo` 为 **`host/owner/repo` 完整形态** (元数据 SSOT 在 sidecar, 路径仅为可读性投影); conclusion 取 **job 级 result**; `created_at` (run 级) / `job_name` 取数时从 API 元数据写入; `job_name` 清洗规则: `/`与空格→`-`, 截 30 字符。
- **语料独立仓 (双源, 需求方裁决, 2026-10-16)**: 语料不入引擎仓 — 公开 CI 日志是互联网随时可拉取的资源, 入仓只会腐坏。双源形态: ① GHA 侧 (公开仓) 滚动槽位存独立语料仓 `github.com/luochen1990/log-skimmer-corpus` (public, **源头自包含零依赖** — GHA weekly 自刷槽位, 不构建引擎不算基线), 引擎仓 CI 经幂等拉取脚本消费; ② **私有语料源** (含敏感环境信息, 不公开分发)。`data/logs`/`data/rules` 是拉取脚本的工作副本 (gitignored); L0 门禁的语料在场 ≥50 断言同时兜底获取失败 (兼容 rules 仓单源 CI)。规则集独立成仓 `github.com/luochen1990/log-skimmer-rules` (社区维护 + Agent 辅助, 规则 PR 的质量门禁在该仓 CI: 发布面引擎 × corpus 语料 × master-vs-PR 规则双测)。压缩率防退化走 PR CI 双测对比: master vs PR 引擎 × 同语料同规则, 无基线文件无配对问题。
- 规则文件头注释标注来源样本 (`# samples: <repo>/<run>-<job>, ...`) 供人工溯源。

## 6. POC 明确不做

MCP 服务已交付; POC 明确不做: Adapter (LLM 学习) / manifest 按需拉取 / WebPage / noise 前缀树优化 / 多字节行边界优化 / CI 集成。引擎 = 加载 + 扫描 + 输出 + 统计, 四件事。
