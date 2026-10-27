# Adapter 创建指南 (项目层规则集 SOP)

> 读者: 在某仓库中使用 log-skimmer MCP、被 skim 结果的 `advice` 字段引导至此的 agent。
> 目标: 为当前仓库创建项目层规则集 (adapter), 让 skim 对本仓库的日志不再失明。

## 概念

adapter = 项目层规则集, 位于 `<repo>/.log-skimmer/rules/*.yaml` (一文件一规则, 可按工具分子目录), 叠加于全局规则集之上、仅对本仓库生效; 随仓库走 (fork 后自动生效)。

何时需要 (默认**不需要**, 先对照判定表):

| 场景 | 落点 |
|---|---|
| 只用标准工具 (cargo/nix/gha/pytest/…) | 不需要 — 全局规则集已覆盖 |
| 自研检查, 输出带项目特异标记 (如 `[myci]` 前缀) | 项目层可直接写; 锚够特异则值得贡献中央仓 |
| 自研检查且输出无特异标记 (宽正则会误撞其他项目日志) | 项目层 — 只有这种情况必须项目层 |

## SOP (五步)

### 1. 采样

收集本仓库 ≥1 段真实 CI 失败日志, 定位失败证据行:

```bash
rg -i -n 'error|fail|panic|fatal|exception' <log>
```

记下每类证据的**锚行** (证据首行) 与续行结构 (多行错误树的缩进/堆栈行)。建议 `rg -C 5` 看上下文。

### 2. 写规则

为每类证据写一条规则。最小模板 (字段约束见文末速查):

```yaml
id: myci-build-fail          # 必须以 tool 值为前缀; 全小写/数字/连字符; 不得与全局规则 id 重复
title: myci 构建失败证据
tool: myci                   # 本项目自定义工具名 (小写)
kind: signal                 # signal=提取证据; noise=标记噪音
intent: failure              # failure|warning|info; noise 规则必须省略本字段
category: build              # 枚举见文末速查; 拿不准 = unknown
patterns:
  - regex: '^\[myci\] error:'  # patterns[0] 是锚行正则
  - regex: '^\s+at '           # 续行 (错误树展开)
    loop: true                 # 贪心重复消费续行
# samples: 本仓 CI run #123 失败日志 (溯源注释, 建议)
```

要点: 正则为 RE2 语义 (无回溯); 锚行带上项目特异标记防误撞其他项目; 召回优先 — 宁可多覆盖 (`intent: failure` 覆盖过头只多几行噪音, 漏检则 skim 失去意义)。

### 3. 落盘

```bash
mkdir -p .log-skimmer/rules
# 把第 2 步的 yaml 写入 .log-skimmer/rules/
```

无需重启 MCP server: skim/list_rules 每次调用自动感知项目层文件变化 (保存后重新调用即生效)。前提: MCP server 以本仓库根为启动目录 (项目层探测基准是 server 启动时的 cwd)。

### 4. 质检 (三项全过)

用真实失败日志再次调用 `skim`:

1. **装载通过**: skim 正常返回。若报「项目层规则装载失败」, 按 YAML 细节修复后重试;
2. **命中**: 目标规则 id 出现在 `blocks[].rules`, 且顶层 `fallback` 字段消失;
3. **语义**: 命中块的 `intent`/`category` 与证据性质一致 (失败证据必须是 failure 块)。

未过的修法: 不命中 → 先用 `rg <regex> <log>` 验证锚正则确实命中日志实测行; 正则命中但 skim 仍零块 → 调用 `list_rules` 看新规则 id 是否在场 — 不在场说明 MCP server 的启动目录不是本仓库根 (项目层探测基准是 server **启动时的 cwd**, 常见于宿主客户端在别的目录拉起 server), 此时向用户报告, 由其修改 MCP server 启动配置 (以本仓库根为 cwd) 后重试; 误命中过多 → 收紧锚; 装载错 → 对照文末速查查字段约束。

### 5. 持久化

```bash
git add .log-skimmer/rules
git commit -m "feat: log-skimmer adapter (项目层规则)"
```

## (OSS 项目) 贡献上游

若正则锚带项目特异标记 (不会误撞其他项目日志), 规则可直接进中央规则仓, 所有项目受益:

1. PR 到 <https://github.com/luochen1990/log-skimmer-rules> (读该仓 README 的贡献指引);
2. 贡献规则的 `attribution` 字段必填:

```yaml
attribution:
  contributor: <github 用户名>
  generated-by: { harness: <agent/工具名>, model: <模型名> }
  adapter-version: <x.y.z>
```

3. 被合入后删除本仓库项目层中对应规则 (全局层已覆盖), 避免重复维护。

## 附: 字段速查

| 字段 | 约束 |
|---|---|
| id | `^[a-z0-9-]+$`, 以 tool 值为前缀, 全局唯一 (与全局层同 id = 装载错误) |
| patterns | ≥1 项; 每项 `regex` 必填, `loop` 默认 false; `[0]` 为锚 |
| kind | `signal` / `noise` |
| intent | `failure` / `warning` / `info` (省缺 = failure); noise 必须缺省 |
| category | `fmt`/`lint`/`typecheck`/`build`/`test`/`security`/`deps`/`infra`/`config`/`unknown` (省缺 = unknown) |
| confidence | `high` / `medium` / `low` (省缺 = medium) |
| context | `{ before: 0-10, after: 0-10 }` (省缺 0/0) |
| 弃用 | `scope` / `repo` 勿写 (v0.6 退役, 生效范围由存储位置决定) |

完整规范 (状态机语义/manifest/输出协议): <https://github.com/luochen1990/log-skimmer/blob/master/docs/rule-format.md>
