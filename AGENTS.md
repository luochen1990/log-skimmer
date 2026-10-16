# Log Skimmer 维护者文档 (MVP wave1)

> 职责边界: 给维护者与 agent 的实现约定。规则格式/状态机语义的 SSOT 是 docs/rule-format.md, 本文不重复, 只写工程纪律。

## 布局

- `engine/` — 纯函数核心 (`skim`/`load_rules`/`normalize_lines`) + CLI 薄壳 (`log-skimmer` bin)。
- `server/` — MCP 集成层 (`log-skimmer-mcp` bin, rmcp stdio): `src/lib.rs` 纯逻辑视图 (skim 结果聚合/read_around 窗口/装载定位) + `src/mcp.rs` 协议层 (3 tool 宏注册, 输入 schema 结构约束)。零块 fallback 语义与判定矩阵见 spec §3; 协议层不得内嵌提取逻辑 (一律复用 engine lib)。
- `rules/` — 规则集 (folksonomy 布局, `_template.yaml` 贡献模板占位)。
- `scripts/` + `data/` — L0 家族与语料 fixture (见下)。

## 工程纪律

- **契约优先**: 引擎实现与规则编写的一切语义争议以 `docs/rule-format.md` 为准; 发现规范未覆盖的语义 → 先补规范再写代码/规则。
- **引擎核心是纯函数**: `skim(rules, lines) -> fragments`, 不做 IO/网络/时间依赖; MCP/CLI 都是纯核心外的薄包装。行归一唯一入口 `engine::normalize_lines` (A0 SSOT, 禁止消费点自写)。
- **依赖纪律**: engine 限 serde/serde_yaml/regex/clap 四类; server 例外登记 — `rmcp` (MCP 协议实现, 自研协议层不可行) / `tokio` (rmcp 运行时) / `serde_json` (tool 响应序列化)。新依赖须给理由登记于此。
- **测试**: 状态机公理 A1-A8 每条 ≥1 单测; 规则集加载回归 (`engine/tests/rules_load.rs`) 防语法漂移; server 侧 stdio 冒烟 + env 门控大语料 e2e (`LOG_SKIMMER_E2E_LOG=<log>`); 规则/引擎变更跑 `just ci-merge` (含 L0 全语料门禁: FN=0 / FP 率 ≤2% / 零块=0 / 语料在场 ≥60 / 压缩率基线防退化, 判定实现 `scripts/l0-gate.sh`)。
- **仓库拓扑 (四仓分工, 需求方裁决 2026-10-16, 接口面叙述见 README「仓库拓扑」)**: ① 本仓 (forgejo 私有) = 内部快速迭代 dev 面: 引擎 + L0 门禁脚本 + spec; ② GitHub `luochen1990/log-skimmer` = 发布面 (public), **不定期手动同步** (`just publish`), 稳定版引擎; ③ GitHub `luochen1990/log-skimmer-rules` = 规则库 (项目核心资产, 社区维护 + Agent 辅助), 引擎运行时单向依赖, 规则迭代直接在该仓 (本仓无 rules/ 目录); ④ GitHub `luochen1990/log-skimmer-corpus` = 语料库 (public 纯数据), **源头自包含零依赖**, GHA weekly 自刷槽位 (14F+5S per repo), 本仓 CI 单向消费。依赖不变量: 全单向无环 — corpus 零依赖; rules 本体零依赖 (其 CI 借发布面引擎+corpus 校验社区 PR); 本仓 CI 消费 corpus+rules (fetch-corpus/fetch-rules 幂等拉取)。
- **L0 压缩率防退化 = 双测对比 (基线文件已退役)**: PR CI 同语料同规则跑两轮 (master 引擎 vs PR 引擎), 对比 per-repo 压缩率 — 纯引擎变量归因, 退化 >2% 判红; 规则与语料在双测中同侧, HEAD 漂移自动消去, 无基线文件/无版本配对/无管辖域问题 (演进: 存储基线→随语料仓原子配对→双测运行时对比, 每步都是配对复杂度的消除)。FN/FP/零块/边界断言对 PR 侧全量 (含 forgejo 语料)。
- **Forgejo 侧语料 (隐私红线)**: **私有内网实例日志永不出内网** (含内网域名/网段/主机名等拓扑侦察面), `fetch-forgejo.sh` 按固定清单 (`data/forgejo-runs.json`, 仅数字元数据 — **禁止 display_title 等 PR 标题类字段**) 现场拉取 (内网 API 调用零出网流量), 换样本走 PR 改清单。新语料的 FN 信号由主仓下一 PR 的 l0-gate 红暴露 (归因明确); 规则头注释记 `# samples:` 溯源。
- **语料数据变更纪律 (sidecar 迁移事故教训)**: 语料数据 (样本/sidecar) 的变更一律在**语料仓工作副本** (data/logs 即其 clone) 内改并提交回推; 主仓工作副本对语料是**只读消费** — 就地改 sidecar 不回推会静默漂移 (ci 字段补丁曾只改主仓副本, 语料仓 57 对仍缺字段才暴露)。
- **公开镜像纪律**: 本仓经 `just publish` 同步到 GitHub public 发布面 — 入仓内容视同公开: 禁止内网运维细节 (ssh 通道/主机名/网段)、第三方私有仓 PR 标题类字段、凭据值; forgejo 侧数据 (manifest/语料) 保持最小数字元数据。
- **隐私纪律 (事故教训, 2026-10-16)**: forgejo 侧语料曾被推入 GitHub public 语料仓 ~1h (暴露内网拓扑: 域名/网段/主机名/用户路径; 无凭据值泄露), 重写历史重建止损。铁律: **数据出境判定先于数据移动** — 任何样本/日志/配置进入新仓前, 先问 "源数据的信任域是什么, 目标仓的暴露面是什么"; 私有实例数据的目标仓必须在同一信任域 (内网) 或经用户明确批准。语料仓 `.gitignore` 的 `git.lambda.lc__*/` 排除是机械防线 (corpus-refresh 的 `git add -A` 不裹挟), 勿删。
- **脚本栈纪律 (需求方裁决 2026-10-16: 长脚本不用 bash)**: 判定链已全 Rust 化 — `engine/src/l0.rs` (`log-skimmer l0` 子命令: 采集→汇总→判定, 口径与断言阈值 SSOT, **判定逻辑有单测** engine/tests/l0.rs; bash/jq 时代 l0-collect/summarize/gate/stats 四件已退役)。存留 bash 仅为薄编排与网络拉取: `l0-run.sh` (双测编排) / `l0-base.sh` (base 引擎检出构建) / `fetch-corpus.sh`+`fetch-rules.sh`+`fetch-forgejo.sh` (幂等拉取族) / `fetch-logs.sh` (槽位抓取, 语料仓自带同款副本)。新增逻辑超过 ~20 行或含条件分支管道组合时, 一律进 engine 子命令, 不再新增 bash 判定/统计脚本。
- 环境: devShell 经 flake.nix (`nix develop`), 提供 rust 工具链 + just/jq/ripgrep/yq; cargo 国内镜像 SSOT 在 `.cargo/config.toml` (rsproxy)。
- **CI**: `.forgejo/workflows/ci-merge.yml` (org 标准 P0 契约; `CORPUS_DIR=/var/lib/forgejo-runner/cache/corpus` 语料持久缓存 — 幂等 clone 一次后增量 pull, 语料流量 ~10MB/run → ~1MB/周) + `ci-periodic.yml` (P2 夜档, 判定器镜像同步 + 内网语料补抓, 骨架源自 lc-studio/forgejo-actions 只改 ⟦必改⟧/⟦可选⟧ 字段); 契约 justfile `ci-merge`/`ci-periodic` recipe。**vm-nix runner 契约 (secret-guard 同模式)**: VM systemPackages 预装 rust 工具链, **禁止 `nix develop`/`nix build` 在 VM 内求值 flake** (ro store + temproots 无写权限, 必炸 — 首跑 run 19423 实证); 编译产物经 job 级 env `CARGO_TARGET_DIR=/var/lib/forgejo-runner/cache/cargo-target` 落宿主持久卷跨 job 复用。secrets: `FORGEJO_API_TOKEN` (本实例 PAT, 读 lc-studio/nixos actions — forgejo 语料拉取; 自动注入的 GITHUB_TOKEN 是本仓 scoped 读不了 nixos)。GitHub 侧主仓 mirror (`luochen1990/log-skimmer`, push mirror 自动同步) 与语料仓刷新凭据在 GitHub 侧管理, 本实例零 GitHub 凭据。devShell (flake.nix) 仅服务本地开发。
- **workflow 文件排障纪律 (run 19427-19435 事故)**: run "Not started + Created=Stopped 同秒" = workflow 文件级错误 (forgejo 严格 YAML 拒载, 如 job 级 `env:` 键重复 — yq/普通解析器宽松测不出), 服务端日志有 `ignore invalid workflow` 明示 (forgejo 服务端 journal, 内网运维通道); **改 workflow 的 PR 推送后必须盯到 run 状态非秒挂**, "预期红" 论断只能建立在看过 run 详情之上 (误判 "等 secret 的红" 让重复键挂了三轮无人察觉)。改 workflow 文件优先整块 edit 替换, 禁 sed 单行插入 (env 块叠加即本次事故形态)。

## POC 交付物清单 (DoD, 已验收)

- [x] 样本: ≥15 失败 run + ≥15 成功 run (三仓合计), 含 sidecar meta
- [x] 规则: 四语料各自主要错误类型覆盖 (25 signal + 4 noise 内含本仓 9 条), 全部通过加载校验
- [x] 引擎: cargo test 15 用例全绿 (A0-A8 + intent/粒度/context), clippy clean
- [x] L0 报告: docs/poc-report.md (v2, 双语料 + 基线对照 + intent 分层)
