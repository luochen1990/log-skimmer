# Log Skimmer (POC)

> CI 日志高信噪比提取工具 — 为 AI agent 提供确定性的「学习型规则集」日志提取。
> 立项评估见 nixos 仓 issue #1872; 本仓处于 **POC 阶段**。

## 动机

Agent 消费 CI 日志时, 全量日志爆上下文且信噪比极低 (github-mcp-server #608 官方确认未解决); grep 单行正则抓不到多行错误树。Log Skimmer 用「LLM 离线学习规则集 + 运行期纯正则逐行状态机提取」同时解决成本、可复现、可审计。

## POC 范围 (仅此四件事)

1. 从 GHA 真实仓 (sst/opencode · niri-wm/niri · nix-community/home-manager) 抓取成败 run 的 job 日志样本;
2. 人工分析失败模式, 按规范手写规则 (不用 LLM — 验证「规则可人工产出」这条基线);
3. Rust 引擎: 加载规则 + 逐行状态机扫描 + fragment 输出 + L0 统计;
4. 数据报告: 压缩率 / FP / FN / 边界完整性, 据此定 MVP 验收线。

**不做**: MCP / Adapter (LLM 学习) / 按需拉取 / WebPage / CI 集成。

## 目录

```
docs/rule-format.md   # 规则格式与状态机规范 (SSOT, 必读)
rules/<tool>/         # 规则 (一文件一规则)
data/logs/            # 日志样本 (gitignore, 本地回测)
engine/               # Rust 引擎 (POC: CLI)
scripts/              # 抓取脚本
```

## 快速开始

```bash
scripts/fetch-logs.sh            # 抓样本 (需 GITHUB_API_TOKEN)
cargo run --release -- engine/ -- skim <log-file> --rules rules/   # 提取
```
