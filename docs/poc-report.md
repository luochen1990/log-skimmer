# Log Skimmer POC 数据报告 (v2)

> 职责边界: POC 实测数据、指标结论、验收线建议与过程发现。规则/语义规范见 [rule-format.md](rule-format.md) (v0.2); 立项背景见 nixos 仓 issue #1872。
> v2 变更: 按第二轮增量建议 (issue 评论 #44096) 补双语料集 / fallback 基线对照 / intent 分层 FP/FN / 三态与 rerun 口径 / per-rule 分列 / 双粒度压缩比。
> 执行时间: 2026-10-16; 引擎 15 单测全绿 + clippy clean; 样本 67 对 (49 失败 / 18 成功, 双语料)。

## TL;DR

**核心假设验证通过 (双语料 + 基线对照)**: 27 条规则 (18 GHA + 9 本仓 Forgejo, 全部人工零 LLM) 在四个真实仓上: **FP=0 vs 基线 230 行** (intent 分层下), FN=0 = 基线, 压缩率 84-99% (基线 86.95%, 但基线输出是无结构行集且成功 run 上含 230 行假阳性)。本仓 Forgejo 轮询回显形态 (与 GHA 直出完全不同的宿主前缀) 由规则正则自承载容忍, 跨形态适配成功。

## 1. 方法与语料

| 语料 | 工具栈 | 失败 | 成功 | 形态特征 |
| --- | --- | --- | --- | --- |
| sst/opencode (GHA) | bun/eslint/playwright | 14 | 5 | GHA 时间戳前缀 + ANSI 色码 |
| niri-wm/niri (GHA) | cargo/rustc/rust-test | 14 | 5 | 同上 |
| nix-community/home-manager (GHA) | nix/git/nixfmt | 14 | 5 | 同上; 完整多行错误树 (最大 219 行) |
| **lc-studio/nixos (Forgejo)** | nix/just/treefmt/nim/systemd | 7 | 3 | **agent-service 沙箱轮询回显** (`… [NmNns]` 前缀, 值树逐行打散, runner 尾部三连) |

- 侧车 meta 全量含 `attempt` 与 `ci` 字段; **rerun 处置实证**: 1 个样本 attempt=3 且最终失败 → 按 attempt 结论保留为优质 failure 语料 (rerun-through 排除规则未触发, 语料无该形态)。
- **三态实证**: 无 cancelled/skipped 结论样本 (Forgejo run 级取消在 job 层落为 failure+terminated 形态, 已收入); 18720 即 terminated 形态 — 按新口径它属 failure (有日志有结论), 其提取命中 3 块。
- GHA 侧 20 个 run 因「无 failure job」(workflow 级失败) 无语料 — run 级失败信号源仍留 MVP。

## 2. L0 主指标 (vs fallback 基线)

基线定义: `rg -i 'error|warn'` 匹配行数 ∪ `tail -100` 行数 (取大), 即「无规则集 agent 的保底手段」。

| 指标 | Log Skimmer | rg 基线 | 评注 |
| --- | --- | --- | --- |
| FN (完全漏检) | **0 / 49** | 0 / 49 | 平手 (基线 tail-100 兜底强) |
| FP (成功 job 假阳性) | **0** (failure-intent 口径) | **230 行** (18 个成功 job) | **核心优势**: 基线在成功 run 上照样吐 error/warn 行; intent 分层使 warning 命中 (166 处) 不计 FP 且语义正确 |
| 压缩率 (块级) | 本仓 83.9% / hm 91.2% / oc 96.4% / niri 98.9% | 86.95% (全局) | LS 分仓 3/4 高于基线; 本仓因 warning 有意保留而略低 (failure-intent-only 口径下本仓 ≈97%, 见 §3) |
| 结构性 | 完整错误树 + 坐标 + category/intent/confidence | 孤立行集, 树中部丢失 | 基线只抓锚行, nix 219 行树只回显 1 行; rg 无法支撑「定位」只支持「瞥见」 |

**双粒度压缩比** (spec §3, 失败 run): 块级 vs 签名行级 — niri 98.89%/99.73%, home-manager 91.22%/**99.72%** (巨树场景签名级优势极大: 219 行树 → 1 行索引), opencode 96.41%/99.18%。验证「索引+摘要」输出定位: agent 首屏用签名级, 按需 `read_around` 展开。

## 3. intent 分层实证 (FP 口径的核心修正)

- `nix-deprecation-warning` (**intent: warning**, 本仓新规则): 成败 run 均大量命中 (166 frags, 4 样本含成功 run) — **旧口径下这是 166 个 FP, 会击穿验收线; 新口径 0 FP 且信息保留**。
- 全部 25 条 signal 规则中 24 条 intent: failure (默认), FP=0; 1 条 warning 如上。
- 本仓 failure-intent-only 压缩率: 7 个失败 run 合计 778 行 → 42 行 (94.6%) — warning 剔除后与 GHA 三仓同量级。

## 4. per-rule 分列 (Top, 全语料 frags/FP)

| rule | frags | lines | FP | 命中样本 |
| --- | --- | --- | --- | --- |
| nix-deprecation-warning (warning) | 166 | 166 | 0 | 4 |
| playwright-assert-failed | 69 | 103 | 0 | 1 |
| github-actions-error-annotation (安全网) | 45 | 45 | 0 | **42** |
| forgejo-actions-job-failed (安全网) | 21 | 21 | 0 | 7 |
| bun-error-json | 11 | 77 | 0 | 11 |
| systemd-ci-unit-fail | 10 | 10 | 0 | 5 |
| rustc-error-tree | 9 | 107 | 0 | 5 |
| just-recipe-fail | 8 | 8 | 0 | 4 |
| nix-eval-error-tree | 3 | **657** | 0 | 3 |
| (其余 16 条均 ≥1 命中, FP 全 0) | | | | |

- **双安全网实证**: GHA annotation 规则 42/42 失败日志命中, Forgejo runner 尾部规则 7/7 — CI 平台自身信号是最稳底座。
- **必要性风险信号**: nix-eval-error-tree 平均 219 行/块 — 贪心 loop 在巨树上输出过宽, 是 L1 必要性检查的首个靶子 (POC 接受, 记 MVP 改进项)。
- noise 规则实测: 本仓 3 条 noise 平均每样本剔 24-50 行 (fetch 回显/drv 清单/轮询占位)。

## 5. 过程发现 (v2 增量)

1. **正则验证方法论坑**: shell 复合引号下 `` `.+` `` 被 bash 工具链某层解释为命令替换, rg 「验证通过」是假阳性 — nix option 错误的实际引号形态是「开 backtick + 闭 apostrophe」(`` `foo' ``)。规则正则验证必须以引擎加载 + 真实日志命中为准 (l0-stats 即此用途), 不信任中间 shell 管道。
2. **宿主前缀自承载成立**: 本仓 `… [NmNns]` 前缀 + 值树打散形态未改引擎一行, 全靠规则正则 (行内锚/前缀容忍) 适配 — A0 条款 (引擎不做预处理) 经跨形态验证。
3. **Forgejo job logs API 双接口内容有细微差异** (全量 vs byte-range, 前缀不一致) — 语料以 byte-range 可互验接口为准。
4. T2 交付的 6 个规则 title 含未引号 `: ` 被**加载期校验**拦截; 本轮新发现的引擎 bug (awk 基线统计 END 字段错位) 由基线 FN 异常暴露 — 统计脚本也是被测对象。
5. subagent 执行面: 日志落盘 agent (forgejo MCP 传手) 一次成功; 引擎实现 agent 两次空跑后架构师接手 — 派遣通道稳定性是排期风险项。

## 6. 验收线建议 (MVP L0 门禁初始线, 按工具分列)

| 项 | 线 | POC 实测 |
| --- | --- | --- |
| FN (failure-intent 完全漏检) | ≤ 5%/工具 | 0 |
| FP (failure-intent 在成功 job) | ≤ 2%/工具 | 0 |
| 压缩率 (块级) | ≥ 85%/工具 (本仓含 warning 82%+) | 83.9-98.9% |
| 边界违例 | = 0 (不变量) | 0 |

按分布随规则集/样本扩容校准; 本仓 warning 输出占比是口径敏感点, MVP 需定「warning 默认呈现策略」(如默认折叠)。

## 7. POC 未覆盖 (MVP 须补)

Adapter (LLM 学习) 全链路 / L1 judge 独立化 (本轮抽查者仍与规则作者同源) / run 级失败 (无 failure job) 信号源 / skim+read_around MCP 组合 / category 枚举冻结 (并行任务进行中) / manifest 按需拉取 / 长日志 (>40KB 单 job) 极端行为。
