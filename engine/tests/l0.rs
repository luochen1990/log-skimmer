//! L0 门禁口径与判定回归 — 判定逻辑的可测试化是 l0 子命令化的核心动机
//! (bash/jq 时代零测试, 破坏验证全靠手工)。断言族语义见 engine/src/l0.rs 头注释。

use log_skimmer::l0::{self, Agg, Summary};
use std::collections::BTreeMap;
use std::path::PathBuf;

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
    let v = l0::gate(&s, None, false);
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

/// 判定: 语料在场不足 → corpus-missing (防空转假绿 — 断言族在空语料上拒绝判定)。
#[test]
fn gate_fails_on_missing_corpus() {
    let mut s = ok_summary();
    s.structural.samples_total = l0::MIN_SAMPLES - 1;
    let v = l0::gate(&s, None, false);
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
