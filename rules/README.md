# 规则集 (rules/) — 贡献者入门

> 职责边界: 面向规则贡献者的接口文档 — 规则集布局速览、字段速览、打标纪律与贡献流程。规则 YAML 格式、状态机语义、L0 统计口径的单一事实来源是 [../docs/rule-format.md](../docs/rule-format.md) (下称 spec); 本文只做速览与指针, 不重复其判定基准。

## 1. 规则集概貌

- 布局: `rules/<tool-id>/<rule-id>.yaml`, 一文件一规则 (如 `rules/nix/nix-eval-error-tree.yaml`)。
- `tool-id` 为 folksonomy 约定名: 无中央注册表, 规则文件的存在即定义 ID (小写字母/数字/连字符)。
- `rule-id` 形如 `<tool-id>-<语义名>`, 全局唯一, 同 id 重复加载报错。
- 当前覆盖 15 个工具: bun, cargo, eslint, forgejo-actions, git, github-actions, just, nim, nix, nixfmt, playwright, rust, rustc, systemd, treefmt。

## 2. 字段语义速览

| 字段 | 必填 | 默认 | 语义 |
| --- | --- | --- | --- |
| id | 必填 | — | 规则唯一标识, `<tool-id>-<语义名>`, 须以 tool 值为前缀 |
| title | 必填 | — | 一句话人类可读描述 |
| tool | 必填 | — | 工具维度, 与目录名一致 |
| scope | 必填 | — | `global` 或 `repo`, 规则是否仅作用于单一仓库 |
| repo | 条件必填 | null | scope 为 repo 时必填, `host/owner/repo` 完整形态; global 时必须为 null |
| patterns | 必填 | — | 状态机 pattern 链 (≥1 项), `patterns[0]` 为锚, 是开块的唯一途径 |
| kind | 必填 | — | `signal` 提取信号片段; `noise` 标记噪音行 (覆盖行从输出中剔除) |
| intent | 选填 | failure | `failure` / `warning` / `info`, 命中对成败判定意味着什么 |
| confidence | 选填 | medium | `high` / `medium` / `low`, 片段继承, 呈现排序参考 |
| category | 选填 | unknown | 封闭枚举 10 值, 信息类别; 省缺即 unknown |
| context | 选填 | before: 0, after: 0 | 输出边界扩展行数 (各 0-10), 只影响输出不参与状态机 |
| attribution | 条件必填 | null | 规则来源声明, Adapter 生成时必填, 手写规则省略 |

intent 三值与 category 十值的详细判定基准不在本文重复:

- intent 语义表与全部字段加载期约束: spec §1 ([../docs/rule-format.md](../docs/rule-format.md))
- category 判定基准 (工具在 CI 中的角色优先, 日志形态辅助): [../docs/category-survey.md](../docs/category-survey.md)

另有保留占位字段 `structured-hint` (POC 不实现), 见 spec §1。

## 3. 打标纪律

核心原则**宁缺勿错**:

- 错标 (category/intent 与实际不符) 污染查询语义 — 下游按这些字段做程序化过滤, 错标不可检测且误导全局; unknown 只损失该规则自身这一条过滤价值。
- 以下情形一律打 `unknown`: 多因混合无法唯一判类 / 样例不足拿不准 / L0 回测发现规则命中多类日志 / 类别超出枚举 (属集合演化信号, 在 PR 中记录样例)。
- `kind: noise` 的规则必须缺省 intent; signal 规则 intent 默认 failure, 确认语义分层后才改标 warning 或 info。
- unknown 占比长期 >40% 触发集合审视; unknown 升级具名走 PR 人工确认; flaky 是属性不是类别, 不进 category。

## 4. 贡献流程与溯源约定

1. 复制 `rules/_template.yaml` 起步, 存为 `rules/<tool-id>/<rule-id>.yaml`。
2. 头注释写样例溯源: `# samples: <repo>/<run>-<job>, ...` (逗号分隔多个样本; 约定见 spec §5 — 样本本体经 `just corpus-fetch` 获取到 `data/logs/` (语料双源: GitHub 语料仓 + 内网现场拉取), 头注释仍是规则↔样本的人工溯源线索)。头注释同时建议写 why 叙事 (锚/扩展正则的匹配依据与宿主前缀容忍考虑), 参考示例: [nix/nix-eval-error-tree.yaml](nix/nix-eval-error-tree.yaml), [bun/bun-error-json.yaml](bun/bun-error-json.yaml)。
3. 新增/修改规则必须跑 `just ci-merge`。其中 L0 门禁的判定阈值 SSOT 在 [../scripts/l0-gate.sh](../scripts/l0-gate.sh) 头注释, 与规则贡献者直接相关的四项: FN=0 (失败样本零 failure 片段的样本数必须为 0) / FP 率 ≤2% (success 样本上的 failure-intent 假阳性) / 零块=0 (任何语料日志 skim 非零, 零块即规则集对该日志失明) / 压缩率基线防退化 (相对退化 >2% 判红)。
4. 规则语法错误 (regex 编译失败/枚举越界/字段约束违例) 会被 `engine/tests/rules_load.rs` 加载回归拦下; 单条规则违例会导致整个规则集拒绝加载, 提交前先本地跑门禁。
