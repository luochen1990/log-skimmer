# Log Skimmer POC — 语料与规则交付报告

> 职责边界: 记录 POC 阶段语料抓取结果 (样本量表/跳过明细)、三仓工具识别结论、
> 手写规则清单与来源锚命中自验数据、spec 反馈与执行中遇到的问题。
> 生成于 2026-10-16; 抓取脚本 `scripts/fetch-logs.sh`; 锚命中/FP 统计由 L0 门禁链 (scripts/l0-gate.sh) 承接;
> POC 期辅助工具 verify-rules.sh / sim-blocks.py 已被引擎加载校验 (rules_load) 与公理单测取代删除。

## 1. 样本量表

配额要求: 每仓失败 run ≥5 + 成功 run =5, 三仓合计 ≥30 对。实际超额完成:

| 仓               | 失败 run (job 对) | 成功 run (job 对) | 合计 |
| ---------------- | ----------------- | ----------------- | ---- |
| sst/opencode     | 14                | 5                 | 19   |
| niri-wm/niri     | 14                | 5                 | 19   |
| nix-community/home-manager | 14      | 5                 | 19   |
| **合计**         | **42**            | **15**            | **57** |

- 失败 run 采样策略: 每 run 取第一个 `conclusion=failure` 的 job; 成功 run 取第一个成功 job。
  home-manager 失败较多为 `conflicts.yml` (label 配置) 与 GitHub Pages 部署冲突, 真正的 nix 失败集中在 Test workflow (4 个)。
- 跳过明细: 0 个 job 日志端点遭遇 404/410 过期清理; 20 个 run 因「无 failure job」跳过
  (workflow 级失败/依赖取消, jobs 里没有 conclusion=failure 的条目, 三仓均有, opencode/niri 为主)。
- 幂等性: 脚本重跑对已存在 run (按 `<run_id>-*.log` glob 判定) 秒级跳过, 已验证。

## 2. 工具识别结论 (按仓)

**sst/opencode** (TS/bun)

- 主力: `bun` — `bun run <script>` 脚本失败 (`error: script "x" exited with code N`)、
  `bun test` (`(fail)` 用例行 + `N pass / N fail / Ran N tests` 汇总)。
- `eslint` — 改动文件检查 (`lint:changed`), 自定义 `anti-slop` 规则集, 紧凑单行 formatter
  (`path:line:col␣␣rule␣␣message`, 双空格分隔, 无 severity 字段)。
- `playwright` — e2e + component tests (失败编号头 `N) [browser] ›`、4 缩进 `Error:` 详情块、
  `Locator:/Expected:/Received:/Timeout:` 键值行、attachment 列表; 大量 `ESC[1A ESC[2K [n/N]` 进度刷新噪音)。
- 失败大头实为 triage agent 崩溃 (JSON 形态 `Error: { "name": "UnknownError", ... }`, 非「构建/检查工具」),
  由 `bun-error-json` + `github-actions-error-annotation` 覆盖, 避免 FN。

**niri-wm/niri** (Rust/cargo)

- `cargo` — MSRV 解析失败 (`error: rustc X is not supported by the following package:` + 2 缩进依赖行)、
  版本/feature 冲突树 (`error: failed to select a version for` + 4 缩进依赖链 + help)、
  编译失败汇总 (`error: could not compile`); 依赖下载/编译进度行 (`Downloaded/Compiling`, 高频噪音)。
- `rustc` — 带诊断码错误树 (`error[E0277]:` 锚 + `-->`/`|` gutter/行号 gutter/help/note/`=` note/`...` 结构),
  CI 环境下输出带 ANSI 色码且行号 gutter 有右对齐填充 (色码之内含前导空格)。
- `rust` (运行时/libtest) — `thread '<name>' (id) panicked at` + 消息 + `note: run with RUST_BACKTRACE=1`;
  `test <name> ... FAILED`; insta 快照断言失败。
- 基础设施类: rustup/curl 网络抖动 (dtolnay/rust-toolchain action)、action 版本解析失败 — 无工具结构输出, 由 gh-annotation 兜底。

**nix-community/home-manager** (nix)

- `nix` — `nix build --show-trace` 求值错误树: 裸 `error:` 行 + 7 空格缩进 trace 帧
  (`… while ...` / `at file:line:col:` / 代码 gutter `NNN|` / caret `| ^`) + 帧间空行 + 末帧缩进 `error: <msg>`。
- `nixfmt` — `nix fmt -- --ci` 的 fail-on-change (`Error: unexpected changes detected`, 上方有 `formatted N files (M changed)` 统计行)。
- `git` — gh-pages 部署 push 被拒 (`! [rejected]` + `error: failed to push some refs` + `hint:` 树)。
- GitHub Actions: conflicts-label workflow 配置错误 (`"merge conflict" label not found` 等)。

## 3. 规则清单与自验数据

16 条 signal + 2 条 noise, 全部通过引擎真实加载校验 (rules_load) 与 L0 全语料回归 (l0-gate)。
「锚命中」= patterns[0] 在来源失败日志中的 grep 行数; 「失败日志块数」= A2/A3 模拟器产出 ≥1 块的失败日志数;
FP = 成功日志上的 signal 块数 (L0 FP 代理)。

| 规则 id                          | kind | 一句话                                | 锚命中 | 失败日志块数 | FP |
| -------------------------------- | ---- | ------------------------------------- | ------ | ------------ | -- |
| bun-error-json                   | sig  | triage agent JSON 形态未捕获错误块     | 11     | 11           | 0  |
| bun-script-exit-fail             | sig  | bun run 脚本非零退出                   | 2      | 2            | 0  |
| bun-test-fail                    | sig  | bun test 失败用例行 `(fail)`          | 1      | 1            | 0  |
| cargo-could-not-compile          | sig  | cargo 编译失败汇总行                  | 8      | 5            | 0  |
| cargo-msrv-unsupported           | sig  | cargo MSRV 不满足树                   | 2      | 2            | 0  |
| cargo-version-conflict           | sig  | cargo 版本/feature 冲突树             | 2      | 2            | 0  |
| eslint-offense-line              | sig  | eslint 违规行 (紧凑 formatter)        | 27     | 1            | 0  |
| git-push-rejected                | sig  | git push 被拒 (rejected+hint 树)      | 2      | 2            | 0  |
| github-actions-error-annotation  | sig  | `##[error]` 工具无关失败安全网         | 45     | 42 (全量)    | 0  |
| nixfmt-fail-on-change            | sig  | nixfmt --ci 检出未格式化改动           | 1      | 1            | 0  |
| nix-eval-error-tree              | sig  | nix 求值错误树 (裸 error: 锚)          | 3      | 3            | 0  |
| playwright-assert-failed         | sig  | playwright 断言失败详情块              | 69     | 1            | 0  |
| playwright-failed-test-header    | sig  | playwright 失败用例编号头              | 26     | 1            | 0  |
| rustc-error-tree                 | sig  | rustc 带码编译错误树                  | 9      | 5            | 0  |
| rust-libtest-failed              | sig  | libtest 单测失败行                    | 2      | 1            | 0  |
| rust-test-panic                  | sig  | Rust 运行时 panic 头                  | 3      | 1            | 0  |
| cargo-progress-noise             | noise | cargo 下载/编译进度行                 | 1949   | -            | -  |
| playwright-progress-noise        | noise | playwright 终端进度刷新行             | 682    | -            | -  |

块边界抽查 (POC 期 sim-blocks 模拟, 语义现已由 engine/tests/axioms.rs 公理单测钉住): nix-eval-error-tree 单块 219 行 (完整 trace 树, 空行分段帧全吞入, `##[error]` 闭块);
rustc-error-tree E0277 单块 27 行 / E0004 双块各 15 行; cargo-version-conflict 10 行完整树;
git-push-rejected 7 行 (rejected→error→4 hint); cargo-msrv 2 行; bun-error-json 7 行完整 JSON。

L0 代理指标: FN 代理 = 0 (42/42 失败日志至少 1 个 signal 块, gh-annotation 全量兜底); FP 代理 = 0 (15 个成功日志零 signal 块)。
压缩率需引擎统计, 本阶段未算 (模拟器仅验证边界)。

## 4. spec 反馈

1. **行分隔符/CRLF 语义未定义**: Windows runner 日志为 CRLF; 规则的 `$` 锚在含 `\r` 的原始行上失配。
   我按 Rust `lines()` 语义 (剥 `\r`) 设计, 自验工具同步 `sed 's/\r$//'`。建议 spec §2 明确行消费语义。
2. **宿主前缀 (时间戳/ANSI) 未提及**: spec 示例锚 `^error: ` 是裸工具输出形态; 真实 GH Actions 日志
   每行带 `YYYY-MM-DDTHH:MM:SS.nnnnnnnZ ` 前缀, 部分工具输出嵌 ANSI 色码 (含色码内右对齐填充空格等怪形态)。
   照抄示例会 0 命中。建议 spec 注明前缀容忍由规则自行承载, 或未来定义引擎预处理层 (POC 按前者)。
3. **A3 透传表述歧义**: 「不命中 → p += 1, 用 patterns[p+1..] 中下一个 pattern 继续尝试同一行」的索引写法
   易误读; 实现按「p 逐 1 前进, 每步用当前 pattern 试同一行, 越过末尾闭块」。建议改精确伪代码。
4. **A6 与 A4 的作用顺序**: context 扩展后的行若被 noise 覆盖, 先扩展后剔除还是分别处理未定义;
   按「A4 先扩输出区间, A6 再对输出行集做差集」理解 (无实际样本触及, 本批规则 context 区间与 noise 不重叠)。
5. **noise_lines_removed 统计口径**: 是「noise 覆盖行数」还是「实际从输出剔除的行数」(后者 = 与 signal
   输出区间相交的 noise 行) 未明确; 按「剔除的行数」理解更符合命名。
6. **`# samples:` 头注释格式**: spec §5 约定 `<repo>/<run>-<job>`; 多样本逗号分隔的个数上限未约定,
   本批每条 ≤5 个 (rustc-error-tree 实际命中 5 个来源, 全列)。

## 5. 遇到的问题

1. 本机 bash 编译禁用了 programmable completion (`compgen` 不存在) → 幂等检查与文件计数改用 glob/`find`。
2. fetch 脚本首版双重计数: 已存在 run 在初始存量计数后又在线内 `return 0` 计一次, 导致目标 14 实抓 9 即停;
   修复返回语义 (0=新落盘计配额 / 2=已存在不计 / 1=跳过)。
3. 验证脚本首版把 grep「无匹配」(exit 1) 误判为「编译失败」(exit 2), 全部规则误报 REGEX-FAIL; 修正退出码判定。
4. rustc 扩展正则两处真实缺陷由块边界模拟暴露: (a) 色码输出中 `help`/`note` 与冒号之间有 `\x1b[0m`;
   (b) 行号 gutter 右对齐的填充空格位于色码之内 (`ESC[...m␣753`)。修正后块从 5 行 → 15~27 行完整树。
5. opencode 失败大头是 triage agent 崩溃而非构建/测试工具, 初版规则集会漏检 11/14 → 增设
   `bun-error-json` 与 `github-actions-error-annotation` 兜底 (也使 FN 代理归零)。
6. home-manager 真正的 nix 失败样本少 (Test workflow 4/14), conflicts-label 与 Pages 部署失败占比高 —
   语料配额已尽量深翻页补齐, 若后续需要更多 nix 错误形态 (如 build 型 deriver 失败) 需扩仓或放宽时间窗。

## 6. Forgejo 本仓语料 (v2 增补, 2026-10-16)

- 失败 7 + 成功 3 (pull_request), 落盘内网 Forgejo 语料槽, sidecar 含 attempt + ci: forgejo。
- 形态: agent-service 沙箱轮询回显 (`… [NmNns]` 前缀, 值树逐行打散) — 与 GHA 直出完全不同, 规则正则自承载适配 (spec A0)。
- 新增规则 9 条 (6 signal + 3 noise): just-recipe-fail / nim-segv / nix-option-conflict / nix-deprecation-warning (首个 intent: warning) / treefmt-fail-on-change / forgejo-actions-job-failed (安全网) / systemd-ci-unit-fail + poll/fetch/drv 三条 noise。
- 已知差异: Forgejo job logs 全量与 byte-range 接口个别行前缀不一致, 语料以 byte-range (可互验) 为准。
