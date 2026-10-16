# Log Skimmer POC 数据报告

> 职责边界: POC 实测数据、指标结论、验收线建议与过程发现。规则/语义规范见 [rule-format.md](rule-format.md); 立项背景见 nixos 仓 issue #1872 评估报告。
> 执行时间: 2026-10-16; 引擎 `engine/` @ 12 单测全绿 + clippy clean; 样本 57 对 (42 失败 / 15 成功)。

## TL;DR

**核心假设验证通过**: 人工手写规则 + 逐行正则状态机, 在三个真实开源仓的 CI 日志上达到 **91-99% 压缩、FN(完全漏检)=0、FP=0、边界违例=0**, 与 Red Hat PFA (95-98%) 同量级。纯确定性提取路线成立, 建议进入 MVP。

## 1. 方法

- **样本**: GHA 三仓各 14 失败 + 5 成功 run 的失败/首 job 日志 (sst/opencode TS · niri-wm/niri Rust · nix-community/home-manager nix), 共 57 对, 采集自 2026-06~10 的真实 run; sidecar meta 见 `data/logs/`。
- **规则**: 18 条 (16 signal + 2 noise), 人工分析归纳 (全程零 LLM — 验证「规则可人工产出」基线), 覆盖 bun/eslint/playwright/cargo/rustc/rust-test/nix/nixfmt/git + 跨仓安全网 github-actions-error-annotation。
- **引擎**: Rust, 规则加载 (spec §1 全量校验) + 逐行状态机 (A1-A8) + fragment 聚合 + L0 统计; 核心纯函数; A1-A8 每公理 ≥1 单测。

## 2. L0 指标 (全量 57 样本)

| 仓 | 失败 run | 失败 run 平均压缩率 | FN (完全漏检) | 成功 run | FP (成功 run 提取块) |
| --- | --- | --- | --- | --- | --- |
| sst/opencode | 14 | 96.41% | 0 | 5 | 0 |
| niri-wm/niri | 14 | 98.89% | 0 | 5 | 0 |
| nix-community/home-manager | 14 | 91.26% | 0 | 5 | 0 |
| **合计/加权** | **42** | **~95.5%** | **0/42** | **15** | **0/15** |

- 边界完整性机械检查: **0 违例** (哨兵按构造恒真, 同时验证状态机实现自洽)。
- 提取块绝对规模 (agent 消费视角): 典型失败 run 输出 5-40 行 (输入 200-30k 行)。
- noise 规则实测: cargo 编译进度 1949 行 / playwright 进度刷新 682 行被有效剔除。

## 3. 充分性抽查 (LLM 对照雏形, 3/42 抽样)

每仓抽 1 失败 run, 人工对照「提取块 vs 完整日志」判断提取块是否足以定位失败:

1. **niri** (cargo 版本冲突): 完整 11 行错误树 (依赖链 + feature 冲突 + help) — **充分**。
2. **home-manager** (label 配置错误): 单行 `##[error]" merge conflict" label not found` — **充分** (GHA annotation 是官方结构化信号, 安全网规则的价值实证)。
3. **opencode** (服务端 500): `Error: {` JSON 树 (name/data/ref) + exit code annotation — **充分**。

抽查 3/3 充分。**局限**: 抽样率低 (7%), 且抽查者即规则作者 (同源偏差); MVP 的 L1 judge 须独立化此对照。

## 4. 过程发现

**实证有效的决策**:

- `github-actions-error-annotation` 跨仓安全网命中 42/42 失败日志 — CI 平台自身 annotation 是最稳定的信号源, 该规则应作为规则集默认底座。
- 宿主前缀容忍 (GHA 时间戳 `...Z ` + ANSI 色码) 必须由规则正则承载 (POC 共识) — 每条规则锚带 `^(timestamp )?` 可选前缀; 色码两处怪形态 (help 冒号前色码/行号填充在色码内) 已在规则中适配。
- 20/62 run 因「无 failure job」(workflow 级失败/依赖取消) 不产生 job 语料 — run 级失败是独立信号源, MVP 的 run 元数据消费需覆盖。

**spec v0 → v0.1 修订输入** (来自实测, 已吸收或待吸收):

| 反馈 | 处置 |
| --- | --- |
| CRLF 行语义未定义 | 吸收: spec §2 增补「行消费 = 剥 `\r` 后的字符串」 |
| 宿主前缀 (时间戳/ANSI) 未提及 | 吸收: spec 注明前缀容忍由规则承载 (引擎不做预处理层) |
| A3 索引表述歧义 | 吸收: 改为指针模型伪码表述 |
| noise 统计口径 (noise_lines Removed 含块外行) | 待 v0.1: 明确 noise 行集可含非 signal 区行 |
| samples 头注释格式未定义 | POC 不强制, MVP 规则贡献指南再定 |

**工程发现**: T2 交付的 6 个规则文件 `title:` 含未引号包裹的 `: ` 导致 YAML 解析失败 — 被引擎加载校验 (而非运行期) 拦截, 加载期校验的设计价值实证; subagent 两次空跑后架构师接手实现 (T3), 状态机时序 bug (noise 剔除依赖规则顺序) 由 A6 单测捕获。

## 5. 验收线建议 (供 MVP 参照, 非硬门槛)

基于本数据 (n=57, 三仓), 建议 MVP CI 门禁 (L0) 初始线:

- 失败 run 完全漏检率 (FN 代理) ≤ 5% (POC 实测 0)
- 成功 run FP 块率 ≤ 2% (POC 实测 0)
- 失败 run 压缩率 ≥ 90% (POC 实测 91.3-98.9%)
- 边界违例 = 0 (不变量, 非阈值)

随规则集与样本扩容, 按分布校准。

## 6. POC 未覆盖 (MVP 须补)

- Adapter (LLM 学习) 全链路 — POC 规则人工产出, 未验证「LLM 产出等质量规则」;
- L1 judge 独立化 (本 POC 抽查者与规则作者同源);
- run 级失败 (无 failure job) 的信号源;
- Forgejo (git.lambda.lc) adapter — POC 样本全部 GHA;
- 长日志 (>30k 行) 与超长错误树 (实测最大单块 219 行) 的极端行为。
