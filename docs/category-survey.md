# Log Skimmer `category` 枚举调研报告

- 调研主体: lc-studio/nixos issue #1872 (内网仓, Log Skimmer 立项)
- 调研问题: 规则元数据 `category` 封闭枚举应采用「pipeline 质量门类型」「失败根因类别」还是精选混合; 初始草案 (`fmt / lint / typecheck / security / proptest / unittest / uitest / test` — issue 原文称 7 类但列有 8 个 token, `test` 总类与三个测试变体并列, 本报告照录) 全是质量门, 是否遗漏 infra 类
- 日期: 2026-10-16 | 方法: oss-research 5 阶段 pipeline, 8 方向证据源全覆盖 (≥6 要求达成)

---

## 1. TL;DR

**推荐采用「两域精选混合」共 10 值**:

```
代码质量门域:  fmt | lint | typecheck | build | test | security
环境根因域:    deps | infra | config
兜底:          unknown
```

一句话理由: 学术数据显示 **22%–55% 的 CI 失败与代码质量门无关** (下界: Rausch 2017 质量门三类仅占错误 62% → 逆命题 38%, Hassan 口径 22%; 上界: Ghaleb 三类「噪声损坏」合计 ~55%), 而商业产品 (Datadog/Develocity) 均以「代码内因 vs 环境外因」为顶层二分 — 纯质量门集合会让 agent 失去「只看 infra 类失败」的查询能力, 直接削弱 Log Skimmer 最有价值的场景 (失败根因定位); 同时 10 值规模处于先例经验带 (semgrep 6 / CodeQL 4 / Datadog 3 domain 共 14 subdomain / Rausch 14 / Develocity 2)。

**Plan B**: 若 POC 期发现贡献者打标负担超标, 先合并 `fmt`+`lint`→`lint`、`typecheck`→`build` 收缩到 8 值, 而不是砍掉环境根因域 (环境域是数据证实的最大失败来源, 不可牺牲)。

---

## 2. 消费场景反推的 category 设计约束

从两个已声明的消费场景 + 架构事实反推:

| # | 约束 | 推导 |
| - | ---- | ---- |
| C1 | **查询语义稳定**: 值可追加、不可重命名/改义 | agent 按 category 过滤是程序化消费 (如「只看 typecheck 类」), 语义漂移 = 静默错误结果, 比缺类更危险 |
| C2 | **规模可浏览**: 10±5 值 | WebPage 按类别归组的导航场景; semgrep 6 / CodeQL 4 / Datadog 14 subdomain 是同类先例的收敛带 |
| C3 | **与 intent 正交** (已定稿 failure\|warning) | category 答「这是什么类别的信息」, intent 答「命中对成败判定意味着什么」; security 告警常是 warning, fmt 门禁失败常是 failure — 任何把成败含义编码进 category 的设计都破坏正交 |
| C4 | **与 tool 正交** (folksonomy 自由命名) | 生态长尾差异 (eslint vs golangci-lint) 由 tool 吸收, category 只吸收「信息类别」差异; 判定应尽量「按工具在 CI 中的角色」而非「按单行日志语义」, 一个工具的规则族通常共享 category |
| C5 | **正则可判定性** | Reader 是纯 Rust regex 逐行状态机, 规则作者必须能从样例行直接判定 category; 两类日志形态在文本上不可分时 (如网络超时 vs 凭据失败), 不得拆成两个 category |
| C6 | **集合稳定性** | 新工具/新包管理器涌现的高频事件不应触发扩集 (新 linter → `lint` 即可); 需要「新失败形态」才扩集的信号是 unknown 占比持续偏高 |
| C7 | **flaky 不进 category** | 时过时挂是「属性」不是「信息类别」; Mozilla Treeherder 用独立 failure_classification 维度承载 (见 §3.7), 与 intent/category 双轴正交的先例一致 |

---

## 3. 先例证据盘点

### 覆盖表 (本节 + §4 数据源汇总)

| 方向 | 来源 | 查到什么 | 关键链接 |
| ---- | ---- | -------- | -------- |
| 1. GHA problem matchers | actions/toolkit 官方文档 + GitHub topic (23 仓) + 默认 matcher 讨论 | 机制无任何分类学; matcher 全部面向质量门工具输出 | [toolkit docs](https://github.com/actions/toolkit/blob/main/docs/problem-matchers.md), [topic](https://github.com/topics/problem-matcher) |
| 2. reviewdog | README 全文 + errorformat 生态 | 按编程语言分组, 不按质量门类型分组 | [reviewdog](https://github.com/reviewdog/reviewdog) |
| 3. awesome-static-analysis | 仓库 (已迁址) TOC 结构 | 主轴 = 语言; "Other" 段有 Security/Build/CI/Config 等横切组 | [analysis-tools-dev/static-analysis](https://github.com/analysis-tools-dev/static-analysis) |
| 4a. semgrep registry | 官方贡献文档 | category 封闭 6 值 + namespace 结构 + security 附加元数据 | [docs.semgrep.dev](https://docs.semgrep.dev/contributing/contributing-to-semgrep-rules-repository) |
| 4b. CodeQL | 官方 metadata 文档 | @tags 4 值 + severity/precision/kind 正交维度 | [codeql docs](https://codeql.github.com/docs/writing-codeql-queries/metadata-for-codeql-queries/) |
| 5. 学术研究 | 5 篇 PDF 原文提取 (+3 项二手转引: Seo/Kerzazi 经 Hassan 论文, Tahir 经检索摘要) | 根因分布数据 + 两套分类学 (见 §4) | 见 §4 表 |
| 6. 商业产品 | Develocity/Datadog 官方文档, BuildPulse/Trunk 检索 | verification/non-verification 二分; LLM 分类 domain/subdomain | [Develocity](https://docs.develocity.ai/2026.1/guides/failure-classification-guide/), [Datadog](https://docs.datadoghq.com/continuous_integration/guides/use_ci_jobs_failure_analysis) |
| 7. Treeherder | Firefox 官方文档 | job 前缀聚类 + failure_classification 独立维度; 3–10% job 失败且多为 intermittent | [firefox-source-docs](https://firefox-source-docs.mozilla.org/testing/treeherder-try/index.html) |
| 8. 自举语料 | 本仓 justfile + .forgejo/workflows + AGENTS.md | 14 种日志信息类别清单 (见 §3.8) | 本地 |

盲区声明: BuildPulse/Trunk 未深挖官方文档 (定位明确: flaky 检测/隔离, 无公开的失败分类学可引); reviewdog errorformat 具体工具数 (宣传 100+) 未逐一清点; Vassallo 部分分布数字因 PDF 布局未完全对齐 (已标注); Tahir flaky 13% 数字转引自检索摘要, 原文未核实。

### 3.1 GitHub Actions problem matchers

机制: `::add-matcher::<json>` 注册 (owner + regex pattern + file/line/severity 映射), 借自 VS Code problem matcher 格式 ([官方文档](https://github.com/actions/toolkit/blob/main/docs/problem-matchers.md))。官方示例 matcher 由 setup-node/setup-python/setup-go/setup-dotnet 等工具链 action 内嵌注册; [topic 下 23 个独立仓库](https://github.com/topics/problem-matcher) 覆盖 rustc/stylelint/markdownlint/xmllint/phplint/gcc/htmllint 等 — **全部是质量门工具的输出解析, 无一涉及 infra/依赖/网络类失败模式**。元数据面只有 `owner`(=工具名) 和 severity(error/warning), **没有类别概念**。

启示:
- matcher 生态的定位是「渲染」(把日志变成 annotation 展示), 不是「查询」; Log Skimmer 的 category 填补的正是这一语义层空白。
- 若未来从 matcher JSON 自动导入规则, category 必须另行补标 (matcher 元数据里没有), 导入器应默认 `unknown`。

### 3.2 reviewdog

组织方式 (来源: [reviewdog README](https://github.com/reviewdog/reviewdog/blob/master/README.md)): (a) `reviewdog -list` 的预定义 errorformat 按**工具名平铺** (生态在 [reviewdog/errorformat](https://github.com/reviewdog/errorformat) 仓库); (b) 官方 GitHub Actions 按**编程语言/领域分组** (Go/JavaScript/TypeScript/CSS/Ruby/Python/Terraform/YAML/Docker/Shell/...), 组内 formatter (prettier/black/shfmt)、type checker (tsc/mypy/pyright/steep)、security (brakeman/bandit/bundler-audit) 混排, **不做质量门类型区分**。

启示: reviewdog 单一消费场景 (把诊断变成 PR 评论) 不需要查询轴, 语言分组够用; Log Skimmer 的 agent 过滤场景是 reviewdog 没有的需求, 不能照抄其「不分类」结论。其 errorformat 库 (vim errorformat 语法的工具输出解析规则集) 是与 Log Skimmer 规则集最同构的社区资产, 规则冷启动可参考其工具覆盖清单。

### 3.3 awesome-static-analysis

注意: 仓库已从 analysis-tools/dev 迁到 **analysis-tools-dev/static-analysis** (14.8k stars, 网站 [analysis-tools.dev](https://analysis-tools.dev); 本节证据为 [仓库 TOC](https://github.com/analysis-tools-dev/static-analysis))。TOC: 「Programming Languages」(40+ 语言小节) + 「Multiple Languages」+ 「Other」; "Other" 段包含 Security/SAST、Build tools、Continuous Integration、Config Files、Containers、Markdown 等横切组。语言小节内 linter/formatter/security 工具混排。

启示: 目录主轴选「语言」是因为它的消费者是人 (按自己技术栈找工具); 但其 "Other" 段被迫长出的横切组 (Security/Build/CI/Config) 说明「信息类别」轴在语言轴覆盖不了的场景中自然涌现 — 支持跨语言查询的 category 设计。

### 3.4 semgrep registry 与 CodeQL

**semgrep** (来源: [贡献文档](https://docs.semgrep.dev/contributing/contributing-to-semgrep-rules-repository)): registry 规则必填 `metadata.category`, 封闭 6 值: `security / correctness / best-practice / maintainability / performance / portability`; namespace 格式 `<language>/<framework>/<category>/$MORE`; `category: security` 附加必填 cwe/owasp/confidence/likelihood/impact/subcategory(vuln|audit|secure default)/vulnerability_class; severity (LOW–CRITICAL) 是独立维度。
**CodeQL** (来源: [metadata 文档](https://codeql.github.com/docs/writing-codeql-queries/metadata-for-codeql-queries/)): `@tags` 官方 4 值 `correctness / maintainability / readability / security` (+少量扩展); 正交维度 `@kind`(problem/path-problem)、`@problem.severity`(error/warning/recommendation)、`@security-severity`(security 专有)、`@precision`(置信度)。

启示:
- 两个最成功的「社区规则集」都把 category 做成**封闭枚举 + 独立于 severity** — 与 Log Skimmer 的 category(封闭) × intent(成败) 设计同构, 直接印证 C1/C3。
- security 作为一等类别在两家均存在, 且都配套附加元数据 — Log Skimmer 的 security 类规则可预留附加字段 (如 CVE 引用), 但不必现在做。
- 二者都是「检查目的」分类 (给静态分析规则用), 没有 build/test/infra — 因为它们只覆盖代码检查场景; Log Skimmer 场景更宽 (整个 CI 日志), 需要在其上扩质量门执行类与根因类。

### 3.5 商业 CI 分析产品

**Gradle Develocity** (来源: [Failure Classification Guide](https://docs.develocity.ai/2026.1/guides/failure-classification-guide/)): 2019.3 起失败看板将构建失败二分为 **verification** (compilation, test assertions 等 — 「标准研发生命周期中预期出现的失败」, 指向开发者输入的问题) vs **non-verification** (build config, dependency resolution, infrastructure 失败等 — 「非预期失败」); 2019.4 起增加**异常消息前缀强制分类**: "Compilation failed / Test failed / Lint failed / Verification failed..." → verification; "Unexpected error / Unexpected failure" → non-verification。
**Datadog CI Visibility (Logs Analysis)** (来源: [官方指南](https://docs.datadoghq.com/continuous_integration/guides/use_ci_jobs_failure_analysis)): 用 **LLM 分类器**将失败日志归入 domain=`code|platform|unknown` + subdomain: code 下 `build|test|quality(format+lint)|security`; platform 下 `assembly|deployment|infrastructure|network|credentials|dependencies|git|checks|setup|script` 共 10 个; unknown 为显式 domain。该分类器同时驱动「可能瞬态失败的自动重试」。附带技术亮点: 日志相关性判定 = 该行未出现在同一 job 历史成功运行中。
**BuildPulse / Trunk.io** ([BuildPulse](https://buildpulse.io) / [Trunk](https://trunk.io)): 定位 flaky test 检测与隔离 (BuildPulse: flaky 识别报告; Trunk: flaky quarantine + lint 目录管理), flaky 维度独立于失败类别 — 与 C7 一致。(未深挖其文档, 定位层面无歧义。)

启示:
- **顶层二分「代码内因 vs 环境外因」是学术+商业的公约数** (Develocity verification/non-verification ≈ Datadog code/platform ≈ Ghaleb environmental/developer-related)。
- Develocity 的前缀强制分类 = 「规则可决定分类」的产业级先例, 与 Log Skimmer「正则规则 + category」的形态完全同构。
- Datadog 为环境域设计了 10 个 subdomain — 说明产业界认为 infra 值得细分类; 但那是 LLM 分类器的能力边界, Log Skimmer 的纯正则规则达不到 (C5)。折中方案: network/credentials/资源/超时等 10 项由 `infra` 单值吸收, 仅 `deps`/`config` 另立 (依赖获取与配置错误的日志形态规整、正则可判)。
- Datadog 与 Log Skimmer 架构同构 (LLM 学规则/分类 + 日志消费), 其「相关日志 = 未见于上次成功运行」的思路可用于 L0 回测语料筛选。

### 3.6 学术 CI 失败分类学

三套互鉴分类学 (数据见 §4; PDF 链接见 §4 表末):
- **Rausch et al. 2017 (MSR)** ([PDF](https://dsg.tuwien.ac.at/team/trausch/pub/PID4727015.pdf)): 对 **14 个** Java OSS 项目研究 (其中 2 个因构建环境异质/日志不可判被排除出错误归因, **12 个**参与归因; 54,248 个 logfile, 92% 成功归类), 得 **14 类分类学**: testfailure / compile / git(VCS 交互错) / buildconfig(构建配置语法错) / crash(构建环境崩溃或超时) / dependency(依赖错, 如非法版本号) / quality(代码检查违规) / **unknown(无法识别原因)** / itestfailure / doc / license / compatibility / androidsdk / buildout。其中 unknown 出现在 12 个归因项目中的 9 个。
- **Vassallo et al. 2017 (ICSME)** ([PDF](https://www.ifi.uzh.ch/seal/people/vassallo/vassalloICSME17.pdf)): 349 OSS + 418 ING 项目, 按 **pipeline 阶段** 分类的分类学: Compilation (production/test/support) / Testing (unit/integration/non-functional/crosscutting) / Packaging / Static Analysis / Dynamic Analysis / Deployment / Documentation / Release Preparation。
- **Ghaleb et al. (TSE, 2021 论文/博士论文)** ([thesis PDF](https://seal-queensu.github.io/publications/pdf/Thesis-Taher-2021.pdf)): 定义三类「噪声损坏」: **Environmental** (环境因素, 60 个子类) / **Cascading** (级联, 非本次变更引入) / **Allowed** (开发者标记允许失败)。

启示:
- Rausch 14 类与本文推荐集合的映射率极高: testfailure→test, compile→build, quality→lint(+fmt), dependency→deps, buildconfig→config, crash/git→infra, unknown→unknown — 推荐集合 ≈ Rausch 分类学的工程裁剪版。
- Rausch 的 `unknown` 作为正式类别 + 高出现率 (9/12 归因项目) 是「必须显式设 unknown 值」的最硬证据; Datadog 把 unknown 提为 domain 级同理。
- itestfailure 只在 4/12 归因项目出现且后来者 (Datadog/Develocity) 均不细分测试类型 — 支持 issue 草案中 unittest/proptest/uitest 三分过细的判断, 收敛为单一 `test`。

### 3.7 Mozilla Treeherder (核实补充)

Firefox 官方文档证实 ([Understanding Treeherder Results](https://firefox-source-docs.mozilla.org/testing/treeherder-try/index.html)): 任务命名按前缀聚类 (build-\*/test-\*/mozlint-\*/source-test-\* — 文中 "NEW failures are for all tasks (build, lint, test, etc.)" 直接印证); **failure_classification 是独立标注维度** ("new failure not classified" 过滤器、intermittent 分类、MITTEN 图标), 由 sheriff 人工/半自动标注, 与任务类型 (category) 分离。经验数据 (同页原文): try push 典型 3–10% job 失败, **其中大多数是 intermittent** (infra 失败/时序/flaky)。

启示: Mozilla 十年级别的超大规模实践验证了「任务类型 (≈category) × 失败判定 (≈intent/属性)」两轴分离; 同时其「多数失败是 intermittent」的经验与 Ghaleb (环境性占 39%) 互相印证 — 环境类失败不是长尾, 是主力。

### 3.8 自举语料: 本仓 CI 日志信息类别枚举

来源: justfile (266 行, check/lint/verify-\*/test-\*/build 家族) + .forgejo/workflows/ci.yml (trigger-poll 薄形态, 真正检查在宿主 forgejo-ci-check unit 即 `just check` 链) + AGENTS.md CI 分级知识。实际会出现的日志信息类别:

| 本仓 CI 环节 | 日志信息类别 | 归入推荐集合 |
| ------------ | ------------ | ------------ |
| treefmt 幂等步 (--fail-on-change) | 被格式化文件清单 | fmt |
| deadnix/statix/shellcheck/ruff/ban-scan/assert-scan | `file:line:col: message` 告警 | lint |
| nim 单测构建 (secrets-test 等) | nim 编译诊断 | typecheck/build |
| 全 host eval | `error: undefined variable ...` 等求值错 | config |
| 触及包定向构建 + FOD | 构建失败 / `got: sha256:..., specified: sha256:...` | build / deps |
| noctalia validate / nginx -t / systemd-analyze verify | 配置 warning/fatal | config |
| secrets 审计 / hardening 审计 | 裸密钥/裸奔服务报告 | security |
| neve 单测 / tests/neve-check-guards | 断言失败输出 | test |
| heavyTests VM 回归 (dae-thin/roaming-dns/nat-nftables/pssd-initrd/eternal-terminal/rsshub) | machine.succeed 失败 / qemu 启动失败 / 超时 | test / infra |
| gen-lan-hosts 漂移哨兵 | 生成产物与源不一致 | config |
| flake input fetch / substituter 下载 | `failed to fetch` / 404 / 超时 | deps / infra |
| ci.yml 轮询 receiver | HTTP 000 / 非 {202,200} (receiver 故障) | infra |
| builder 资源 | `Killed` (OOM) / `No space left on device` | infra |

覆盖度: 推荐集合对上述清单 **全覆盖**; issue 草案 (纯质量门) 对 deps/infra/config/config-校验类约 6 行 **无类可归** (只能漏掉或误塞 unknown)。

启示 (dogfood 视角): 本仓 CI 已有真实的环境域失败样本 — ci.yml 注释记载的 2026-09-13 双 PR 同秒 push 致容器 /tmp 踩踏、宿主 check 实际通过但 job 假红, 即典型 `infra` 类失败; 若 category 无环境域, agent 对这类「日志红但根因不在代码」的场景只能整读原始日志, 恰是 Log Skimmer 要节省的 token 大头。

---

## 4. 学术数据: CI 失败根因分布

| 研究 (年份) | 数据集 | 失败率 | 根因分布 (关键数字) |
| ----------- | ------ | ------ | ------------------- |
| Beller, Böhme, Zaidman, "OOPS, My Tests Broke the Build" (MSR 2017) | TravisTorrent, Java+Ruby | — | 测试失败占损坏构建 **59.0% (Java) / 52.3% (Ruby)**; 构建过程失败 14.4–21.3pp; **infra 问题 ~5%**; 测试结果对构建结局决定性 98.3% |
| Rausch, Hummer, Leitner, Schulte (MSR 2017) | 14 个 Java OSS 项目 (12 个参与错误归因), 54,248 logfile | 均值 **37%** | testfailure+quality+compile 三类 ≈ **62%** 错误; +git+buildconfig 后 **>80%**; 41% 构建因测试失败而败; **构建 worker 故障 (infra flakiness) 平均 9%、最高 27%** |
| Hassan & Wang (ASE 2017 博士论坛, 综述自引) | TravisTorrent | **29%** CI trial 失败 | 测试失败 **65%** / 质量检查错 **13%** / 依赖+编译+配置 **22%** (引 Rausch) |
| Seo et al. "Programmers' Build Errors at Google" (ICSE 2014) | Google CI | **37%** | (编译错误深度研究; 经 Hassan 综述转引) |
| Vassallo et al. "A Tale of CI Build Failures" (ICSME 2017) | 349 OSS + 418 ING, 34,182 次失败 | OSS **26%** | ING: Testing(crosscutting) **28.0%** / unit 5.2% / integration 5.0%; 编译相关仅 4.2% (production) — 远低于 Miller 观测; OSS: crosscutting 13.3%; Miller (Microsoft, 69 次失败小样本): 静态分析类 ~40% |
| Ghaleb, da Costa, Zou, Hassan (TSE; 2021 博士论文 Ch.4) | 321,855 个损坏 job | — | Environmental **39% (job 级) / 33% (build 级)**; Cascading 29% (build 级); Allowed 9%; 三类合计 **~55%** (build 级口径: 33+29+9−重叠 17≈54); Environmental 有 **60 个子类**; 其中 78% 由内部 CI 错误/外部连接/超限导致 |
| Kerzazi et al. (经 Ghaleb 转引) | 大型 web 公司 | — | **~18%** 损坏与代码变更无关 |
| Tahir et al. (JSS 2023, 综述; 转引自检索摘要, 原文未核实) | — | — | **13% 失败构建源于 flaky tests** |

PDF 原文链接: [Beller](https://inventitech.com/assets/publications/2017_beller_gousios_zaidman_oops_my_tests_broke_the_build_an_explorative_analysis_of_travis_ci_with_github.pdf) | [Rausch](https://dsg.tuwien.ac.at/team/trausch/pub/PID4727015.pdf) | [Hassan](https://foyzulhassan.github.io/files/DoctoralSymp_ASE.pdf) | [Vassallo](https://www.ifi.uzh.ch/seal/people/vassallo/vassalloICSME17.pdf) | [Ghaleb thesis](https://seal-queensu.github.io/publications/pdf/Thesis-Taher-2021.pdf)

**对核心张力的裁决**: 数据一致显示 — (a) 质量门类失败 (test+lint/fmt+compile) 是最大头但只占 **62–78%**, 逆命题即 **22–38% 的失败是 git/buildconfig/crash/dependency 类** (38% 为 Rausch 口径, 22% 为 Hassan 引述口径); (b) 若把级联/环境噪声计入 (Ghaleb 口径), 与代码变更无关的失败高达 **~55%**; (c) 商业产品把这个逆命题做成了产品主线 (Develocity 的 non-verification 度量、Datadog 的 platform 域 + 瞬态自动重试)。**结论: 封闭集合必须包含环境根因域, 否则 agent 对 1/5 到 1/2 的失败失去结构化查询能力。**

---

## 5. 候选集合对比 (帕累托前沿)

三个候选均处于帕累托前沿 (A 在打标负担上极端最优、C 在表达力上极端最优, 各不被支配; B 为综合推荐)。

C 候选构成 (≈20 值): B 的质量门域 6 值 + 环境域展开 — deps/network/credentials/resource/timeout/git/config/setup/script/assembly/deployment (Datadog platform 10 subdomain 展开 + infra 拆分) + Rausch 特有 compatibility/doc/license + unknown。

| 维度 | A: 纯质量门 (修正 5 值: fmt/lint/typecheck/security/test) | **B: 两域精选混合 (10 值, 推荐)** | C: 全根因细粒度 (≈20 值, 对齐 Datadog platform subdomain + Rausch 全类) |
| ---- | ---- | ---- | ---- |
| agent 过滤查询表达力 | 弱: 无法表达「只看 infra/deps 失败」, 而这占失败 22–55% | 强: 两域全覆盖, 10 值粒度可组合 (`not lint`、`infra+deps`) | 最强: network/credentials/resource/timeout/git 可单独查询 |
| 集合稳定性 (C6) | 高 (新 linter→lint) | 高: 新工具/包管理器映射进现有值; 扩集触发条件罕见 | 低: 新失败形态频出 (Ghaleb 环境子类高达 60 个即前车之鉴), 枚举持续膨胀 |
| 贡献者打标认知负担 | 最低 (5 值, 判定直觉) | 中 (10 值, 判定表可覆盖边界) | 高: 20 值量级且边界形态学不可分 (TLS 失败: network? credentials? infra? — 违反 C5) |
| 与 intent 正交性 (C3) | 差: 质量门视角隐含「命中≈failure」, category 与 intent 强耦合 | 好: 环境域类别天然中性 (infra 命中可为 warning), 正交性有数据支撑 | 好 (同 B) |
| 自举覆盖度 (§3.8) | ≈54%: 13 行中 6 行 (deps/infra/config/漂移/资源) 无类可归 | **100%** (§3.8 表全部落位) | 100% 但多个值在本仓零规则 (空转) |
| 先例对齐 | 无先例 (matcher 生态无分类; 工具生态不这么做顶层) | ≈ Rausch 14 类裁剪 + Develocity 二分展开 + Datadog code 4 subdomain 对齐 | ≈ Datadog 全 subdomain 平铺 (但 Datadog 是 LLM 分类器, 不受 C5 约束) |

---

## 6. 推荐方案 + 边界案例判定表

### 6.1 推荐集合 (方案 B) 定义

| 值 | 域 | 定义 (判定基准: 工具在 CI 中的角色优先, 日志形态辅助) |
| -- | -- | ---- |
| `fmt` | 质量门 | 格式化工具的漂移报告 (代码风格不一致) |
| `lint` | 质量门 | 静态分析/风格/反模式告警 (不含类型诊断与安全扫描) |
| `typecheck` | 质量门 | 独立类型检查器/编译器的类型诊断 (`nim check`/mypy/pyright/tsc 类) |
| `build` | 质量门 | 编译/链接/打包/构建系统失败 (非类型诊断的构建期错误) |
| `test` | 质量门 | 任何测试框架的断言/用例失败 (unit/integration/VM/e2e/proptest 统一) |
| `security` | 质量门 | 漏洞/密钥泄漏/危险模式的安全扫描告警 (含依赖漏洞审计) |
| `deps` | 环境根因 | 依赖解析/下载/版本冲突/哈希校验失败 (供应链获取问题) |
| `infra` | 环境根因 | 网络/凭据/资源 (OOM、磁盘)/超时/runner 与执行环境故障 |
| `config` | 环境根因 | 配置文件/声明式求值/生成物漂移错误 (nix eval, nginx -t, systemd verify, yaml 语法) |
| `unknown` | 兜底 | 无法唯一判类 (见 §7) |

设计要点: (a) 域前缀 (gate-/env-) 只做文档与 WebPage 分组呈现, **不进枚举值本身** (枚举保持平铺单词, 降低书写摩擦, 域归属由文档承载 — 未来调整域归属不破坏值); (b) `test` 不细分 — 测试类型由 `tool` 表达 (folksonomy 天然胜任: tool=nim-unittest / pytest / vm-test); (c) security 与 deps 的边界: **依赖的漏洞内容** → security, **依赖的获取/解析失败** → deps。

### 6.2 边界案例判定表 (每类 2–3 个具体日志形态)

| 日志形态 (例子) | 判定 | 依据 |
| -------------- | ---- | ---- |
| treefmt `--fail-on-change` 输出被格式化文件清单; prettier `Code style issues found in 2 files` | `fmt` | 格式化工具角色 |
| shellcheck `SC2086 (info): Double quote to prevent...`; ruff `F841 local variable ... never used`; 本仓 ban-scan 命中 | `lint` | 静态告警 |
| tsc `TS2322: Type 'string' is not assignable to type 'number'`; mypy `error: Unsupported operand types for + ("str" and "int")`; nim `Error: type mismatch: got <string, int>` | `typecheck` | 类型诊断 |
| gcc `error: expected ';' before '}' token`; ld `undefined reference to 'foo'`; ninja `FAILED: CMakeFiles/...` | `build` | 编译/链接/构建系统 |
| pytest `FAILED tests/test_x.py::test_y - assert 1 == 2`; cargo test `test result: FAILED. 1 passed; 1 failed`; 本仓 VM 测试 `machine.succeed: command failed` | `test` | 测试断言 (VM 集成测试同) |
| cargo audit `Crate 'openssl-sys' has an unpatched vulnerability (RUSTSEC-2023-XXXX)`; gitleaks `leak detected: AKIA...`; 本仓 secrets 审计命中 | `security` | 安全内容 (即使对象是依赖) |
| npm `404 Not Found - GET https://registry.npmjs.org/xxx`; nix FOD `got: sha256:aaa..., specified: sha256:bbb...`; pip `Could not find a version that satisfies the requirement` | `deps` | 依赖获取/解析/哈希 |
| kernel `Out of memory: Killed process 1234 (nix-daemon)`; `curl: (7) Failed to connect to 192.0.2.10 port 9123` / `SSL certificate problem: self signed certificate in certificate chain` (网络与凭据形态学不可分, 合并); GHA `The job running on runner X has timed out` / 本仓 ci.yml 轮询 HTTP 000 / `No space left on device` | `infra` | 资源/网络/凭据/超时/执行环境 |
| nix `error: undefined variable 'foo'`; nginx `nginx: [emerg] unknown directive "..."`; 本仓 gen-lan-hosts 漂移哨兵 (systemd-analyze verify fatal 同族) | `config` | 配置/求值/生成物一致性 |
| 裸 `Command failed with status (127)` 无前置上下文 (Rausch 提到的典型形态); 日志截断后仅有 exit code | `unknown` | 不可判定 (§7) |
| 覆盖率门 `Coverage: 85% (threshold 90%)` 判红 | `test` (倾向) | 测试衍生质量门; 若 POC 高频出现再议 `metrics` (开放问题 #4) |

表注: `security` 与 `deps` 的分界是「内容 vs 获取」— cargo audit 报依赖漏洞 (内容是安全问题) → `security`; pip/npm 下载或解析失败 (获取问题) → `deps`。

---

## 7. POC 期 `unknown` 打标指引

**原则: 宁缺勿错。** 错标污染 category 的查询语义 (agent 按类过滤拿到假结果, 且不可检测); unknown 只损失该规则自身的过滤价值, 不污染任何类。

何时打 `unknown`:
1. 多因混合: 同一规则样例日志横跨多类且无法用更精确的 pattern 切分;
2. 类别超出枚举: 日志形态规整但 §6.1 无类可归 (这同时是集合演化的信号, 应在 PR 里记录样例);
3. 作者不确定/样例不足: 仅凭 1–2 个样例不足以确认判定基准;
4. L0 回测发现规则同时命中多类日志 (precision 检查未过) → 降级 unknown 而非猜测。

临时约定 (POC 期手写规则):
- `category` 字段省缺即视为 `unknown` (集中预处理: 默认值在规则装载入口统一注入, 核心逻辑假设字段恒存在);
- 双层验证器对 `category != unknown` 的规则, L1 judge 增加「类别可判性」检查项 (judge 认为样例行无法支撑所标类别 → 一票否决降 unknown);
- `unknown` 占比是仪表盘指标: 长期 >40% 触发集合审视 — 人工可归类的日志形态大多可正则化 (Rausch 正是用 expression-based pattern 完成 92% 归因), 因此规则集稳态下 unknown 的大头应是「暂无规则」而非「有规则但 unknown」(此为推断, POC 验证);
- `unknown` 规则升级为具名类别走 PR 人工确认 (类别变更 = 语义变更, 不可静默)。

先例支撑: Rausch 14 类中 `unknown` 是正式成员 (9/12 归因项目出现); Datadog 把 `unknown` 设为 domain 级一等值; Develocity 以 "Unexpected error/failure" 前缀显式兜底 — 三个独立先例一致。

---

## 8. 开放问题

1. `fmt` 是否并入 `lint`? Datadog 的 `quality` 合并了 format+lint, semgrep 无此区分; 但 fmt 的日志形态 (文件清单) 与 lint (file:line:col) 差异显著、规则写法不同, 倾向 POC 后按规则占比裁决。
2. `test` 是否需要细分回 unittest/proptest/uitest? 本报告证据 (Rausch itestfailure 仅 4/12 归因项目、商业产品均不细分) 支持不细分, 由 `tool` 表达; 若 agent 查询日志显示高频「只看单测失败」需求再议。
3. `infra` 何时拆出 `resource`/`timeout`? 拆分条件: infra 内部规则占比失衡 + 形态学可分 (OOM/磁盘/超时的日志特征高度规整, 满足 C5); POC 统计后再定。
4. 指标类报告 (coverage/benchmark/size-budget) 是否需要 `metrics` 类或由 intent 吸收? 当前倾向: 覆盖率门归 `test`, 纯报告类暂不打规则 (Log Skimmer 使命是失败定位与关键信息, 非全量结构化)。
5. 枚举版本化策略: 值只增不改 (C1), 但需定义「category 集合的语义版本」与规则集分发的联动 (规则集 manifest 携带枚举版本, Reader 拒绝携带未知枚举版本的规则集)。
6. 从 GHA problem matcher JSON 自动导入的规则 category 全部为 unknown, 是否值得做一轮 LLM 预标 + 人工复核的批量初始化? (Datadog 证明 LLM 分类在此域可行。)
7. security 类是否预留附加元数据字段 (CVE/严重度), 对齐 semgrep/CodeQL 的 security 附加必填项实践?

## 不确定性声明

- 本报告的失败分布数字均转自论文原文 PDF, 但各研究口径不同 (job 级 vs build 级; 错误归类 vs 构建归因; 2014–2021 年数据, 现代 CI 形态已有变化 — 容器化 runner 时代 infra 失败占比可能更高), 引用时按「量级证据」使用而非精确值。
- Vassallo 的部分分布数字因 pdftotext 布局错位未全部对齐, 表中仅引用可确认项; Seo/Kerzazi/Tahir 数字为二手转引 (转引源为 Hassan 论文与 JSS 综述)。
- 方案 B 的打标负担「中」是推断 (无 POC 实测数据支撑); 这正是 §下一步 要验证的。

## 可验证的下一步 (POC 判据)

1. **自举回测**: 用 §3.8 本仓 CI 的 14 类样例日志 (可从 forgejo-ci-check 的 journal 与历史失败 run 采集) 手写首批 ~30 条规则, 验证 10 值判定表无歧义落位 — 目标: 有类可归率 100%, 打标争议 <10%。
2. **unknown 压力测试**: 对 20 段「无规则覆盖」的真实失败日志跑 LLM Adapter 生成候选规则, 观察其 category 标注分布与本判定表的一致率 — 目标: 一致率 ≥80%, 否则修订判定表而非扩张枚举。
3. **查询场景走查**: 用 3 个典型 agent 请求 (「CI 挂了, 只给我 infra 类线索」/「这个 PR 的 lint 和 fmt 违规」/「测试为什么红」) 走查规则集, 确认 10 值的查询表达力无缺口。
