//! 引擎用户可见文案单点收敛 (issue #76: CLI/MCP 文案默认英文化 + i18n 结构预留)。
//!
//! 职责边界: 引擎 CLI 运行期 stderr/stdout 诊断文案与人类可读渲染行的唯一来源 —
//! 未来引入 zh 文案映射时只改本模块 (消费点零改动)。
//! 风格: 英文、简洁、自解释 — 保留信息三要素 (哪个校验失败 / 建议动作 / 涉及值),
//! 句式与 README.md 英文正文一致。
//!
//! 刻意不收敛面 (收敛成本 > 收益, 各自就地英文):
//! - clap help 文案 (main.rs 的 doc comment / 属性): 与 CLI 参数定义同生共演,
//!   未来 zh 化时随 clap 属性化一并收敛;
//! - sample_io 的 Err (`{路径}: {io 原因}`): 纯路径 × 系统错误拼接, 无自然语言成分;
//! - log-skimmer-wasm 的 3 条 ABI 参数层错误 (null 指针 / 非 UTF-8 / rules JSON
//!   解析失败): 与手写 C ABI 调用契约同生共演且无 CLI 对应形态, 为 3 条串引入
//!   跨 crate 依赖不值当 — wasm 侧装载失败/输入拒斥文案已复用本模块 (经 engine
//!   lib 单点), 不在此列;
//! - 不变式 panic 文案 (如 manifest.rs 的引擎版本常量畸形): 构建契约违例的
//!   dev-facing 断言 (同测试 expect/panic 语), 不属运行期用户交互面。

use std::path::Path;

// ---- 规则装载 (lib.rs: load_rules / load_rules_from_sources / validate_rule) ----

/// LoadErrors Display 前缀: 聚合计数 + 逐条罗列 (整体拒绝, spec §1; 单复数随 n)。
pub fn ruleset_invalid(n: usize, joined: &str) -> String {
    let unit = if n == 1 { "issue" } else { "issues" };
    format!("invalid ruleset ({n} {unit}):\n{joined}")
}

/// 规则目录不存在 (引擎失明拒绝, 非静默空集)。
pub fn rules_dir_not_found(dir: &Path) -> String {
    format!("{}: rules directory not found", dir.display())
}

/// 路径存在但非目录 (如实区分于「不存在」, 防误导排障方向 — issue #47a)。
pub fn rules_path_not_a_dir(dir: &Path) -> String {
    format!(
        "{}: path exists but is not a directory (a ruleset must be a directory)",
        dir.display()
    )
}

/// 目录下无 *.yaml (空规则集 = 引擎失明)。
pub fn ruleset_no_yaml(dir: &Path) -> String {
    format!(
        "{}: no *.yaml files under the directory (an empty ruleset leaves the engine blind)",
        dir.display()
    )
}

/// 内存源形态的空集拒绝 (与 [ruleset_no_yaml] 同族)。
pub fn sources_no_rules() -> &'static str {
    "(sources): no rule sources (an empty ruleset leaves the engine blind)"
}

/// 单个规则文件读取失败。
pub fn source_read_failed(rel: &str, err: impl std::fmt::Display) -> String {
    format!("{rel}: read failed: {err}")
}

/// 规则文件 YAML/字段解析失败 (serde 细节透传)。
pub fn rule_parse_failed(rel: &str, err: impl std::fmt::Display) -> String {
    format!("{rel}: YAML/field parse failed: {err}")
}

/// id 全局唯一违例 — 并列首见源与冲突源 (issue #14 B 的 grep-ability 契约)。
pub fn dup_rule_id(id: &str, first: &str, rel: &str) -> String {
    format!("duplicate rule id: {id} ({first} vs {rel})")
}

/// id 字符集违例 (spec §1 字段约束)。
pub fn bad_rule_id(rel: &str, id: &str) -> String {
    format!("{rel}: invalid id (must match ^[a-z0-9-]+$): {id}")
}

/// id 未以 tool 值为前缀。
pub fn id_prefix_mismatch(rel: &str, id: &str, tool: &str) -> String {
    format!("{rel}: id must be prefixed with the tool value: id={id} tool={tool}")
}

/// patterns 为空数组。
pub fn patterns_empty(rel: &str) -> String {
    format!("{rel}: patterns is empty")
}

/// kind=noise 时 intent 必须缺省。
pub fn noise_with_intent(rel: &str) -> String {
    format!("{rel}: kind=noise must omit intent")
}

/// context 超出 [0,10] (文案是 spec 人话表述, 有意不随 CONTEXT_MAX 动态化)。
pub fn context_out_of_range(rel: &str) -> String {
    format!("{rel}: context out of range [0,10]")
}

/// regex 编译失败 (serde/regex 细节透传)。
pub fn regex_compile_failed(rel: &str, idx: usize, err: impl std::fmt::Display) -> String {
    format!("{rel}: patterns[{idx}] regex compile failed: {err}")
}

// ---- manifest 版本协商 (manifest.rs, spec §1.M) ----

pub fn manifest_parse_failed(origin: &str, err: impl std::fmt::Display) -> String {
    format!("manifest: parse failed ({origin}): {err}")
}

pub fn manifest_read_failed(path: &Path, err: impl std::fmt::Display) -> String {
    format!("manifest: read failed ({}): {err}", path.display())
}

/// 规则集声明版本超出引擎支持版本 — 建议升级引擎 (或回退规则集版本)。
pub fn manifest_format_too_new(declared: u32, supported: u32) -> String {
    format!(
        "manifest: format_version {declared} exceeds this engine's supported version \
         {supported} — the ruleset uses a newer rule format; upgrade the engine \
         (or pin back the ruleset)"
    )
}

/// min_engine_version 形态非法 (须完整数字三元组)。
pub fn manifest_bad_min_engine_version(min: &str) -> String {
    format!(
        "manifest: min_engine_version \"{min}\" is invalid (must be a full numeric \
         major.minor.patch, e.g. 0.2.0)"
    )
}

/// min_engine_version 高于本引擎 — 建议升级引擎 (或回退规则集版本)。
pub fn manifest_engine_too_old(min: &str, engine: &str) -> String {
    format!(
        "manifest: min_engine_version {min} is above this engine {engine} — the ruleset \
         needs a newer engine; upgrade the engine (or pin back the ruleset)"
    )
}

// ---- 输入三分类拒斥 (lib.rs input_reject_message; CLI stderr 与 wasm envelope 共用) ----

/// 空输入 (0 行): 疑似下载截断, 数据错误族硬拒绝。
pub fn log_empty() -> &'static str {
    "log is empty (0 lines): possibly a truncated download; refusing to pass silently"
}

/// 纯空白输入: 疑似下载截断或空跑, 指向日志源排查。
pub fn log_whitespace_only(lines: usize) -> String {
    format!(
        "log is whitespace-only ({lines} blank lines): possibly a truncated download or an \
         empty run; check the log source; refusing to pass silently"
    )
}

// ---- 人类可读渲染 (CLI stdout) ----

/// 折叠标记行 (spec §3.R2): 锚块文本后的单行元数据; 锚行 = 各被折叠 fragment
/// 区间首元素, 最多列 8 个, 超出以 `…(N anchors total)` 收尾。不计入 output_lines。
pub fn collapse_line(
    repeat_blocks: usize,
    rule: &str,
    start_line: usize,
    end_line: usize,
    omitted_lines: usize,
    frag_spans: &[(usize, usize)],
) -> String {
    let mut s = format!(
        "⟨×{repeat_blocks} {rule} folded blocks [L{start_line}-L{end_line}]; \
         {omitted_lines} lines omitted; anchors"
    );
    for (a, _) in frag_spans.iter().take(8) {
        s.push_str(&format!(" L{a}"));
    }
    if frag_spans.len() > 8 {
        s.push_str(&collapse_anchors_overflow(frag_spans.len()));
    }
    s.push('⟩');
    s
}

/// 折叠行锚行列表的收尾 (锚行 >8 个时; 仅 collapse_line 消费)。
fn collapse_anchors_overflow(total: usize) -> String {
    format!(" …({total} anchors total)")
}

/// CLI 统计行 (人类可读输出收尾): `input N lines -> output M lines (pct compression),
/// noise removed X, boundary violations Y{, fold_note}`。pct 为 `n/a` 或 `x.x%`
/// 由调用方决策 (零块 → "n/a")。
pub fn stats_line(
    input_lines: usize,
    output_lines: usize,
    pct: &str,
    noise_removed: usize,
    boundary_violations: usize,
    fold_note: &str,
) -> String {
    format!(
        "input {input_lines} lines -> output {output_lines} lines ({pct} compression), \
         noise removed {noise_removed}, boundary violations {boundary_violations}{fold_note}"
    )
}

/// 统计行折叠尾注 (spec §3.R2): 零折叠时由调用方传空串 (零行为变更保证)。
pub fn fold_note(folded_blocks: usize, omitted_lines: usize) -> String {
    format!(", repeat-collapse folded {folded_blocks} blocks (-{omitted_lines} lines)")
}

/// 零块失明警示 (spec §3, 无条件): 回退基线 + 贡献规则出口。
pub fn zero_blocks_warning() -> &'static str {
    "⚠ zero blocks extracted — the ruleset is blind to this log; fall back to the \
     rg 'error|warn' + tail baseline; for logs of new tools consider contributing \
     rules: https://github.com/luochen1990/log-skimmer-rules"
}

// ---- CLI stderr (main.rs: skim / l0 / hygiene 子命令) ----

pub fn read_log_failed_stdin(err: impl std::fmt::Display) -> String {
    format!("failed to read log (stdin): {err}")
}

pub fn read_log_failed(err: impl std::fmt::Display) -> String {
    format!("failed to read log {err}")
}

/// 路径参数空串专属文案 (issue #73a): 与真缺值 (clap "a value is required ...")
/// 分流; label 为参数名 (如 --rules), hint 为期望语义。
pub fn empty_path_arg(label: &str, hint: &str) -> String {
    format!("{label} cannot be empty ({hint})")
}

pub fn l0_collect_failed(err: impl std::fmt::Display) -> String {
    format!("l0 collection failed: {err}")
}

/// 孤儿样本缺侧标签 (issue #46): 单侧在场, 跳过不计数。
pub fn missing_side_label(missing: &crate::l0::MissingSide) -> &'static str {
    match missing {
        crate::l0::MissingSide::Sidecar => "sidecar (*.log.json)",
        crate::l0::MissingSide::Log => "log (*.log)",
    }
}

/// 孤儿样本警告 (issue #46): 单侧在场, 跳过不计数。
pub fn l0_orphan_warning(path: &str, missing_side: &str) -> String {
    format!("l0: warning: orphan sample {path} missing {missing_side}, skipped (not counted)")
}

/// 零配对样本 = 采集层语料非法 (issue #44), 拒绝在空语料上判定。
pub fn l0_zero_paired_samples(corpus: &Path) -> String {
    format!(
        "l0: corpus has zero paired samples ({}) — invalid corpus, refusing to judge \
         (check the directory contents / the fetch chain)",
        corpus.display()
    )
}

pub fn l0_base_read_failed(path: &Path, err: impl std::fmt::Display) -> String {
    format!("base summary read failed {}: {err}", path.display())
}

pub fn l0_base_parse_failed(path: &Path, err: impl std::fmt::Display) -> String {
    format!("base summary parse failed {}: {err}", path.display())
}

pub fn hygiene_patterns_load_failed(err: impl std::fmt::Display) -> String {
    format!("hygiene: failed to load the pattern list — {err}")
}

pub fn hygiene_dir_not_found(dir: &Path) -> String {
    format!("hygiene: directory not found {}", dir.display())
}

pub fn hygiene_pass(n_patterns: usize, dir: &Path) -> String {
    format!(
        "hygiene: pass — {n_patterns} credential patterns, zero hits ({})",
        dir.display()
    )
}

pub fn hygiene_hits_header(n: usize) -> String {
    format!(
        "hygiene: {n} credential-pattern hits — critical privacy red line, must never enter \
         the repo (fix: move credentials to CI secrets / the runtime environment; entries \
         already in history require git filter-repo):"
    )
}

// ---- 凭据形态清单装载 (hygiene.rs load_patterns / load_lines) ----

pub fn hygiene_list_read_failed(path: &Path, err: impl std::fmt::Display) -> String {
    format!(
        "{}: {err} (the list file must ship with the repo)",
        path.display()
    )
}

/// 清单行内含 # (行内注释笔误形态, 整行成为字面模式 — 防线静默失效)。
pub fn hygiene_inline_comment(path: &Path, bad: &str) -> String {
    format!(
        "{}: pattern contains # (inline comment?) — {bad:?}; comments must occupy \
         their own line",
        path.display()
    )
}

pub fn hygiene_duplicate(path: &Path, item: &str) -> String {
    format!("{}: duplicate pattern ({item})", path.display())
}

pub fn hygiene_empty_list(path: &Path) -> String {
    format!(
        "{}: zero valid patterns — an empty list disarms the guard",
        path.display()
    )
}

pub fn hygiene_bad_regex(path: &Path, pattern: &str, err: impl std::fmt::Display) -> String {
    format!(
        "{}: invalid regex in the pattern list — {pattern:?}: {err}",
        path.display()
    )
}

// ---- L0 采集层 (l0.rs collect) ----

pub fn corpus_dir_not_found(dir: &Path) -> String {
    format!("corpus directory not found: {}", dir.display())
}

/// 同配对名 plain 与 gz 双份在场 = 配对歧义, 拒绝静默择一。
pub fn pairing_name_conflict(plain: &str, gz: &str, sidecar: &str) -> String {
    format!(
        "sample pairing-name conflict: {plain} and {gz} share a pairing name (sidecar \
         {sidecar}); refusing to silently pick one"
    )
}

pub fn sidecar_read_failed(path: &Path, err: impl std::fmt::Display) -> String {
    format!("sidecar read failed {}: {err}", path.display())
}

pub fn sidecar_parse_failed(path: &Path, err: impl std::fmt::Display) -> String {
    format!("sidecar parse failed {}: {err}", path.display())
}

pub fn l0_log_read_failed(err: impl std::fmt::Display) -> String {
    format!("log read failed {err}")
}

/// sidecar 缺失 attempt 的聚合计数单行警告 (P0-2: 按 attempt=1 兜底不中断, 缺口可见)。
pub fn attempt_missing_warning(n: usize) -> String {
    format!(
        "l0: warning: {n} sidecar(s) missing the attempt field (treated as attempt=1; \
         spec §5 mandates it; consider refetching)"
    )
}

// ---- L0 判定层 (l0.rs gate: violation notes + 单行摘要) ----

/// 语料样本数不足 — note 分流两种读法 (issue #44: 定向验证属预期 / 全量语料疑获取链路)。
pub fn note_corpus_missing(samples: usize, min_samples: usize) -> String {
    format!(
        "corpus sample floor not met ({samples} <{min_samples}) — expected for targeted \
         small-corpus validation (the fail is an anti-idle-green guard; the positive signal \
         is per_rule_hits); on a full corpus this suggests a fetch-chain failure"
    )
}

/// 仓维度覆盖哨兵 (样本够但覆盖广度缩水 — 样本维度哨兵探不到的形态)。
pub fn note_corpus_repos_missing(
    samples: usize,
    min_samples: usize,
    repos: usize,
    min_repos: usize,
) -> String {
    format!(
        "corpus coverage shrank silently: {samples}≥{min_samples} samples but only \
         {repos} distinct repos (<{min_repos}) — breadth insufficient (a fetch chain that \
         only deep-dives a few repos; the sample-count sentinel cannot see this); for \
         targeted validation use per_rule_hits as the signal"
    )
}

/// FN 违例 (完全漏检, 召回优先不变量)。
pub fn note_fn() -> &'static str {
    "failed samples with zero failure-intent fragments (complete miss; recall-priority \
     invariant)"
}

/// FN 违例 detail 截断形态: 总数 + 上限 (verdict JSON 防膨胀, 定位不截断于 summary 侧)。
pub fn note_fn_capped(total: usize, cap: usize) -> String {
    format!(
        "{} — {total} FN sample(s) in total, detail capped at {cap}, listing the first {cap}",
        note_fn()
    )
}

/// FP 率违例。
pub fn note_fp_rate(rate_pct: f64, fp_max_pct: f64) -> String {
    format!(
        "failure-intent false-positive rate on success samples exceeds the threshold \
         ({rate_pct:.2}% >{fp_max_pct}%)"
    )
}

/// FP 率违例 detail 截断形态。
pub fn note_fp_rate_capped(rate_pct: f64, fp_max_pct: f64, pairs: usize, cap: usize) -> String {
    format!(
        "{} — {pairs} offending (rule, sample) pair(s) in total, detail capped at {cap}, \
         listing the first {cap}",
        note_fp_rate(rate_pct, fp_max_pct)
    )
}

/// 边界完整性哨兵违例 (按状态机构造恒真 — 非零 = 引擎 bug)。
pub fn note_boundary() -> &'static str {
    "a block's tail line is not matched by any pattern of its rule (A4 boundary-integrity \
     sentinel, true by construction — nonzero = engine bug)"
}

/// 零块违例 (规则集对该日志失明 — 结构承诺)。
pub fn note_zero_block() -> &'static str {
    "zero-block sample(s) present — the ruleset is blind to that log (structural promise)"
}

/// 死规则违例 (--require-all-rules-hit 正向验证)。
pub fn note_dead_rules(n: usize) -> String {
    format!(
        "{n} rule(s) with zero corpus hits (positive validation: a typo'd regex must not \
         merge all-green, issue #19)"
    )
}

/// 双测漂移通知 (repo 仅在 PR 侧, 不判红)。
pub fn drift_notice(repo: &str) -> String {
    format!("{repo}: missing on the base side (corpus drift?); needs manual review")
}

/// 双测对比模式标签。
pub const COMPARE_MODE_DUAL: &str = "dual-run compare";
/// 单测模式标签 (未提供 base, 压缩率对比跳过)。
pub const COMPARE_MODE_SINGLE: &str =
    "single run (no base summary provided — compression comparison skipped; CI should \
     supply base to enable the regression sentinel)";

/// stderr 单行摘要 (issue #74: 阈值对照随行 — 零阈值族 `值/上限`, 方向性族 `<=`/`>=`)。
#[allow(clippy::too_many_arguments)]
pub fn l0_summary_line(
    verdict: &str,
    fn_count: u64,
    fp_rate_pct: f64,
    fp_max_pct: f64,
    zero_blocks: u64,
    boundary: u64,
    samples: u64,
    min_samples: usize,
    repos: u64,
    min_repos: usize,
    comp_blk_pct: f64,
    comp_sig_pct: f64,
    dead_rules: usize,
) -> String {
    format!(
        "l0-gate: verdict={verdict} FN={fn_count}/0 FP={fp_rate_pct:.2}%<={fp_max_pct}% \
         zero-blocks={zero_blocks}/0 boundary={boundary}/0 samples={samples}>={min_samples} \
         repos={repos}>={min_repos} comp blk={comp_blk_pct:.2}% sig={comp_sig_pct:.2}% \
         dead-rules={dead_rules}"
    )
}

/// exempt 组状态单行 (豁免余量 = 适配进度度量; N=0 静默)。
pub fn l0_exempt_line(n: u64, repos: u64) -> String {
    format!(
        "l0: exempt: {n} sample(s) across {repos} repo(s) not asserted (sidecar \
         gate_exempt — moved out once adapted; exemption headroom = adaptation progress)"
    )
}
