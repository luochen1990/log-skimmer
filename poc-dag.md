# POC 执行 DAG (过程工件, 不入正式仓)

状态: pending | running | done | blocked

```
T1 spec+骨架文档 (架构师)                    [done]
├─→ T2 抓样本+分析+手写规则 (agent A)         [pending]   依赖: T1 (spec 是规则契约)
├─→ T3 Rust 引擎骨架 (agent B)               [pending]   依赖: T1 (spec 是实现契约)
│    T2 ∥ T3 (无相互依赖)
└─→ T4 联调+L0 统计+数据报告 (架构师/agent)   [pending]   依赖: T2 ∧ T3
     └─→ T5 建仓+推送                        [blocked]   依赖: 用户权限 + T4
```

- T4 验收: 跑通全样本, 三仓各出压缩率/FP/FN, 机械边界检查零违例 (或违例有解释), docs/poc-report.md 成稿。
- T2 交付: scripts/fetch-logs.sh (幂等) + data/logs/ 样本 + rules/ ≥6 条。
- T3 交付: engine/ cargo 项目, A1-A8 单测全绿, CLI skim 子命令。
