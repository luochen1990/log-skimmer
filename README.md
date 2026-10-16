# Log Skimmer

CI 日志高信噪比提取工具 — 以确定性规则集把失败的 CI 日志压缩成带坐标、带语义标注的高信噪比摘要，供 AI agent 消费。

> 引擎与规则集已在 4 个真实仓库的语料上完成回测验证 (见下文数据), 并以 L0 门禁化语料回归守住质量基线; CLI 与 MCP server (`log-skimmer-mcp`) 双形态可用。

## 项目动机

AI agent 排查 CI 失败时, 日志消费存在结构性困境:

- **全量日志太大**: 一次失败的 job 动辄数千上万行, 整体塞进上下文既昂贵, 又让关键错误淹没在噪音里。
- **关键词基线假阳性高**: agent 的保底手段 `rg -i 'error|warn'` 会把成功 job 里大量无害的 warning 字样当成错误信号。POC 实测: 该基线在 18 个成功 job 上产出 **230 行假阳性** (FP 率 100%, 均值 12.8 行/job); Log Skimmer 同口径为 **0 行**。
- **单行正则抓不住错误树**: nix/rustc 等工具的失败证据是多行结构 (POC 语料中最大的 nix 错误树达 219 行), `rg` 只回显孤立锚行, 支撑「瞥见」但无法支撑「定位」。

Log Skimmer 的解法是**学习型规则集 + 运行期纯正则逐行状态机**: 规则描述「各类工具的失败证据长什么样」(锚行 + 多行扩展), 引擎按规则提取出完整错误树并附加坐标与语义标注。LLM 只在未来学习期 (规则生成 Adapter) 参与, 读取期零 LLM、零网络、结果可复现。

POC 回测语料: **4 个真实仓库、67 对成败样本 (49 失败 / 18 成功)、约 6 万行日志**, 覆盖 GitHub Actions 与 Forgejo Actions 两种 CI 平台:

| 仓库 | 工具栈 | 形态特征 |
| --- | --- | --- |
| anomalyco/opencode (原 sst/opencode, 2026-10 易主) | bun / eslint / playwright | GHA 时间戳前缀 + ANSI 色码 |
| niri-wm/niri | cargo / rustc / rust-test | 同上 |
| nix-community/home-manager | nix / git / nixfmt | 完整多行错误树 (最大 219 行) |
| lc-studio/nixos | nix / just / treefmt / nim / systemd | Forgejo 沙箱轮询回显前缀 |

核心结果: FN (失败完全漏检) 0/49; FP 0 行 vs 基线 230 行; 双粒度压缩率 — 块级 83.9%-98.9%, 签名行级 99.18%-99.73% (巨树场景收益最大: 219 行错误树在签名行级收敛为 1 行索引)。

## 仓库拓扑

项目由四个仓组成, 依赖全部单向 (无环), 各有信任域与演进节奏:

| 仓 | 位置 | 定位 | 演进节奏 | 依赖 |
| --- | --- | --- | --- | --- |
| `lc-studio/log-skimmer` (dev) | forgejo 私有 | 内部快速迭代: 引擎 + L0 门禁脚本 + spec | 每日 | CI 消费 corpus + rules |
| `luochen1990/log-skimmer` (发布面) | GitHub public | 稳定版引擎发布 | 不定期手动同步 (`just publish`) | 零 (被 rules 仓 CI 消费) |
| `luochen1990/log-skimmer-rules` | GitHub public | **规则库** — 项目核心资产, 社区维护 + Agent 辅助 | 社区 PR | CI 借发布面引擎 + corpus 做校验; 本体零依赖 |
| `luochen1990/log-skimmer-corpus` | GitHub public | **语料库** — 回归语料, 源头, 自包含 | GHA weekly 自刷槽位 | **零依赖** |

```
forgejo dev 仓 ──不定期手动同步──▶ GitHub 发布面
                                      │ 引擎 (rules 仓 CI 校验用)
                                      ▼
GitHub rules 仓 ◀──运行时单向依赖── CLI (log-skimmer skim --rules)
GitHub corpus 仓 ◀──CI 单向依赖──── dev 仓 L0 门禁 (语料+规则在场)
```

不变量: ① 依赖单向 — corpus 是源头 (零依赖, 被依赖), rules 只被消费, 无环; ② 信任域 — forgejo 侧私有 (内网语料可近), GitHub 侧 public (零内网数据); ③ L0 压缩率防退化不依赖基线文件 — PR CI **双测对比** (master 引擎 vs PR 引擎 × 同语料同规则), 规则与语料在双测中同侧, 漂移自动消去。

## 关键特性

- **intent 三分**: 每条规则标注提取意图 — `failure` (失败证据, 在成功 job 命中才算假阳性) / `warning` (有问题倾向但非失败, 如 deprecation) / `info` (纯环境快照, 如 runner 版本)。分层让 FP 统计口径语义正确: 「成功 job 里有 warning」是预期而非误报。
- **category 封闭枚举**: 10 个封闭值 (`fmt` / `lint` / `typecheck` / `build` / `test` / `security` / `deps` / `infra` / `config` / `unknown`), 描述信息类别, 与 intent 正交。agent 可据此程序化过滤, 不必再对文本做二次理解。
- **零块安全网**: 规则集底座恒有双侧平台安全网 — 失败侧 (CI 平台 annotation / runner 失败信号) 兜住失败日志, 成功侧 (runner 版本等环境 info 规则) 兜住成功日志。由此「任何真实 CI 日志 skim 输出非零」成为**结构承诺** (POC 67/67 兑现); 零块 = 规则集对该日志失明, 与 job 成败无关。
- **结构承诺与可复现**: 零 LLM、零网络, 逐行状态机 + RE2 语义正则 (线性复杂度), 同输入恒同输出。输出定位为「索引 + 摘要」: 每个块恒带行号坐标与元数据 (规则 / category / intent / confidence), agent 据此决定是否展开上下文, 而非把输出当日志替代品。
- **规则 folksonomy**: 无中央注册表, 规则文件的存在即定义工具 ID (`<tool-id>/<rule-id>.yaml`, 一文件一规则)。规则集独立于引擎, 住在 [log-skimmer-rules](https://github.com/luochen1990/log-skimmer-rules) (社区维护 + Agent 辅助), 当前 15 个工具共 30 条规则, 覆盖 bun / cargo / eslint / forgejo-actions / git / github-actions / just / nim / nix / nixfmt / playwright / rust / rustc / systemd / treefmt。

路线图: LLM 规则学习 Adapter (日志 → 规则草案 PR + 回测报告)、Validator 双层验证、WebPage 规则浏览站属后续规划; MCP server (`server/`) 已交付。

## 快速上手

构建 (Rust toolchain) + 规则集获取 (规则独立成仓, 社区维护):

```bash
cargo build --release
# 产物: target/release/log-skimmer
git clone --depth 1 https://github.com/luochen1990/log-skimmer-rules /path/to/rules
```

两种输出形态:

```bash
# 人类可读 (默认): 块间 --- 分隔, 块头带行号坐标与命中规则, 尾行输出压缩统计
target/release/log-skimmer skim failed-job.log --rules /path/to/rules

# JSON 协议 (--json): 块/fragment 结构化坐标与元数据 + L0 统计, 供 agent 程序化消费
target/release/log-skimmer skim failed-job.log --rules /path/to/rules --json

# --context N: 全局覆盖上下文扩展为前 N 行 / 后 N 行 (默认按规则自带)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules --context 3
```

退出码语义 (调用方契约):

| 退出码 | 含义 | 调用方行为 |
| --- | --- | --- |
| 0 | 正常完成 | 消费输出 |
| 2 | 规则集非法 (加载校验失败) 或日志读取失败 | 修复规则 / 日志后重试 |
| 3 | 零块失明 — 规则集对该日志无任何命中 | 无条件回退 `rg -i 'error|warn'` 基线 |

### MCP server (agent 集成形态)

`log-skimmer-mcp` 以 stdio transport 暴露三个 tool: `skim(log_text, context?)` (提取块 + fragment 摘要 + 压缩统计; 零块时返回 `fallback: "no-blocks"` 与回退建议字段, 不报错) / `read_around(log_text, line_start, line_end)` (按坐标取原文窗口, 单次限 200 行) / `list_rules()` (装载规则清单)。

```bash
cargo build --release -p log-skimmer-server
# 产物: target/release/log-skimmer-mcp (stdio transport)
```

opencode / Claude 等客户端配置示例 (command 形态):

```json
{
  "local": {
    "log-skimmer": {
      "type": "local",
      "command": ["/path/to/log-skimmer-mcp"],
      "enabled": true
    }
  }
}
```

部署前提: 二进制旁须有规则集目录 (log-skimmer-rules 仓 clone), 或设 `LOG_SKIMMER_RULES` 指向它 (缺省探测编译期路径与可执行文件相邻路径, 均失败时 fail-fast 退出码 2)。单条日志上限 32MB, 超限报错并建议 `tail -c` 截取。

开发验证 (CI 与本地同链: 直接调用 — 需 PATH 有 rust 工具链; 纯 nix 环境经 devShell):

```bash
just ci-merge              # CI 合入门禁: 语料获取 → fmt → clippy → test → L0 全语料门禁 (vm-nix 预装工具链直用)
nix develop -c just check  # 系统无 rust 时: devShell 内快速循环 (fmt + clippy + test)
```

回归语料双源: [log-skimmer-corpus](https://github.com/luochen1990/log-skimmer-corpus) (公开仓 CI 日志滚动快照, GHA weekly 自刷) + 私有内网实例日志按固定清单现场拉取 (数据不出内网), 首次跑门禁时自动获取。

## 贡献指引

- 新增或修改规则: 到 [log-skimmer-rules](https://github.com/luochen1990/log-skimmer-rules) 提 PR, 读仓内 README (规则贡献指南与打标纪律)。
- 规则格式、状态机语义与输出协议的规范 SSOT: [docs/rule-format.md](docs/rule-format.md)。
