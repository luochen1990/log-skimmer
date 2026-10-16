# Log Skimmer 维护者文档 (POC)

> 职责边界: 给维护者与 agent 的实现约定。规则格式/状态机语义的 SSOT 是 docs/rule-format.md, 本文不重复, 只写工程纪律。

## 工程纪律

- **契约优先**: 引擎实现与规则编写的一切语义争议以 `docs/rule-format.md` 为准; 发现规范未覆盖的语义 → 先补规范再写代码/规则。
- **引擎核心是纯函数**: `skim(rules, lines) -> fragments`, 不做 IO/网络/时间依赖 — 未来 MCP 层在纯核心外包装。CLI 只是薄壳。
- **依赖最小**: POC 限 serde/serde_yaml/regex/clap 四类, 新依赖须给理由。
- **测试**: 状态机公理 A1-A8 每条 ≥1 单测; 规则文件变更跑 `cargo test` + 对 data/logs/ 全样本回归 (本地)。
- **样本不入 git** (`data/` gitignore): 原始日志体积大且含他人内容; 规则头注释记 `# samples:` 溯源。
- **POC 脚本语言不限** (bash/rust 均可); MVP 起再立语言规范。
- 环境: 本机为 NixOS; 若无系统 cargo, 用 `nix-shell -p rustc cargo`。POC 阶段不上 flake/justfile, 建仓转正式仓时补。

## POC 交付物清单 (DoD)

- [x] 样本: ≥15 失败 run + ≥15 成功 run (三仓合计), 含 sidecar meta
- [x] 规则: 四语料各自主要错误类型覆盖 (25 signal + 4 noise 内含本仓 9 条), 全部通过加载校验
- [x] 引擎: cargo test 15 用例全绿 (A0-A8 + intent/粒度/context), clippy clean
- [x] L0 报告: docs/poc-report.md (v2, 双语料 + 基线对照 + intent 分层)
