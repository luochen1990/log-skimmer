# Log Skimmer POC 数据报告 (v2)

> 职责边界: POC 实测数据、指标结论、验收线建议与过程发现。规则/语义规范见 [rule-format.md](rule-format.md) (v0.2)。
> v2 变更: 按第二轮增量建议补双语料集 / fallback 基线对照 / intent 分层 FP/FN / 三态与 rerun 口径 / per-rule 分列 / 双粒度压缩比。
> 执行时间: 2026-10-16; 引擎 15 单测全绿 + clippy clean; 样本 67 对 (49 失败 / 18 成功, 双语料)。

## TL;DR

**核心假设验证通过 (双语料 + 基线对照)**: 28 条规则 (18 GHA + 10 私有语料源 Forgejo, 全部人工零 LLM) 在四个真实 CI 环境上: **FP 率 0% vs 基线 100%** (18 个成功 job 基线全部产出假阳性, 均值 12.8 行/job; intent 三分口径), FN=0 = 基线, 压缩率 83-99% (结果性指标, 不做门槛)。私有源 Forgejo 轮询回显形态 (与 GHA 直出完全不同的宿主前缀) 由规则正则自承载容忍, 跨形态适配成功。

## 1. 方法与语料

| 语料 | 工具栈 | 失败 | 成功 | 形态特征 |
| --- | --- | --- | --- | --- |
| sst/opencode (GHA) | bun/eslint/playwright | 14 | 5 | GHA 时间戳前缀 + ANSI 色码 |
| niri-wm/niri (GHA) | cargo/rustc/rust-test | 14 | 5 | 同上 |
| nix-community/home-manager (GHA) | nix/git/nixfmt | 14 | 5 | 同上; 完整多行错误树 (最大 219 行) |
| **私有语料源 (Forgejo Actions)** | nix/just/treefmt/nim/systemd | 7 | 3 | **沙箱轮询回显** (`… [NmNns]` 前缀, 值树逐行打散, runner 尾部三连) |

- 侧车 meta 全量含 `attempt` 与 `ci` 字段; **rerun 处置实证**: 1 个样本 attempt=3 且最终失败 → 按 attempt 结论保留为优质 failure 语料 (rerun-through 排除规则未触发, 语料无该形态)。
- **三态实证**: 无 cancelled/skipped 结论样本 (Forgejo run 级取消在 job 层落为 failure+terminated 形态, 已收入); terminated 形态样本按新口径属 failure (有日志有结论), 其提取命中 3 块。
- GHA 侧 20 个 run 因「无 failure job」(workflow 级失败) 无语料 — run 级失败信号源仍留 MVP。

## 2. L0 主指标 (vs fallback 基线)

基线定义: `rg -i 'error|warn'` 匹配行数 ∪ `tail -100` 行数 (取大), 即「无规则集 agent 的保底手段」。

| 指标 | Log Skimmer | rg 基线 | 评注 |
| --- | --- | --- | --- |
| FN (完全漏检) | **0 / 49** | 0 / 49 | 平手 (基线 tail-100 兜底强) |
| FP (成功 job 假阳性) | **0 行 / 18 job (FP 率 0%)** | **230 行 / 18 job (FP 率 100%, 均值 12.8 行/job)** | **核心优势**: 基线在每个成功 job 上都吐假阳性"错误"行; intent 分层使 warning/info 命中不计 FP 且语义正确 |
| 压缩率 (块级) | 私有源 83.9% / hm 91.2% / oc 96.4% / niri 98.9% | 86.95% (全局) | LS 分仓 3/4 高于基线; 私有源因 warning 有意保留而略低 (failure-intent-only 口径下私有源 ≈97%, 见 §3) |
| 结构性 | 完整错误树 + 坐标 + category/intent/confidence | 孤立行集, 树中部丢失 | 基线只抓锚行, nix 219 行树只回显 1 行; rg 无法支撑「定位」只支持「瞥见」 |

**双粒度压缩比** (spec §3, 失败 run): 块级 vs 签名行级 — niri 98.89%/99.73%, home-manager 91.22%/**99.72%** (巨树场景签名级优势极大: 219 行树 → 1 行索引), opencode 96.41%/99.18%。验证「索引+摘要」输出定位: agent 首屏用签名级, 按需 `read_around` 展开。

## 3. intent 三分实证 (FP 口径的核心修正)

`intent` 三分 (v0.3, 需求方反馈): `failure` (失败证据, 唯一 FP 统计对象) / `warning` (**有问题倾向**但非失败, deprecation 类) / `info` (**纯环境快照**, 无问题倾向 — 版本行等; 排除环境因素时按需取用)。warning 与 info 的区分是「问题倾向」: 前者可能提示根因 (诊断高优先), 后者是环境事实。

- `nix-deprecation-warning` (**warning**): 成败 run 均大量命中 (166 frags, 4 样本含成功 run) — 旧口径下这是 166 个 FP, 会击穿验收线; 新口径 0 FP 且信息保留。
- `forgejo-actions-runner-version-info` (**info**, runner 版本行): 10 frags / 7 样本 — 版本信息类「有价值但与错误无关」的内容有了正确归位, 不再挤占 failure 语义。
- 全部 26 条 signal 规则: 24 failure (默认, FP=0) + 1 warning + 1 info。
- 私有源 failure-intent-only 压缩率: 7 个失败 run 合计 778 行 → 42 行 (94.6%) — warning/info 剔除后与 GHA 三仓同量级。

## 4. per-rule 分列 (Top, 全语料 frags/FP)

| rule | frags | lines | FP | 命中样本 |
| --- | --- | --- | --- | --- |
| nix-deprecation-warning (warning) | 166 | 166 | 0 | 4 |
| playwright-assert-failed | 69 | 103 | 0 | 1 |
| github-actions-error-annotation (安全网) | 45 | 45 | 0 | **42** |
| forgejo-actions-job-failed (安全网) | 21 | 21 | 0 | 7 |
| bun-error-json | 11 | 77 | 0 | 11 |
| systemd-ci-unit-fail | 10 | 10 | 0 | 5 |
| forgejo-actions-runner-version-info (info) | 10 | 10 | 0 | 7 |
| rustc-error-tree | 9 | 107 | 0 | 5 |
| just-recipe-fail | 8 | 8 | 0 | 4 |
| nix-eval-error-tree | 3 | **657** | 0 | 3 |
| (其余 16 条均 ≥1 命中, FP 全 0) | | | | |

- **双安全网实证**: GHA annotation 规则 42/42 失败日志命中, Forgejo runner 尾部规则 7/7 — CI 平台自身信号是最稳底座。
- **必要性风险信号**: nix-eval-error-tree 平均 219 行/块 — 贪心 loop 在巨树上输出过宽, 是 L1 必要性检查的首个靶子 (POC 接受, 记 MVP 改进项)。
- noise 规则实测: 私有源 3 条 noise 平均每样本剔 24-50 行 (fetch 回显/drv 清单/轮询占位)。

## 5. 过程发现 (v2 增量)

1. **正则验证方法论坑**: shell 复合引号下 `` `.+` `` 被 bash 工具链某层解释为命令替换, rg 「验证通过」是假阳性 — nix option 错误的实际引号形态是「开 backtick + 闭 apostrophe」(`` `foo' ``)。规则正则验证必须以引擎加载 + 真实日志命中为准 (l0-stats 即此用途), 不信任中间 shell 管道。
2. **宿主前缀自承载成立**: 本仓 `… [NmNns]` 前缀 + 值树打散形态未改引擎一行, 全靠规则正则 (行内锚/前缀容忍) 适配 — A0 条款 (引擎不做预处理) 经跨形态验证。
3. **Forgejo job logs API 双接口内容有细微差异** (全量 vs byte-range, 前缀不一致) — 语料以 byte-range 可互验接口为准。
4. T2 交付的 6 个规则 title 含未引号 `: ` 被**加载期校验**拦截; 本轮新发现的引擎 bug (awk 基线统计 END 字段错位) 由基线 FN 异常暴露 — 统计脚本也是被测对象。

## 6. 验收线建议 (MVP L0 门禁初始线, 按工具分列; v4 修订: FN 降为 0 容忍不变量)

**错误代价不对称性** (需求方决策): FP 的代价是多几行噪音; **FN 的代价是 agent 误判成败 (失败当成功) 或失去定位能力被迫回退全量日志 — 工具失去存在意义**。故 FN 不设容忍线、直为不变量; 宁可规则覆盖过头牺牲压缩率, 不可漏 (spec §0 召回优先条款)。

| 项 | 线 | 性质 | POC 实测 |
| --- | --- | --- | --- |
| FN (failure-intent 完全漏检) | **= 0, 不变量** | 漏检零容忍 (召回优先) | 0 |
| FP (failure-intent 在成功 job) | ≤ 2%/工具 | 质量门槛 (容忍) | 0 |
| 边界违例 | = 0 | 不变量 | 0 |
| 压缩率 | > 0 (有得压即可, 能压 10 个点也是收益) | 弱底线, 防零收益 | 83.2-98.9% |
| 压缩率回归 | 同语料集上规则集变更不得显著下降 | 防退化哨兵 (CI 内对比) | — |

**FN=0 的三重结构配套** (承诺不能只写在门槛上): ① **双侧平台安全网恒居规则集底座** — 失败侧 (GHA annotation 42/42 + Forgejo runner 失败尾部 7/7) 兜住失败日志, 成功侧 (runner 版本 info 规则: GHA `Current runner version` 首行 + Forgejo `Runner ... (version:...)` 首行, 67/67 样本命中) 兜住成功日志 — 「成功日志至少一条 info」由此成为规则集**义务**而非建议; ② **零块警示无条件** — 双侧安全网使「任何真实 CI 日志 skim 非零」成为结构承诺 (POC 实测 67/67), 零块 = 规则集失明, 与 job 成败无关 → exit 3 + 无条件回退 rg 基线, **判定完全内生于规则集, 不需要调用方传入任何预期元数据** (expect 类标志是过度设计, 已删); ③ 新规则/规则变更在语料集上 CI 回归 (L0 门禁本体)。

压缩率作为**报告指标**持续跟踪 (供 L1 必要性检查与呈现策略决策), 不做绝对门槛; warning/info 呈现策略 (折叠/过滤) 是 MVP 定稿项, 它决定消费端看到的实际体量。

## 7. POC 未覆盖 (MVP 须补)

Adapter (LLM 学习) 全链路 / L1 judge 独立化 (本轮抽查者仍与规则作者同源) / run 级失败 (无 failure job) 信号源 / skim+read_around MCP 组合 / category 枚举冻结 (并行任务进行中) / manifest 按需拉取 / 长日志 (>40KB 单 job) 极端行为。
