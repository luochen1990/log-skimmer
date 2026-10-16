# Log Skimmer 维护者文档 (POC)

> 职责边界: 给维护者与 agent 的实现约定。规则格式/状态机语义的 SSOT 是 docs/rule-format.md, 本文不重复, 只写工程纪律。

## 工程纪律

- **契约优先**: 引擎实现与规则编写的一切语义争议以 `docs/rule-format.md` 为准; 发现规范未覆盖的语义 → 先补规范再写代码/规则。
- **引擎核心是纯函数**: `skim(rules, lines) -> fragments`, 不做 IO/网络/时间依赖 — 未来 MCP 层在纯核心外包装。CLI 只是薄壳。
- **依赖最小**: POC 限 serde/serde_yaml/regex/clap 四类, 新依赖须给理由。
- **测试**: 状态机公理 A1-A8 每条 ≥1 单测; 规则集加载回归 (`engine/tests/rules_load.rs`) 防语法漂移; 规则/引擎变更跑 `just ci-merge` (含 L0 全语料门禁: FN=0 / FP 率 ≤2% / 零块=0 / 压缩率基线防退化, 判定实现 `scripts/l0-gate.sh`)。
- **语料已入仓 (M1 起)**: `data/logs/` 67 对样本是 L0 门禁回归 fixture, 必须可复现 (POC 期不入仓决策已演进); 抓取残留 `fetch.log` 仍排除; 规则头注释记 `# samples:` 溯源。
- **脚本**: L0 家族三件套 — `l0-collect.sh` (采集 SSOT, fail-fast) / `l0-summarize.jq` (统计口径 SSOT) / `l0-gate.sh` (判定阈值 SSOT); `l0-stats.sh` 是消费采集层的人类报表。改口径只动前两者, 改阈值只动 gate。
- **脚本语言不限** (bash/rust 均可); MVP 起再立语言规范。
- 环境: devShell 经 flake.nix (`nix develop`), 提供 rust 工具链 + just/jq/ripgrep/yq; cargo 国内镜像 SSOT 在 `.cargo/config.toml` (rsproxy)。
- **CI**: `.forgejo/workflows/ci-merge.yml` (org 标准 P0 契约, 骨架源自 lc-studio/forgejo-actions, 只改 ⟦必改⟧/⟦可选⟧ 字段); 契约实现 justfile `ci-merge` recipe (外层 nix develop 进 devShell, CI runner 无系统 cargo)。

## POC 交付物清单 (DoD)

- [x] 样本: ≥15 失败 run + ≥15 成功 run (三仓合计), 含 sidecar meta
- [x] 规则: 四语料各自主要错误类型覆盖 (25 signal + 4 noise 内含本仓 9 条), 全部通过加载校验
- [x] 引擎: cargo test 15 用例全绿 (A0-A8 + intent/粒度/context), clippy clean
- [x] L0 报告: docs/poc-report.md (v2, 双语料 + 基线对照 + intent 分层)
