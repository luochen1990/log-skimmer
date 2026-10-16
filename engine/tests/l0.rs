//! L0 门禁口径与判定回归 — 判定逻辑的可测试化是 l0 子命令化的核心动机
//! (bash/jq 时代零测试, 破坏验证全靠手工)。断言族语义见 engine/src/l0.rs 头注释。

use log_skimmer::l0::{self, Agg, Summary};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// 口径: 采集→汇总 三态/excluded/fp 分子分母/压缩率聚合 (fixture 小语料, 数字逐项断言)。
#[test]
fn collect_and_summarize_semantics() {
    let samples = l0::collect(&fixtures().join("l0-corpus"), &fixtures().join("rules"))
        .expect("fixture 语料采集失败");
    assert_eq!(samples.len(), 3, "3 个样本全在场 (sidecar+log 配对)");

    let s = l0::summarize(&samples);
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

    // 压缩率 = 聚合行数比 (主指标域)
    let (i, o, sig) = (
        s.totals.input_lines,
        s.totals.output_lines,
        s.totals.signature_output_lines,
    );
    assert!((s.totals.comp_blk_pct - 100.0 * (i - o) as f64 / i as f64).abs() < 1e-9);
    assert!((s.totals.comp_sig_pct - 100.0 * (i - sig) as f64 / i as f64).abs() < 1e-9);
}

/// 判定: 合规 summary → pass (断言族全绿, 单测模式)。
#[test]
fn gate_pass_on_compliant_summary() {
    let mut s = ok_summary();
    let v = l0::gate(&s, None);
    assert_eq!(v.verdict, "pass");
    assert!(v.violations.is_empty());
    assert!(!v.compression_compare.base_fail);
    assert!(v.compression_compare.mode.contains("单测"));
    s.totals.fn_samples = 0; // clippy: s 有后续读者前的确定性
    let _ = s;
}

/// 判定: FN>0 → fail + fn violation (spec §0 召回优先)。
#[test]
fn gate_fails_on_fn() {
    let mut s = ok_summary();
    s.totals.fn_samples = 1;
    let v = l0::gate(&s, None);
    assert_eq!(v.verdict, "fail");
    assert!(v.violations.iter().any(|x| x.kind == "fn"));
}

/// 判定: FP 率超阈 → fail (分母口径 = success 输入行)。
#[test]
fn gate_fails_on_fp_rate() {
    let mut s = ok_summary();
    s.totals.fp_rate_pct = l0::FP_MAX_PCT + 0.1;
    let v = l0::gate(&s, None);
    assert_eq!(v.verdict, "fail");
    assert!(v.violations.iter().any(|x| x.kind == "fp-rate"));
}

/// 判定: 语料在场不足 → corpus-missing (防空转假绿 — 断言族在空语料上拒绝判定)。
#[test]
fn gate_fails_on_missing_corpus() {
    let mut s = ok_summary();
    s.structural.samples_total = l0::MIN_SAMPLES - 1;
    let v = l0::gate(&s, None);
    assert_eq!(v.verdict, "fail");
    assert!(v.violations.iter().any(|x| x.kind == "corpus-missing"));
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
    let v = l0::gate(&pr, Some(&base));
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
    let v = l0::gate(&pr, Some(&base));
    assert_eq!(v.verdict, "pass");
    assert!(v.compression_compare.regressions.is_empty());
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
