# Log Skimmer

[English](README.md) | [简体中文](README.zh-CN.md)

CI 日志高信噪比提取工具 — 以确定性规则集把失败的 CI 日志压缩成带坐标、带语义标注的高信噪比摘要，供 AI agent 消费。

> 引擎与规则集已在 3 个 GitHub 公开仓 + 1 个私有语料源共 67 对成败样本上完成回测验证 (见下文数据); CLI 与 MCP server (`log-skimmer-mcp`) 双形态可用。

## 项目动机

AI agent 排查 CI 失败时, 日志消费存在结构性困境:

- **全量日志太大**: 一次失败的 job 动辄数千上万行, 整体塞进上下文既昂贵, 又让关键错误淹没在噪音里。
- **关键词基线假阳性高**: agent 的保底手段 `rg -i 'error|warn'` 会把成功 job 里大量无害的 warning 字样当成错误信号。POC 实测: 该基线在 18 个成功 job 上产出 **230 行假阳性** (FP 率 100%, 均值 12.8 行/job); Log Skimmer 同口径为 **0 行**。
- **单行正则抓不住错误树**: nix/rustc 等工具的失败证据是多行结构 (POC 语料中最大的 nix 错误树达 219 行), `rg` 只回显孤立锚行 (错误树的首行, 如 `error: ...`), 支撑「瞥见」但无法支撑「定位」。

Log Skimmer 的解法是**学习型规则集 + 运行期纯正则逐行状态机**: 规则描述「各类工具的失败证据长什么样」(锚行 + 多行扩展), 引擎按规则提取出完整错误树并附加坐标与语义标注。LLM 只在未来学习期 (规则生成 Adapter) 参与, 读取期零 LLM、零网络、结果可复现。

POC 回测语料: **3 个 GitHub 公开仓 + 1 个私有语料源, 67 对成败样本 (49 失败 / 18 成功)、约 6 万行日志**, 覆盖 GitHub Actions 与 Forgejo Actions 两种 CI 平台:

| 仓库 | 工具栈 | 形态特征 |
| --- | --- | --- |
| anomalyco/opencode (原 sst/opencode, 2026-10 易主) | bun / eslint / playwright | GHA 时间戳前缀 + ANSI 色码 |
| niri-wm/niri | cargo / rustc / rust-test | 同上 |
| nix-community/home-manager | nix / git / nixfmt | 完整多行错误树 (最大 219 行) |
| 私有语料源 (private corpus) | nix / just / treefmt / nim / systemd | Forgejo 沙箱轮询回显前缀 |

核心结果: FN (失败完全漏检) 0/49; FP 0 行 vs 基线 230 行; 双粒度压缩率 — 块级 83.9%-98.9%, 签名行级 (只计锚行) 99.18%-99.73% (巨树场景收益最大: 219 行错误树在签名行级收敛为 1 行索引)。

## 仓库拓扑

项目由三个公开仓组成, 依赖全部单向 (无环):

| 仓 | 定位 | 演进节奏 | 依赖 |
| --- | --- | --- | --- |
| `luochen1990/log-skimmer` (本仓) | **引擎** — CLI + MCP server | 不定期发布 | 零 (被 rules 仓 CI 消费) |
| `luochen1990/log-skimmer-rules` | **规则库** — 项目核心资产, 社区维护 + Agent 辅助 | 社区 PR | 运行时单向依赖本仓引擎 |
| `luochen1990/log-skimmer-corpus` | **语料库** — 回归语料, 源头, 自包含 | GHA weekly 自刷槽位 | **零依赖** |

```
rules 仓 ◀──运行时单向依赖── CLI (log-skimmer skim --rules)
rules 仓 CI ──规则校验──▶ 本仓引擎 × corpus 语料 (master-vs-PR 规则双测)
```

不变量: 依赖单向 — corpus 是源头 (零依赖, 被依赖), rules 只被消费, 无环。

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
# 人类可读 (默认): 块间 --- 分隔, 块头带行号坐标、intent/category 与命中规则, 尾行输出压缩统计
# (人类可读输出整体走 stdout, `> out.txt` 重定向完整保留; 错误/警告才走 stderr)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules

# JSON 协议 (--json): 块/fragment 结构化坐标与元数据 + L0 统计, 供 agent 程序化消费
# (stdout 纯 JSON, 统计行等诊断走 stderr)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules --json

# --context N: 全局覆盖上下文扩展为前 N 行 / 后 N 行 (默认按规则自带; 取值 0-10, 越界为用法错误)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules --context 3
```

退出码语义 (调用方契约):

| 退出码 | 含义 | 调用方行为 |
| --- | --- | --- |
| 0 | 正常完成 | 消费输出 |
| 2 | 数据错误: 规则集非法 (加载校验失败) / 日志读取失败 / 日志为空 (0 行, 疑似下载截断) | 修复数据后重试 |
| 3 | 零块失明 — 规则集对该日志无任何命中 | 无条件回退 `rg -i 'error\|warn'` 基线 |
| 64 | 用法错误 — 参数解析失败 (BSD sysexits `EX_USAGE`; `--help`/`--version` 仍 exit 0) | 检查命令拼写 (stderr 有 clap 输出), 修数据无济于事 |

### MCP server (agent 集成形态)

`log-skimmer-mcp` 以 stdio transport 暴露三个 tool: `skim(log_text, context?)` (提取块 + fragment 摘要 + 压缩统计 (零块时省略); 零块时返回 `fallback: "no-blocks"` 与回退建议字段, 不报错) / `read_around(log_text, line_start, line_end)` (按坐标取原文窗口, 单次限 200 行) / `list_rules()` (装载规则清单)。

```bash
cargo build --release -p log-skimmer-server
# 产物: target/release/log-skimmer-mcp (stdio transport)
```

客户端配置示例 — opencode (`opencode.json` 的 `mcp` 段; 环境变量经 `environment` 字段传):

```json
{
  "mcp": {
    "log-skimmer": {
      "type": "local",
      "command": ["/path/to/log-skimmer-mcp"],
      "environment": { "LOG_SKIMMER_RULES": "/path/to/log-skimmer-rules" },
      "enabled": true
    }
  }
}
```

Claude Desktop (`claude_desktop_config.json` 的 `mcpServers` 段; 环境变量经 `env` 键传):

```json
{
  "mcpServers": {
    "log-skimmer": {
      "command": "/path/to/log-skimmer-mcp",
      "env": { "LOG_SKIMMER_RULES": "/path/to/log-skimmer-rules" }
    }
  }
}
```

规则集定位 (启动时一次性解析, 目录不存在 / 无 `*.yaml` / 加载校验失败均 fail-fast 退出码 2):

1. 环境变量 `LOG_SKIMMER_RULES` 指向的**规则集目录** (log-skimmer-rules 仓的 clone, 含 `*.yaml`) — 已设置即采用、不回落后续探测 (指向无效目录直接 fail-fast), 规则集不在二进制旁时的标准部署手段;
2. 缺省探测 — 可执行文件所在目录依次尝试相邻的 `rules/` → `../rules/` → `../../rules/` (首个为目录者生效; 覆盖「rules 与 bin 同级」部署布局与 cargo `target/{debug,release}/` 构建布局);
3. 编译时嵌入的本仓源码树 fixture 路径 — 开发/单测形态兜底, 部署后通常不存在。

输入上限 (**仅 MCP 侧**): 单条 `log_text` 超过 32MB 报 invalid_params 并建议 `tail -c` 截取后分段提交; CLI 文件输入 (`skim <log>`) 无此限制。

server 进程退出码不是 MCP 工具调用方契约 — 上表只约束 `log-skimmer` CLI: `log-skimmer-mcp` 启动期 exit 2 = 规则集装载失败 fail-fast (部署环境问题, 见上句), 排障应检查 stderr (MCP 协议只占 stdout, stderr 是日志通道), 修复数据或重试调用无济于事。

开发验证:

```bash
cargo test --workspace     # 单测: 引擎状态机公理 + 规则加载回归 + MCP stdio 冒烟
nix develop -c just check  # 系统无 rust 时: devShell 内快速循环 (fmt + clippy + test)
```

回归语料: [log-skimmer-corpus](https://github.com/luochen1990/log-skimmer-corpus) (公开仓 CI 日志滚动快照, GHA weekly 自刷)。

## 贡献指引

- 新增或修改规则: 到 [log-skimmer-rules](https://github.com/luochen1990/log-skimmer-rules) 提 PR, 读仓内 README (规则贡献指南与打标纪律)。
- 规则格式、状态机语义与输出协议的规范 SSOT: [docs/rule-format.md](docs/rule-format.md)。

### 贡献者本地验证 (纯公开件)

规则贡献的完整本地验证只需三个公开仓, 无需任何私有凭据:

```bash
# ① 三件套: 引擎 (本仓) + 公开语料仓 + 规则仓 fork (改动在 fork 分支上做)
cargo build --release
git clone --depth 1 https://github.com/luochen1990/log-skimmer-corpus /path/to/corpus
git clone --depth 1 https://github.com/luochen1990/log-skimmer-rules /path/to/rules

# ② 快验: 单样本 skim (退出码 3 = 规则集对该日志零命中; 单规则是否命中以 ③ 的
#    per_rule_hits 归因为准)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules

# ③ 全量门禁: 全语料回归 (FN=0 / FP率≤2% / 零块=0 / 语料在场≥50) + 每规则命中报告
#    per_rule_hits。新规则的正向通过信号: 在 per_rule_hits 中计数 ≥1 (即不在 dead_rules 列表)。
target/release/log-skimmer l0 --corpus /path/to/corpus --rules /path/to/rules

# ④ 严格模式 (可选): 存在语料零命中规则即 fail。注意现有规则集含公开语料暂无
#    样本的天然零命中工具 (见 dead_rules 列表), 全量开启会列出它们 — 严格模式
#    适用于「语料应覆盖全部规则」的场景; 对新规则的定向验证以 ③ 的信号为准。
target/release/log-skimmer l0 --corpus /path/to/corpus --rules /path/to/rules --require-all-rules-hit
```

l0 退出码说明: 语料目录不存在或零配对样本属数据错误 (exit 2, 修环境/取数链路); 以自组小语料 (<50 配对样本) 跑 ③ 会命中语料在场断言而 fail (exit 1) — 定向验证请以 per_rule_hits 为信号。样本 = `*.log` + 同名 `*.log.json` 配对, 单侧缺失的孤儿样本在 stderr 警告并跳过 (不计数)。

新工具规则 PR 请附最小样例日志 (溯源写法见规范 §5 `samples: self-provided` 约定)。

注: 开发仓内部的 `just ci-merge` 全链含内网语料拉取段, 属内网维护者路径 (公开克隆形态不含该 recipe); 社区贡献者走上述公开件路径即可完成等价的规则质量验证。

## License

- 引擎与服务: Apache-2.0 (见 [LICENSE](LICENSE))。
- 规则集 [log-skimmer-rules](https://github.com/luochen1990/log-skimmer-rules): CC0 1.0 (规则与文档, 无版权保留)。
- 语料仓 [log-skimmer-corpus](https://github.com/luochen1990/log-skimmer-corpus): 仓内编排与元数据 CC0 1.0; 日志样本内容版权归原作者 (再分发口径见该仓 README)。
