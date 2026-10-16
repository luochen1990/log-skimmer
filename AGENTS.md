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
- **语料双源 (需求方裁决)**: 语料不入主仓。① GHA 侧: 语料仓 `github.com/luochen1990/log-skimmer-corpus` (public), **GHA 全闭环自刷新** (需求方裁决 2026-10-16: 刷新全走 GitHub Actions, 语料字节不经内网出口省外网流量) — 语料仓自带判定器**镜像** (engine 提升根包 + rules + L0 四脚本, 由主仓 `corpus-sync.sh` 单向同步, 语料仓勿手编) + `refresh.yml` workflow (weekly cron + engine-sync dispatch: 槽位重拉 14F+5S → 镜像引擎重算基线 → 原子配对单 commit 推送), 全程 GitHub 机房零家庭流量; ② Forgejo 侧: **隐私红线 — 私有内网实例日志永不出内网** (含内网域名/网段/主机名等拓扑侦察面), `fetch-forgejo.sh` 按固定清单 (`data/forgejo-runs.json`) 现场拉取 (内网 API 调用零出网流量), 换样本走 PR 改清单。新语料的 FN 信号由主仓下一 PR 的 l0-gate 红暴露 (归因明确); 规则头注释记 `# samples:` 溯源。
- **语料数据变更纪律 (sidecar 迁移事故教训)**: 语料数据 (样本/sidecar/基线) 的变更一律在**语料仓工作副本** (data/logs 即其 clone) 内改并提交回推; 主仓工作副本对语料是**只读消费** — 就地改 sidecar 不回推会静默漂移 (ci 字段补丁曾只改主仓副本, 语料仓 57 对仍缺字段, baseline-empty 防线咬住才暴露)。
- **隐私纪律 (事故教训, 2026-10-16)**: forgejo 侧语料曾被推入 GitHub public 语料仓 ~1h (暴露内网拓扑: 域名/网段/主机名/用户路径; 无凭据值泄露), 重写历史重建止损。铁律: **数据出境判定先于数据移动** — 任何样本/日志/配置进入新仓前, 先问 "源数据的信任域是什么, 目标仓的暴露面是什么"; 私有实例数据的目标仓必须在同一信任域 (内网) 或经用户明确批准。语料仓 `.gitignore` 的 `git.lambda.lc__*/` 排除是机械防线 (corpus-refresh 的 `git add -A` 不裹挟), 勿删。
- **脚本**: L0 家族 — `l0-collect.sh` (采集 SSOT, fail-fast, 输出含 conclusion 供三态口径; 语料根经 `CORPUS_DIR` 参数化) / `l0-summarize.jq` (统计口径 SSOT; FN 谓词只数 failure/timed_out, cancelled 不计缺失, spec §4) / `l0-gate.sh` (判定阈值 SSOT, 含语料在场断言防空转假绿 + baseline-empty 硬防线; 基线读语料仓内 `l0-baseline.json`, 管辖域 ci=github) / `l0-stats.sh` (人类报表, 含 rg 基线对照); 语料生命周期 `fetch-corpus.sh` (获取, 幂等 clone/pull) + `fetch-logs.sh` (GHA 抓取, 槽位 SSOT 默认 14F+5S) + `fetch-forgejo.sh` (内网现场) + `corpus-sync.sh` (判定器镜像单向同步 + dispatch 触发语料仓即时刷新; 已退役的 `corpus-refresh.sh` 内网全量刷新被其取代)。改口径只动前两者, 改阈值只动 gate。新增统计/判定脚本默认同栈 (bash + jq); 超过 ~50 行或出现复杂控制流时改为 engine 内嵌子命令 (Rust), 不再引入新语言。
- 环境: devShell 经 flake.nix (`nix develop`), 提供 rust 工具链 + just/jq/ripgrep/yq; cargo 国内镜像 SSOT 在 `.cargo/config.toml` (rsproxy)。
- **CI**: `.forgejo/workflows/ci-merge.yml` (org 标准 P0 契约; `CORPUS_DIR=/var/lib/forgejo-runner/cache/corpus` 语料持久缓存 — 幂等 clone 一次后增量 pull, 语料流量 ~10MB/run → ~1MB/周) + `ci-periodic.yml` (P2 夜档, 判定器镜像同步 + 内网语料补抓, 骨架源自 lc-studio/forgejo-actions 只改 ⟦必改⟧/⟦可选⟧ 字段); 契约 justfile `ci-merge`/`ci-periodic` recipe。**vm-nix runner 契约 (secret-guard 同模式)**: VM systemPackages 预装 rust 工具链, **禁止 `nix develop`/`nix build` 在 VM 内求值 flake** (ro store + temproots 无写权限, 必炸 — 首跑 run 19423 实证); 编译产物经 job 级 env `CARGO_TARGET_DIR=/var/lib/forgejo-runner/cache/cargo-target` 落宿主持久卷跨 job 复用。secrets: `FORGEJO_API_TOKEN` (读 lc-studio/nixos actions 的 PAT, forgejo 语料拉取 — 自动注入的 GITHUB_TOKEN 是本仓 scoped 读不了 nixos) + `CORPUS_GH_TOKEN` (GitHub PAT, 语料仓镜像推送 + engine-sync dispatch, 仅 ci-periodic 需要)。devShell (flake.nix) 仅服务本地开发。

## POC 交付物清单 (DoD, 已验收)

- [x] 样本: ≥15 失败 run + ≥15 成功 run (三仓合计), 含 sidecar meta
- [x] 规则: 四语料各自主要错误类型覆盖 (25 signal + 4 noise 内含本仓 9 条), 全部通过加载校验
- [x] 引擎: cargo test 15 用例全绿 (A0-A8 + intent/粒度/context), clippy clean
- [x] L0 报告: docs/poc-report.md (v2, 双语料 + 基线对照 + intent 分层)
