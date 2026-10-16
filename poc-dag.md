# POC 执行 DAG (过程工件, 不入正式仓)

状态: pending | running | done | blocked

```
T1 spec+骨架文档 (架构师)                    [done]  spec v0.1 (含 A0 行语义, 吸收 T2 反馈)
├─→ T2 抓样本+分析+手写规则 (agent A)         [done]  57 对样本 (42F/15S), 18 规则 (16 signal+2 noise)
├─→ T3 Rust 引擎骨架 (架构师亲自)             [done]  subagent 两次空跑降级; 12 单测全绿+clippy clean;
│                                                    A6 时序 bug (noise 顺序依赖) 被公理单测捕获
└─→ T4 联调+L0 统计+数据报告 (架构师)          [done]  压缩 91-99% / FN=0 / FP=0 / 边界违例=0;
                                                     充分性抽查 3/3; docs/poc-report.md 成稿
     └─→ T5 建仓+推送                        [blocked]  等待用户建 lc-studio/log-skimmer 或 token 提权
```

本地仓: /tmp/opencode/log-skimmer @ 61ba5e7 (33 files, 单提交)。
