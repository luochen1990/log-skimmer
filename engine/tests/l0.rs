//! L0 门禁口径与判定回归 — 判定逻辑的可测试化是 l0 子命令化的核心动机
//! (bash/jq 时代零测试, 破坏验证全靠手工)。断言族语义见 engine/src/l0.rs 头注释。

use log_skimmer::l0::{self, Agg, Summary};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// 口径: 采集→汇总 三态/excluded/fp 分子分母/压缩率聚合 (fixture 小语料, 数字逐项断言)。
#[test]
fn collect_and_summarize_semantics() {
    let c = l0::collect(&fixtures().join("l0-corpus"), &fixtures().join("rules"))
        .expect("fixture 语料采集失败");
    let samples = &c.samples;
    assert_eq!(samples.len(), 3, "3 个样本全在场 (sidecar+log 配对)");
    assert_eq!(
        c.rule_ids.len(),
        4,
        "装载规则全集随采集返回 (per_rule_hits 定义域)"
    );

    let s = l0::summarize(&c);
    // excluded (attempt=2 success) 滤除主指标, 但全量统计仍计
    assert_eq!(s.structural.samples_total, 3);
    assert_eq!(s.structural.samples_excluded, 1);
    assert_eq!(s.totals.runs, 2, "主指标仅含未排除样本");

    // failure 样本 (error: 行命中 fixture nix 规则) — FN=0
    assert_eq!(s.totals.fn_samples, 0);
    // success 样本含 ##[error] 行 (fixture gha 规则) → fp_frags≥1, fp_lines>0
    assert!(s.totals.fp_frags >= 1, "success 样本的 failure 命中计入 FP");
    assert!(s.totals.fp_lines >= 1);
    // fp 率 = fp_lines / success 输入行 (attempt=2 的 success 行不计入分母)
    let success_input: u64 = samples
        .iter()
        .filter(|x| x.conclusion == "success" && !x.excluded)
        .map(|x| x.input_lines as u64)
        .sum();
    assert_eq!(s.totals.success_input_lines, success_input);
    let want_fp_pct = 100.0 * s.totals.fp_lines as f64 / success_input as f64;
    assert!(
        (s.totals.fp_rate_pct - want_fp_pct).abs() < 1e-9,
        "FP 率分母=success 输入行"
    );

    // 压缩率 = 聚合行数比 (主指标域); 报告口径两位小数收敛 (issue #74:
    // f64 全精度尾巴在 JSON/stderr 两侧均无信息量, 收敛于计算点单点生效)
    let (i, o, sig) = (
        s.totals.input_lines,
        s.totals.output_lines,
        s.totals.signature_output_lines,
    );
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    assert!((s.totals.comp_blk_pct - r2(100.0 * (i - o) as f64 / i as f64)).abs() < 1e-9);
    assert!((s.totals.comp_sig_pct - r2(100.0 * (i - sig) as f64 / i as f64)).abs() < 1e-9);
    // 性质: 值 ×100 近整数 = 至多两位小数 (7 行分母的比值天然带长尾,
    // 未收敛的原值在此断言下必红)
    assert!((s.totals.comp_blk_pct * 100.0).fract().abs() < 1e-9);
    assert!((s.totals.comp_sig_pct * 100.0).fract().abs() < 1e-9);
}

/// 判定: 合规 summary → pass (断言族全绿, 单测模式)。
#[test]
fn gate_pass_on_compliant_summary() {
    let mut s = ok_summary();
    let v = l0::gate(&s, None, false);
    assert_eq!(v.verdict, "pass");
    assert!(v.violations.is_empty());
    assert!(!v.compression_compare.base_fail);
    assert!(v.compression_compare.mode.contains("单测"));
    s.totals.fn_samples = 0; // clippy: s 有后续读者前的确定性
    let _ = s;
}

/// stderr 单行摘要自解释 (issue #74): pass 时验收线也一眼可见 — 零阈值断言族
/// `值/上限`, 方向性断言 `≤/≥`; 阈值字面量从常量派生 (改常量断言同步变)。
#[test]
fn gate_summary_line_pairs_values_with_thresholds() {
    let s = ok_summary();
    let v = l0::gate(&s, None, false);
    assert_eq!(v.verdict, "pass");
    let line = v.summary_line();
    assert!(line.contains("verdict=pass"), "{line}");
    assert!(line.contains("FN=0/0"), "{line}");
    assert!(
        line.contains(&format!("FP率=0.00%≤{}%", l0::FP_MAX_PCT)),
        "{line}"
    );
    assert!(line.contains("零块=0/0"), "{line}");
    assert!(line.contains("边界违例=0/0"), "{line}");
    assert!(
        line.contains(&format!(
            "语料={}≥{}",
            s.structural.samples_total,
            l0::MIN_SAMPLES
        )),
        "{line}"
    );
    // fail 形态同格式: 超标值与验收线同在, 一眼可见差距
    let mut bad = ok_summary();
    bad.totals.fn_samples = 2;
    bad.totals.fp_rate_pct = l0::FP_MAX_PCT + 0.5;
    let v = l0::gate(&bad, None, false);
    assert_eq!(v.verdict, "fail");
    let line = v.summary_line();
    assert!(line.contains("verdict=fail"), "{line}");
    assert!(line.contains("FN=2/0"), "{line}");
    assert!(line.contains(&format!("≤{}%", l0::FP_MAX_PCT)), "{line}");
}

/// 判定: FN>0 → fail + fn violation (spec §0 召回优先)。
#[test]
fn gate_fails_on_fn() {
    let mut s = ok_summary();
    s.totals.fn_samples = 1;
    let v = l0::gate(&s, None, false);
    assert_eq!(v.verdict, "fail");
    assert!(v.violations.iter().any(|x| x.kind == "fn"));
}

/// 判定: FP 率超阈 → fail (分母口径 = success 输入行)。
#[test]
fn gate_fails_on_fp_rate() {
    let mut s = ok_summary();
    s.totals.fp_rate_pct = l0::FP_MAX_PCT + 0.1;
    let v = l0::gate(&s, None, false);
    assert_eq!(v.verdict, "fail");
    assert!(v.violations.iter().any(|x| x.kind == "fp-rate"));
}

/// 判定: 语料在场不足 (0<n<50; CLI 路径采集层已保证 n≥1) → fail — 防空转下限;
/// note 分流两种读法 (issue #44): 定向验证小语料属预期 (正向信号 per_rule_hits),
/// 全量语料下才是获取链路故障 — 不可单指控后者误导定向验证的贡献者。
#[test]
fn gate_fails_on_missing_corpus() {
    let mut s = ok_summary();
    s.structural.samples_total = l0::MIN_SAMPLES - 1;
    let v = l0::gate(&s, None, false);
    assert_eq!(v.verdict, "fail");
    let viol = v
        .violations
        .iter()
        .find(|x| x.kind == "corpus-missing")
        .expect("corpus-missing violation 在场");
    assert!(viol.note.contains("定向验证"), "note: {}", viol.note);
    assert!(viol.note.contains("获取链路"), "note: {}", viol.note);
    assert!(
        viol.note.contains("per_rule_hits"),
        "note 应指向定向验证的正向信号: {}",
        viol.note
    );
}

/// 采集: 语料目录不存在 = 语料非法 (与规则目录对称, load_rules 同款 Err) —
/// CLI 侧映射 exit 2 (issue #44: 采集层面失败不是质量门禁红)。
#[test]
fn collect_missing_corpus_dir_is_err() {
    let e = l0::collect(
        &PathBuf::from("/nonexistent-corpus-dir-l0-test"),
        &fixtures().join("rules"),
    )
    .unwrap_err();
    assert!(e.contains("语料目录不存在"), "Err 应点明目录不存在: {e}");
}

/// 判定: 边界违例 note 如实描述 A4 块尾行哨兵 (issue #43) — spec §4 边界完整性
/// 检查的是「block 尾行仍被该规则任一 pattern 匹配」, 与 context 取值域无关;
/// 不得误述为 "context 越界" 或内嵌孤立的 [0,10] 字面量 (CONTEXT_MAX 已单源)。
#[test]
fn gate_boundary_note_matches_a4_check() {
    let mut s = ok_summary();
    s.structural.boundary_violations = 1;
    let v = l0::gate(&s, None, false);
    assert_eq!(v.verdict, "fail");
    let viol = v
        .violations
        .iter()
        .find(|x| x.kind == "boundary")
        .expect("boundary violation 在场");
    assert!(viol.note.contains("尾行"), "note: {}", viol.note);
    assert!(!viol.note.contains("[0,10]"), "note: {}", viol.note);
    assert!(!viol.note.contains("context"), "note: {}", viol.note);
}

/// 判定: 双测压缩率退化 >2% → fail; base 缺 repo → drift 通知不判红。
#[test]
fn gate_dual_test_regression_and_drift() {
    let mut pr = ok_summary();
    let mut base = ok_summary();
    // repo R: base 98% → pr 95% (delta = -3.06% < -2%) → regression
    pr.per_repo
        .get_mut("github.com/r/kept")
        .unwrap()
        .comp_blk_pct = 95.0;
    base.per_repo
        .get_mut("github.com/r/kept")
        .unwrap()
        .comp_blk_pct = 98.0;
    // repo NEW 仅在 pr 侧 → drift notice, 不判红
    pr.per_repo
        .insert("github.com/r/new".into(), Agg::default());
    let v = l0::gate(&pr, Some(&base), false);
    assert_eq!(v.compression_compare.mode, "双测对比");
    assert_eq!(v.compression_compare.regressions.len(), 1);
    assert_eq!(
        v.compression_compare.regressions[0].repo,
        "github.com/r/kept"
    );
    assert!(!v.compression_compare.drift_notices.is_empty());
    assert!(v.compression_compare.base_fail);
    assert_eq!(v.verdict, "fail");
}

/// 判定: 双测无退化 → pass (改善方向不报)。
#[test]
fn gate_dual_test_improvement_passes() {
    let mut pr = ok_summary();
    let mut base = ok_summary();
    pr.per_repo
        .get_mut("github.com/r/kept")
        .unwrap()
        .comp_blk_pct = 99.0; // 改善
    base.per_repo
        .get_mut("github.com/r/kept")
        .unwrap()
        .comp_blk_pct = 98.0;
    let v = l0::gate(&pr, Some(&base), false);
    assert_eq!(v.verdict, "pass");
    assert!(v.compression_compare.regressions.is_empty());
}

/// 配对可见性 (issue #46): 单侧在场样本 (fixture 钉双侧缺口形态) 不再静默 — 进
/// Collected.orphans (相对路径 + 缺失侧), 不进 samples; 配对成功样本计数不变。
#[test]
fn collect_reports_orphan_samples() {
    let c = l0::collect(&fixtures().join("l0-corpus"), &fixtures().join("rules"))
        .expect("fixture 语料采集失败");
    assert_eq!(c.samples.len(), 3, "孤儿不进 samples, 配对样本计数不变");
    assert_eq!(
        c.orphans,
        vec![
            l0::OrphanSample {
                path: "github.com/x/y/2026-10-16/orphan-no-log.log.json".into(),
                missing: l0::MissingSide::Log,
            },
            l0::OrphanSample {
                path: "github.com/x/y/2026-10-16/orphan-no-sidecar.log".into(),
                missing: l0::MissingSide::Sidecar,
            },
        ],
        "双侧配对缺口都显形 (路径字典序)"
    );
}

/// 正向验证口径 (issue #19 B): per_rule_hits 定义域 = 装载规则全集;
/// signal = fragment 数 (全样本域, 含 excluded — 仅被 rerun 样本命中不是死规则),
/// noise = 剔除区块数 (noise 不产 fragment), 零值 = 语料上未生效。
#[test]
fn per_rule_hits_counts_and_dead_rules() {
    let c = l0::collect(
        &fixtures().join("l0-corpus"),
        &fixtures().join("l0-rules-hits"),
    )
    .expect("fixture 语料采集失败");
    let s = l0::summarize(&c);
    let want: BTreeMap<String, u64> = [
        ("hits-error-anchor", 2u64), // 样本 1 (main) + 样本 3 (excluded) 各 1 锚
        ("note-noise", 1),           // 样本 1 剔除 1 区块 (noise 无 fragment)
        ("dead-never-match", 0),     // 恒不匹配 → 零命中补零在域
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(
        s.per_rule_hits, want,
        "计数含 excluded 样本 (liveness), noise 以区块计, 死规则补零"
    );
}

/// 判定: dead_rules 恒报告 (报告形态), 默认不判红 — verdict 语义不变 (issue #19)。
#[test]
fn gate_reports_dead_rules_without_failing_by_default() {
    let mut s = ok_summary();
    s.per_rule_hits.insert("alive-rule".into(), 3);
    s.per_rule_hits.insert("dead-rule".into(), 0);
    let v = l0::gate(&s, None, false);
    assert_eq!(v.verdict, "pass", "默认开关下死规则不改变 verdict");
    assert_eq!(
        v.stats["dead_rules"],
        serde_json::json!(["dead-rule"]),
        "死规则列表恒进报告"
    );
    assert_eq!(v.stats["per_rule_hits"]["dead-rule"], 0);
}

/// 判定: --require-all-rules-hit 开启 → 死规则 fail 并列出; 全命中 → pass;
/// 空域 (per_rule_hits 缺省, 如旧 base summary 形态) 视为无可断言。
#[test]
fn gate_require_all_rules_hit() {
    let mut s = ok_summary();
    s.per_rule_hits.insert("alive-rule".into(), 3);
    s.per_rule_hits.insert("dead-rule".into(), 0);
    let v = l0::gate(&s, None, true);
    assert_eq!(v.verdict, "fail");
    let viol = v
        .violations
        .iter()
        .find(|x| x.kind == "dead-rules")
        .expect("dead-rules violation 在场");
    assert_eq!(viol.detail, Some(serde_json::json!(["dead-rule"])));

    s.per_rule_hits.insert("dead-rule".into(), 1); // 复活
    let v = l0::gate(&s, None, true);
    assert_eq!(v.verdict, "pass");

    // 空域 (per_rule_hits 缺省, 如旧 base summary 形态) → 无可断言
    let v = l0::gate(&ok_summary(), None, true);
    assert_eq!(v.verdict, "pass");
}

/// P0-2 (走查): sidecar 缺失 attempt — 统计语义不变 (按 attempt=1 处理), 缺口
/// 可见化: 采集聚合计数 + 单行警告渲染 (计数 N 与 按 1 处理说明必须在场);
/// success 样本不因字段缺失被误排 (rerun-through 判定 attempt>1 不触发)。
#[test]
fn collect_missing_attempt_defaults_to_one_and_warns() {
    let c = l0::collect(
        &fixtures().join("l0-attempt-corpus"),
        &fixtures().join("rules"),
    )
    .expect("fixture 语料采集失败");
    assert_eq!(
        c.sidecars_missing_attempt, 1,
        "缺失 attempt 的 sidecar 聚合计数 (非逐条)"
    );
    let missing = c
        .samples
        .iter()
        .find(|s| s.sample.ends_with("10-10-build-success.log"))
        .expect("缺失 attempt 样本在场");
    assert_eq!(missing.attempt, 1, "按 attempt=1 处理 (统计语义不变)");
    assert!(
        !missing.excluded,
        "success + 缺失 attempt 不触发 rerun 排除"
    );
    let warn = c
        .attempt_missing_warning()
        .expect("缺口在场时警告渲染非 None");
    assert!(!warn.contains('\n'), "单行警告: {warn}");
    assert!(warn.contains("1 个"), "计数 N 在场: {warn}");
    assert!(warn.contains("attempt=1"), "按 1 处理说明在场: {warn}");
    assert!(warn.contains("§5"), "指向规范必带要求: {warn}");
}

/// P0-2 回归保护 + 静默条件: attempt>1 且 success → excluded (rerun-through
/// 口径不回归); 全部带 attempt → 零计数无警告 (N=0 静默, 警告只对真实缺口发出)。
#[test]
fn collect_all_attempts_present_no_warning_and_rerun_excluded() {
    let c = l0::collect(&fixtures().join("l0-corpus"), &fixtures().join("rules"))
        .expect("fixture 语料采集失败");
    assert_eq!(c.sidecars_missing_attempt, 0, "fixture 全带 attempt");
    assert!(c.attempt_missing_warning().is_none(), "N=0 不输出警告");
    let rerun = c
        .samples
        .iter()
        .find(|s| s.sample.ends_with("3-3-check-success.log"))
        .expect("attempt=2 success 样本在场");
    assert!(rerun.excluded, "attempt>1 且 success → rerun-through 排除");
}

// ---- gz 压缩形态 (spec §5): 配对名 = 样本文件名去 .gz 后缀 ----

/// 临时语料目录 (同 cli_contract 惯例: ls-<套件>-<名>, 复跑先清)。
fn gz_tmp_corpus(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ls-l0-gz-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 现场写出 gzip 样本 (gz 数据不入库二进制 fixture)。
fn gz_write(path: &Path, content: &str) {
    let mut w = flate2::write::GzEncoder::new(
        std::fs::File::create(path).unwrap(),
        flate2::Compression::default(),
    );
    std::io::Write::write_all(&mut w, content.as_bytes()).unwrap();
    w.finish().unwrap();
}

/// 三样本小语料 (failure / success 含 FP / rerun-through), 按位掩码决定每样本
/// 存 plain (*.log) 还是压缩 (*.log.gz) 形态 — sidecar 恒为 配对名 + .json。
fn gz_build_corpus(d: &Path, gz_mask: &[bool]) {
    let samples: &[(&str, &str, u32, &str)] = &[
        ("f1", "failure", 1, "error: boom\nplain noise line\n"),
        (
            "s1",
            "success",
            1,
            "Runner Image: ubuntu\n##[error] flaky fp\n",
        ),
        ("s2", "success", 2, "error: transient on attempt 1\n"),
    ];
    for ((name, conclusion, attempt, body), gz) in samples.iter().zip(gz_mask) {
        if *gz {
            gz_write(&d.join(format!("{name}.log.gz")), body);
        } else {
            std::fs::write(d.join(format!("{name}.log")), body).unwrap();
        }
        std::fs::write(
            d.join(format!("{name}.log.json")),
            format!("{{\"repo\":\"r/x\",\"conclusion\":\"{conclusion}\",\"attempt\":{attempt}}}"),
        )
        .unwrap();
    }
}

/// 同一批样本 全 plain / 全 gz / 混合 三形态: l0 全部指标 (Summary 整体)
/// 完全一致 — 压缩是存储细节, 不得改变任何指标; 采集不因形态产生孤儿。
#[test]
fn gz_and_plain_corpus_metrics_identical() {
    let summary_json = |gz_mask: &[bool]| {
        let d = gz_tmp_corpus("equiv");
        gz_build_corpus(&d, gz_mask);
        let c = l0::collect(&d, &fixtures().join("rules")).expect("采集失败");
        assert_eq!(c.samples.len(), 3, "三形态均应配满 3 样本 (无孤儿)");
        assert!(c.orphans.is_empty(), "orphans: {:?}", c.orphans);
        serde_json::to_value(l0::summarize(&c)).unwrap()
    };
    let plain = summary_json(&[false, false, false]);
    assert_eq!(
        plain,
        summary_json(&[true, true, true]),
        "全 gz 与全 plain 指标须完全一致"
    );
    assert_eq!(
        plain,
        summary_json(&[true, false, true]),
        "混合语料与全 plain 指标须完全一致"
    );
}

/// 配对可见性扩展到压缩形态: X.log.gz + X.log.json 配对成功;
/// X.log.gz 单侧在场 = 孤儿 (缺 sidecar); sidecar 在场而 X.log/X.log.gz
/// 均缺 = 孤儿 (缺日志, 既有语义不变)。
#[test]
fn gz_sidecar_pairing_and_orphans() {
    let d = gz_tmp_corpus("orphans");
    gz_write(&d.join("a.log.gz"), "error: paired\n");
    std::fs::write(
        d.join("a.log.json"),
        r#"{"repo":"r/x","conclusion":"failure"}"#,
    )
    .unwrap();
    gz_write(&d.join("b.log.gz"), "error: no sidecar\n");
    std::fs::write(
        d.join("c.log.json"),
        r#"{"repo":"r/x","conclusion":"failure"}"#,
    )
    .unwrap();
    let c = l0::collect(&d, &fixtures().join("rules")).expect("采集失败");
    assert_eq!(c.samples.len(), 1, "a.log.gz 与 a.log.json 配对成功");
    let orphans: Vec<_> = c
        .orphans
        .iter()
        .map(|o| (o.path.as_str(), o.missing))
        .collect();
    assert_eq!(
        orphans,
        vec![
            ("b.log.gz", l0::MissingSide::Sidecar),
            ("c.log.json", l0::MissingSide::Log),
        ],
        "gz 单侧在场进孤儿清单, 缺失侧标注正确"
    );
}

/// 同配对名 plain 与 gz 双份在场 (X.log + X.log.gz + sidecar) = 语料非法:
/// 配对歧义不可静默择一 (另一份样本会被无声吞掉), fail-fast 报双路径。
#[test]
fn duplicate_pairing_name_fails_fast() {
    let d = gz_tmp_corpus("conflict");
    std::fs::write(d.join("x.log"), "error: plain\n").unwrap();
    gz_write(&d.join("x.log.gz"), "error: gz\n");
    std::fs::write(
        d.join("x.log.json"),
        r#"{"repo":"r/x","conclusion":"failure"}"#,
    )
    .unwrap();
    let err = l0::collect(&d, &fixtures().join("rules")).unwrap_err();
    assert!(
        err.contains("x.log") && err.contains("x.log.gz"),
        "报错须含 plain 与 gz 双路径: {err}"
    );
    assert!(err.contains("冲突"), "报错须点明冲突语义: {err}");
}

/// 解压失败经 l0 采集路径 fail-fast: 非法 gz 样本 → Err 带样本路径,
/// 不静默跳样本 (跳样本 = 门禁语料面静默缩水)。
#[test]
fn collect_corrupt_gz_fails_with_path() {
    let d = gz_tmp_corpus("corrupt");
    std::fs::write(d.join("z.log.gz"), "plain text mislabeled as gz").unwrap();
    std::fs::write(
        d.join("z.log.json"),
        r#"{"repo":"r/x","conclusion":"failure"}"#,
    )
    .unwrap();
    let err = l0::collect(&d, &fixtures().join("rules")).unwrap_err();
    assert!(err.contains("z.log.gz"), "采集报错须带样本路径: {err}");
    assert!(
        err.contains("日志读取失败"),
        "与既有读取失败文案同族: {err}"
    );
}

fn ok_summary() -> Summary {
    let agg = |blk: f64| Agg {
        runs: 30,
        input_lines: 10_000,
        output_lines: 200,
        signature_output_lines: 80,
        comp_blk_pct: blk,
        comp_sig_pct: 99.0,
        ..Default::default()
    };
    let mut s = Summary {
        totals: agg(98.0),
        structural: l0::Structural {
            samples_total: 60,
            ..Default::default()
        },
        ..Default::default()
    };
    s.per_repo.insert("github.com/r/kept".into(), agg(98.0));
    s
}
