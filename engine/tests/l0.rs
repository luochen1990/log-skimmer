//! L0 门禁口径与判定回归 — 判定逻辑的可测试化是 l0 子命令化的核心动机
//! (bash/jq 时代零测试, 破坏验证全靠手工)。断言族语义见 engine/src/l0.rs 头注释。

use log_skimmer::l0::{self, Agg, FragRec, L0Sample, Summary};
use log_skimmer::Intent;
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
    assert_eq!(
        s.structural.repos_total, 1,
        "仓维度哨兵 = 去重 repo 数 (全样本域)"
    );
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
    assert!(v.compression_compare.mode.contains("single run"));
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
        line.contains(&format!("FP={:.2}%<={}%", 0.0, l0::FP_MAX_PCT)),
        "{line}"
    );
    assert!(line.contains("zero-blocks=0/0"), "{line}");
    assert!(line.contains("boundary=0/0"), "{line}");
    assert!(
        line.contains(&format!(
            "samples={}>={}",
            s.structural.samples_total,
            l0::MIN_SAMPLES
        )),
        "{line}"
    );
    assert!(
        line.contains(&format!(
            "repos={}>={}",
            s.structural.repos_total,
            l0::MIN_REPOS
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
    assert!(line.contains(&format!("<={}%", l0::FP_MAX_PCT)), "{line}");
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

/// FN violation detail 逐样本点名: 1 个 FN 样本 + 1 个正常失败样本 (有提取)
/// → detail 精确列出该样本 (repo/相对路径/输入行数); FN=0 → 无 fn violation。
/// 引擎算术内存直测 (lit() 构造器, AGENTS 语料边界纪律)。
#[test]
fn gate_fn_violation_detail_lists_samples() {
    let c = l0::Collected {
        rule_tools: BTreeMap::new(),
        samples: vec![
            // FN 样本: failure 且零 failure-intent fragment (lit 全零指标)
            L0Sample {
                input_lines: 42,
                ..lit("github.com/r/a", "failure", false)
            },
            // 正常失败样本: 有提取, 不进 FN 清单
            L0Sample {
                input_lines: 3,
                failure_frags: 1,
                failure_frag_lines: 1,
                ..lit("github.com/r/b", "failure", false)
            },
        ],
        rule_ids: vec![],
        orphans: vec![],
        sidecars_missing_attempt: 0,
    };
    let s = l0::summarize(&c);
    assert_eq!(s.totals.fn_samples, 1);
    assert_eq!(s.fn_detail.len(), 1, "清单长度 ≡ totals.fn_samples");
    let v = l0::gate(&s, None, false);
    let viol = v
        .violations
        .iter()
        .find(|x| x.kind == "fn")
        .expect("FN>0 必有 fn violation");
    let detail: Vec<l0::FnDetail> = serde_json::from_value(viol.detail.clone().unwrap())
        .expect("detail 可反序列化为逐样本清单");
    assert_eq!(
        detail,
        vec![l0::FnDetail {
            repo: "github.com/r/a".into(),
            sample: "github.com/r/a/failure.log".into(),
            input_lines: 42,
        }],
        "detail 精确点名 FN 样本"
    );

    // FN=0 (失败样本有提取) → 无 fn violation, fn_detail 空
    let mut c0 = c;
    c0.samples = vec![L0Sample {
        failure_frags: 1,
        failure_frag_lines: 1,
        ..lit("github.com/r/a", "failure", false)
    }];
    let s0 = l0::summarize(&c0);
    assert_eq!(s0.totals.fn_samples, 0);
    assert!(s0.fn_detail.is_empty());
    let v0 = l0::gate(&s0, None, false);
    assert!(
        v0.violations.iter().all(|x| x.kind != "fn"),
        "FN=0 不产 fn violation"
    );
}

/// FN detail 极端多: 60 个 gated FN 样本 → detail 截断至 FN_DETAIL_MAX, note
/// 携带总数与上限 (fn-responder 定位不因截断失真), 截断保持构造序稳定。
#[test]
fn gate_fn_detail_caps_with_total_in_note() {
    let samples: Vec<L0Sample> = (0..60)
        .map(|i| lit(&format!("github.com/r/{i}"), "failure", false))
        .collect();
    let s = l0::summarize(&l0::Collected {
        rule_tools: BTreeMap::new(),
        samples,
        rule_ids: vec![],
        orphans: vec![],
        sidecars_missing_attempt: 0,
    });
    assert_eq!(s.totals.fn_samples, 60);
    assert_eq!(s.fn_detail.len(), 60, "summary 侧清单不截断");
    let v = l0::gate(&s, None, false);
    let viol = v.violations.iter().find(|x| x.kind == "fn").unwrap();
    let detail: Vec<l0::FnDetail> = serde_json::from_value(viol.detail.clone().unwrap()).unwrap();
    assert_eq!(detail.len(), l0::FN_DETAIL_MAX, "verdict detail 有上限");
    assert_eq!(detail[0].repo, "github.com/r/0");
    assert!(viol.note.contains("60"), "note 携带总数: {}", viol.note);
    assert!(
        viol.note.contains(&l0::FN_DETAIL_MAX.to_string()),
        "note 携带上限: {}",
        viol.note
    );
}

/// serde 兼容: detail 新类型 Vec<FnDetail> 从旧 verdict 的空数组 `[]` 直接
/// 反序列化为空 Vec — 顶层恒数组, 旧消费方按数组解析零断裂。
#[test]
fn fn_detail_deserializes_legacy_empty_array() {
    let empty: Vec<l0::FnDetail> = serde_json::from_str("[]").unwrap();
    assert!(empty.is_empty());
}

/// FP violation detail 逐 (规则, 样本) 对定位 (fp-responder 迭代环机读面):
/// success 样本上 failure-intent fragment → 对清单 (样本内按规则聚合, 计数/
/// 行数累加); warning 命中与失败家族样本的命中不计 FP; 豁免样本的 FP 不进
/// 断言细节 (gated 域)。引擎算术内存直测 (lit() 构造器, 显式数字逐项对账)。
#[test]
fn gate_fp_violation_detail_lists_rule_sample_pairs() {
    let c = l0::Collected {
        rule_tools: BTreeMap::new(),
        samples: vec![
            // gated success 样本: fp-rule 两 fragment (1+2 行) 聚成一对;
            // warn-rule 命中属预期, 不进 FP 对清单
            L0Sample {
                input_lines: 10,
                failure_frags: 2,
                failure_frag_lines: 3,
                frags: vec![
                    FragRec {
                        rule: "fp-rule".into(),
                        intent: Intent::Failure,
                        lines: 1,
                    },
                    FragRec {
                        rule: "fp-rule".into(),
                        intent: Intent::Failure,
                        lines: 2,
                    },
                    FragRec {
                        rule: "warn-rule".into(),
                        intent: Intent::Warning,
                        lines: 5,
                    },
                ],
                sample: "r/a/s1.log".into(),
                ..lit("r/a", "success", false)
            },
            // 失败家族样本上的 fp-rule 命中是真信号, 不进 FP 对清单
            L0Sample {
                failure_frags: 1,
                failure_frag_lines: 1,
                frags: vec![FragRec {
                    rule: "fp-rule".into(),
                    intent: Intent::Failure,
                    lines: 1,
                }],
                ..lit("r/a", "failure", false)
            },
            // 豁免 success 样本的 FP 不进断言细节 (gated 域)
            L0Sample {
                failure_frags: 1,
                failure_frag_lines: 1,
                frags: vec![FragRec {
                    rule: "exempt-rule".into(),
                    intent: Intent::Failure,
                    lines: 1,
                }],
                ..lit("r/b", "success", true)
            },
        ],
        rule_ids: vec![],
        orphans: vec![],
        sidecars_missing_attempt: 0,
    };
    let s = l0::summarize(&c);
    // FP 率分子/分母 (gated 域): 3 行 / 10 success 输入行 = 30% > 2%
    assert_eq!(s.totals.success_input_lines, 10);
    assert_eq!(s.totals.fp_lines, 3);
    let want = vec![l0::FpDetail {
        rule: "fp-rule".into(),
        repo: "r/a".into(),
        sample: "r/a/s1.log".into(),
        fp_frags: 2,
        fp_lines: 3,
    }];
    assert_eq!(
        s.fp_detail, want,
        "对清单 = gated success 域 failure-intent 命中, 样本内按规则聚合"
    );
    // 一致性 (性质): 单对行数分解之和 ≡ totals.fp_lines (gated FP 分子)
    assert_eq!(
        s.fp_detail.iter().map(|d| d.fp_lines).sum::<u64>(),
        s.totals.fp_lines
    );
    let v = l0::gate(&s, None, false);
    let viol = v
        .violations
        .iter()
        .find(|x| x.kind == "fp-rate")
        .expect("FP 率超阈必有 fp-rate violation");
    assert!(viol.note.contains("30.00%"), "note: {}", viol.note);
    let detail = viol.detail.as_ref().unwrap();
    assert_eq!(detail["fp_rate_pct"], serde_json::json!(30.0));
    assert_eq!(detail["fp_lines"], serde_json::json!(3));
    let pairs: Vec<l0::FpDetail> =
        serde_json::from_value(detail["fp_detail"].clone()).expect("fp_detail 可机读");
    assert_eq!(pairs, want, "detail 精确定位 FP 的 (规则, 样本) 对");
}

/// FP detail 极端多: 60 对 → detail 截断至 FP_DETAIL_MAX, note 携带总数与
/// 上限 (与 FN 截断语义对称), 截断保持构造序稳定。
#[test]
fn gate_fp_detail_caps_with_total_in_note() {
    let samples: Vec<L0Sample> = (0..60)
        .map(|i| L0Sample {
            failure_frags: 1,
            failure_frag_lines: 1,
            frags: vec![FragRec {
                rule: "fp-rule".to_string(),
                intent: Intent::Failure,
                lines: 1,
            }],
            ..lit(&format!("github.com/r/{i}"), "success", false)
        })
        .collect();
    let s = l0::summarize(&l0::Collected {
        rule_tools: BTreeMap::new(),
        samples,
        rule_ids: vec![],
        orphans: vec![],
        sidecars_missing_attempt: 0,
    });
    assert_eq!(s.fp_detail.len(), 60, "summary 侧清单不截断");
    let v = l0::gate(&s, None, false);
    let viol = v.violations.iter().find(|x| x.kind == "fp-rate").unwrap();
    let pairs: Vec<l0::FpDetail> =
        serde_json::from_value(viol.detail.as_ref().unwrap()["fp_detail"].clone()).unwrap();
    assert_eq!(pairs.len(), l0::FP_DETAIL_MAX, "verdict detail 有上限");
    assert_eq!(pairs[0].repo, "github.com/r/0", "截断保持构造序");
    assert!(viol.note.contains("60"), "note 携带总数: {}", viol.note);
    assert!(
        viol.note.contains(&l0::FP_DETAIL_MAX.to_string()),
        "note 携带上限: {}",
        viol.note
    );
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
    assert!(viol.note.contains("targeted"), "note: {}", viol.note);
    assert!(viol.note.contains("fetch-chain"), "note: {}", viol.note);
    assert!(
        viol.note.contains("per_rule_hits"),
        "note 应指向定向验证的正向信号: {}",
        viol.note
    );
}

/// 判定: 仓维度语料在场不足 (样本 ≥50 但去重仓 <MIN_REPOS) → fail — 防语料面
/// **静默缩水**: 获取链路只深挖少数仓时样本数探不到, 仓维度是覆盖广度哨兵。
/// 分层语义: 样本下限未过时由 corpus-missing 单独报告, 本断言不叠报 —
/// 定向验证小语料的唯一违例形态保持不变 (issue #44 读法分流的延续)。
#[test]
fn gate_fails_on_corpus_repos_floor() {
    let mut s = ok_summary();
    s.structural.repos_total = l0::MIN_REPOS - 1;
    let v = l0::gate(&s, None, false);
    assert_eq!(v.verdict, "fail");
    let viol = v
        .violations
        .iter()
        .find(|x| x.kind == "corpus-repos-missing")
        .expect("corpus-repos-missing violation 在场");
    assert!(viol.note.contains("coverage shrank"), "note: {}", viol.note);
    assert!(
        viol.note.contains(&format!("<{}", l0::MIN_REPOS)),
        "note 应带仓维度阈值对照: {}",
        viol.note
    );

    // 分层: 样本下限未过 + 仓维度也不足 → 仅 corpus-missing (不叠报)
    let mut small = ok_summary();
    small.structural.samples_total = l0::MIN_SAMPLES - 1;
    small.structural.repos_total = 1;
    let v = l0::gate(&small, None, false);
    assert_eq!(v.verdict, "fail");
    assert!(v.violations.iter().any(|x| x.kind == "corpus-missing"));
    assert!(
        !v.violations
            .iter()
            .any(|x| x.kind == "corpus-repos-missing"),
        "样本下限已报告, 仓维度不叠报"
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
    assert!(
        e.contains("corpus directory not found"),
        "Err 应点明目录不存在: {e}"
    );
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
    assert!(viol.note.contains("tail line"), "note: {}", viol.note);
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
    assert_eq!(v.compression_compare.mode, "dual-run compare");
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
    assert!(warn.contains("1 sidecar"), "计数 N 在场: {warn}");
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

/// 存储形态后缀归一 (gz 等价测试辅助): fn/fp violation detail 的定位字段
/// 携带**实际文件名** (X.log.gz vs X.log — 指针真实, 消费方零解析, 与
/// L0Sample.sample/OrphanSample.path 同约定), 形态间仅此后缀字面不等 —
/// 归一后要求 Summary 整体完全一致 (压缩仍是纯存储细节)。
fn norm_gz_suffix(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(
            s.strip_suffix(".log.gz")
                .map(|b| format!("{b}.log"))
                .unwrap_or(s),
        ),
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(norm_gz_suffix).collect())
        }
        serde_json::Value::Object(o) => {
            serde_json::Value::Object(o.into_iter().map(|(k, v)| (k, norm_gz_suffix(v))).collect())
        }
        other => other,
    }
}

/// 同一批样本 全 plain / 全 gz / 混合 三形态: l0 全部指标 (Summary 整体)
/// 完全一致 — 压缩是存储细节, 不得改变任何指标; 采集不因形态产生孤儿。
/// 唯一允许的字面差 = detail 定位字段的 .gz 后缀 (实际文件名指针, 见
/// norm_gz_suffix), 归一后整体相等。
#[test]
fn gz_and_plain_corpus_metrics_identical() {
    let summary_json = |gz_mask: &[bool]| {
        let d = gz_tmp_corpus("equiv");
        gz_build_corpus(&d, gz_mask);
        let c = l0::collect(&d, &fixtures().join("rules")).expect("采集失败");
        assert_eq!(c.samples.len(), 3, "三形态均应配满 3 样本 (无孤儿)");
        assert!(c.orphans.is_empty(), "orphans: {:?}", c.orphans);
        norm_gz_suffix(serde_json::to_value(l0::summarize(&c)).unwrap())
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
    assert!(err.contains("conflict"), "报错须点明冲突语义: {err}");
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
        err.contains("log read failed"),
        "与既有读取失败文案同族: {err}"
    );
}

/// 引擎逻辑直测样本构造器 (语料边界, AGENTS.md 测试纪律: 规则门禁语义的
/// 唯一 SSOT 是真实语料; 引擎聚合/判定算术用显式内存数据直测 — 不造语料
/// 形态生造物)。全零指标 + 显式覆写 = 断言数字可逐项对账。
fn lit(repo: &str, conclusion: &str, gate_exempt: bool) -> L0Sample {
    L0Sample {
        repo: repo.to_string(),
        conclusion: conclusion.to_string(),
        attempt: 1,
        sample: format!("{repo}/{conclusion}.log"),
        excluded: false,
        gate_exempt,
        input_lines: 1,
        output_lines: 0,
        signature_output_lines: 0,
        failure_frags: 0,
        failure_frag_lines: 0,
        warning_frags: 0,
        info_frags: 0,
        zero_block: 0,
        boundary_violations: 0,
        frags: vec![],
        noise_hits: BTreeMap::new(),
    }
}

/// 豁免通道语义样本集 (内存直造): gated = r/a failure 有提取 + r/a、r/c
/// success 干净; exempt = r/b 未适配 (2 failure 零提取 + 1 success 含 failure
/// 命中=FP) + r/c 1 failure 零提取 — exempt 组 FN×3 / FP 率 50% / 零块×3,
/// 若入门禁必红。
fn exempt_collected() -> l0::Collected {
    l0::Collected {
        rule_tools: BTreeMap::new(),
        samples: vec![
            L0Sample {
                input_lines: 2,
                output_lines: 1,
                signature_output_lines: 1,
                failure_frags: 1,
                failure_frag_lines: 1,
                ..lit("r/a", "failure", false)
            },
            lit("r/a", "success", false),
            L0Sample {
                input_lines: 2,
                zero_block: 1,
                ..lit("r/b", "failure", true)
            },
            L0Sample {
                input_lines: 2,
                output_lines: 1,
                signature_output_lines: 1,
                failure_frags: 1,
                failure_frag_lines: 1,
                ..lit("r/b", "success", true)
            },
            L0Sample {
                zero_block: 1,
                ..lit("r/b", "failure", true)
            },
            lit("r/c", "success", false),
            L0Sample {
                zero_block: 1,
                ..lit("r/c", "failure", true)
            },
        ],
        rule_ids: vec!["hits-error-anchor".to_string()],
        orphans: vec![],
        sidecars_missing_attempt: 0,
    }
}

/// 豁免通道 (T-P2b-2): gate_exempt 样本跳 FN/FP/零块断言 (不进失败判定),
/// 指标照常计算并进报告; 语料在场计数含 exempt (防空转哨兵全覆盖)。
/// 引擎算术直测 (exempt_collected(), 无 IO 无规则匹配): 断言数字与构造
/// 逐项对账 — 豁免样本的指标不进门禁判定、照进报告。
#[test]
fn exempt_samples_skip_assertions_but_report() {
    let c = exempt_collected();
    let s = l0::summarize(&c);
    let v = l0::gate(&s, None, false);

    // 断言面只看 gated 组: exempt 组的 FN×3/FP 率 50%/零块×3 若入门禁必红,
    // 此处唯一违例是小语料防空转 (7<50, 与豁免无关 — 定向验证 fail 属预期)
    assert_eq!(
        v.violations.iter().map(|x| x.kind).collect::<Vec<_>>(),
        vec!["corpus-missing"],
        "exempt 组的 FN/FP/零块不进失败判定"
    );

    // gated 组指标 (totals 语义 = gated 域)
    assert_eq!(s.totals.runs, 3, "gated 主指标样本 = 2 (r/a) + 1 (r/c)");
    assert_eq!(s.totals.fn_samples, 0);
    assert!(
        s.fn_detail.is_empty(),
        "豁免仓的 FN 不进断言细节清单 (fn_detail 域 = gated)"
    );
    assert_eq!(s.totals.fp_rate_pct, 0.0);
    assert_eq!(s.structural.zero_block, 0, "零块断言域 = gated");

    // exempt 组指标照常计算 (报告面): 若进门禁必红的三个信号都在
    assert_eq!(s.exempt_totals.runs, 4);
    assert_eq!(s.exempt_totals.fn_samples, 3);
    assert!(
        s.exempt_totals.fp_frags >= 1,
        "exempt success 的 failure 命中照计"
    );
    assert!(
        s.exempt_totals.fp_rate_pct > l0::FP_MAX_PCT,
        "exempt FP 率超阈也不判红: {}",
        s.exempt_totals.fp_rate_pct
    );
    assert_eq!(s.structural.exempt_zero_block, 3);

    // 语料在场计数含 exempt (哨兵全覆盖) + 豁免余量可见
    assert_eq!(s.structural.samples_total, 7, "含 exempt 样本");
    assert_eq!(s.structural.samples_exempt, 4);
    assert_eq!(s.structural.exempt_repos, 2, "r/b + r/c (ANY 语义)");

    // 判定报告双组聚合节 (verdict JSON)
    assert_eq!(v.stats["groups"]["gated"]["fn"], 0);
    assert_eq!(v.stats["groups"]["exempt"]["fn"], 3);
    assert_eq!(v.stats["groups"]["exempt"]["runs"], 4);
    assert_eq!(v.stats["exempt"]["samples"], 4);
    assert_eq!(v.stats["exempt"]["repos"], 2);

    // CLI 单行: exempt 状态单列 (计数 N 与仓数 M 在场)
    let line = v.exempt_line().expect("exempt 在场时状态行非 None");
    assert!(!line.contains('\n'), "单行: {line}");
    assert!(line.contains("4 sample"), "{line}");
    assert!(line.contains("2 repo"), "{line}");
    assert!(line.contains("not asserted"), "{line}");
}

/// 旧 sidecar (无 gate_exempt 字段) 向后兼容: 解析机制用单件直测 (IO 机制类,
/// 与 gz 配对测试同族 — 最小构造件, 非语料形态生造物), 行为零漂移用内存直造。
#[test]
fn old_sidecar_without_gate_exempt_defaults_false() {
    // 解析层: 无字段 sidecar → gate_exempt=false (serde default)
    let d = std::env::temp_dir().join("ls-l0-sidecar-default");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("x.log"), "error: boom\n").unwrap();
    std::fs::write(
        d.join("x.log.json"),
        r#"{"repo":"r/x","conclusion":"failure","attempt":1}"#,
    )
    .unwrap();
    let c = l0::collect(&d, &fixtures().join("rules")).expect("采集失败");
    assert!(
        c.samples.iter().all(|s| !s.gate_exempt),
        "无字段 sidecar 解析为 gate_exempt=false"
    );

    // 行为层: 零豁免时 exempt 报告面全零、状态行静默 (None) — 豁免通道
    // 对存量判定零漂移 (全 gated 语义与豁免特性合入前一致)
    let c = l0::Collected {
        rule_tools: BTreeMap::new(),
        samples: vec![
            L0Sample {
                input_lines: 2,
                failure_frags: 1,
                failure_frag_lines: 1,
                ..lit("r/x", "failure", false)
            },
            lit("r/x", "success", false),
        ],
        rule_ids: vec!["hits-error-anchor".to_string()],
        orphans: vec![],
        sidecars_missing_attempt: 0,
    };
    let s = l0::summarize(&c);
    assert_eq!(s.structural.samples_exempt, 0);
    assert_eq!(s.exempt_totals.runs, 0, "exempt 组空");
    let v = l0::gate(&s, None, false);
    assert_eq!(
        v.violations.iter().map(|x| x.kind).collect::<Vec<_>>(),
        vec!["corpus-missing"],
        "零豁免判定 = 豁免特性合入前的语义 (仅小语料防空转)"
    );
    assert_eq!(v.stats["groups"]["exempt"]["runs"], 0);
    assert!(v.exempt_line().is_none(), "N=0 静默 (警告只对真实豁免发出)");
}

/// 逐仓分解表 (sweep 报告骨架): repo → 样本数/豁免样本数/FN/FP/零块/压缩率。
/// 行口径 = 主指标域 (滤 rerun-through), FN/零块不分 gated/exempt (诊断视图 —
/// 未适配仓离达标有多远); exempt_samples 标组属 (0=全入断言)。
#[test]
fn per_repo_breakdown_rows() {
    let c = exempt_collected();
    let s = l0::summarize(&c);
    let rows = &s.per_repo_breakdown;
    assert_eq!(rows.len(), 3, "逐仓表覆盖全部仓 (gated + exempt)");

    let a = &rows["r/a"];
    assert_eq!(
        (a.runs, a.exempt_samples, a.fn_samples, a.zero_block),
        (2, 0, 0, 0)
    );
    assert_eq!(a.fp_rate_pct, 0.0);

    let b = &rows["r/b"];
    assert_eq!(
        (b.runs, b.exempt_samples, b.fn_samples, b.zero_block),
        (3, 3, 2, 2)
    );
    assert!(b.fp_rate_pct > 0.0, "exempt 仓的 FP 照常进表");

    let m = &rows["r/c"];
    assert_eq!(
        (m.runs, m.exempt_samples, m.fn_samples, m.zero_block),
        (2, 1, 1, 1),
        "mixed 行: gated+exempt 合并计, exempt_samples 标组属"
    );
}

/// per-rule × per-repo 命中矩阵: 定义域 = 装载规则全集 (与 per_rule_hits 一致,
/// 零命中规则为空 map), 计数口径 = signal fragment 数 ∪ noise 剔除区块数,
/// 全样本域 (liveness); 性质: 逐规则 Σ_repo matrix == per_rule_hits。
#[test]
fn rule_repo_matrix_counts() {
    let c = l0::Collected {
        rule_tools: BTreeMap::new(),
        samples: vec![
            L0Sample {
                input_lines: 2,
                output_lines: 1,
                signature_output_lines: 1,
                failure_frags: 1,
                failure_frag_lines: 1,
                frags: vec![FragRec {
                    rule: "hits-error-anchor".to_string(),
                    intent: Intent::Failure,
                    lines: 1,
                }],
                noise_hits: BTreeMap::from([("note-noise".to_string(), 1u64)]),
                ..lit("r/a", "failure", false)
            },
            L0Sample {
                noise_hits: BTreeMap::from([("note-noise".to_string(), 1u64)]),
                ..lit("r/a", "success", false)
            },
        ],
        rule_ids: vec![
            "dead-never-match".to_string(),
            "hits-error-anchor".to_string(),
            "note-noise".to_string(),
        ],
        orphans: vec![],
        sidecars_missing_attempt: 0,
    };
    let s = l0::summarize(&c);
    let m = &s.rule_repo_hits;
    // 定义域 = 装载全集: dead 规则以空 map 在场
    assert_eq!(
        m.get("dead-never-match"),
        Some(&BTreeMap::new()),
        "零命中规则 = 空 map (死规则由 dead_rules 单列)"
    );
    assert_eq!(
        m.get("hits-error-anchor"),
        Some(&BTreeMap::from([("r/a".to_string(), 1u64)])),
        "signal 规则按 fragment 数逐仓计"
    );
    assert_eq!(
        m.get("note-noise"),
        Some(&BTreeMap::from([("r/a".to_string(), 2u64)])),
        "noise 规则按剔除区块数逐仓计 (两样本各一区块)"
    );
    // 一致性 (性质): 每规则矩阵行和 == per_rule_hits
    for (rule, per_repo) in m {
        let sum: u64 = per_repo.values().sum();
        assert_eq!(
            sum, s.per_rule_hits[rule],
            "矩阵行和应等于 per_rule_hits[{rule}]"
        );
    }
}

/// 双测兼容 (T-P2b-2): base summary 产自旧引擎时无新字段 — Summary 反序列化
/// 以 serde(default) 兜底, 新字段全默认 (CI 双测: origin/master 引擎 emit ×
/// PR 引擎 --base 消费)。字段缺 default 属性 = 旧 base 上解析炸 (exit 2)。
#[test]
fn old_base_summary_without_new_fields_deserializes() {
    let old = r#"{
      "per_repo_conclusion": {},
      "per_repo": {"github.com/r/kept": {
        "runs": 30, "input_lines": 10000, "output_lines": 200,
        "signature_output_lines": 80, "fn": 0, "success_input_lines": 5000,
        "fp_frags": 0, "fp_lines": 0, "success_warning_frags": 0,
        "success_info_frags": 0, "comp_blk_pct": 98.0, "comp_sig_pct": 99.0,
        "fp_rate_pct": 0.0}},
      "totals": {
        "runs": 30, "input_lines": 10000, "output_lines": 200,
        "signature_output_lines": 80, "fn": 0, "success_input_lines": 5000,
        "fp_frags": 0, "fp_lines": 0, "success_warning_frags": 0,
        "success_info_frags": 0, "comp_blk_pct": 98.0, "comp_sig_pct": 99.0,
        "fp_rate_pct": 0.0},
      "per_rule": {},
      "structural": {"zero_block": 0, "boundary_violations": 0,
                     "samples_total": 60, "samples_excluded": 1},
      "per_rule_hits": {"some-rule": 3}
    }"#;
    let s: Summary = serde_json::from_str(old).expect("旧格式 base summary 必须可解析");
    assert_eq!(s.exempt_totals.runs, 0);
    assert!(s.per_repo_breakdown.is_empty());
    assert!(s.rule_repo_hits.is_empty());
    assert_eq!(
        (
            s.structural.exempt_zero_block,
            s.structural.exempt_boundary,
            s.structural.samples_exempt,
            s.structural.exempt_repos
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        s.structural.repos_total, 0,
        "旧格式无仓维度哨兵字段 → serde 默认 0 (仅作 base 消费, 断言域在当前侧)"
    );
    assert!(
        s.fn_detail.is_empty(),
        "旧格式无 fn_detail 字段 → serde 默认空清单"
    );
    assert!(
        s.fp_detail.is_empty(),
        "旧格式无 fp_detail 字段 → serde 默认空清单"
    );
    // 旧字段照常消费 (双测比对域)
    assert_eq!(s.per_repo["github.com/r/kept"].comp_blk_pct, 98.0);
    assert_eq!(s.per_rule_hits["some-rule"], 3);
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
            repos_total: l0::MIN_REPOS,
            ..Default::default()
        },
        ..Default::default()
    };
    s.per_repo.insert("github.com/r/kept".into(), agg(98.0));
    s
}

// ---- 规则健康档案 (2026-10-21 落地): 内存直测 — 分级断崖/Wilson/样本级去重/覆盖率 ----

fn hsample(conclusion: &str, frags: &[(&str, Intent)]) -> L0Sample {
    L0Sample {
        repo: "github.com/r/t".into(),
        conclusion: conclusion.into(),
        attempt: 1,
        sample: format!("{conclusion}.log"),
        excluded: false,
        gate_exempt: false,
        input_lines: 10,
        output_lines: 2,
        signature_output_lines: 1,
        failure_frags: frags.iter().filter(|(_, i)| *i == Intent::Failure).count() as u64,
        failure_frag_lines: 0,
        warning_frags: frags.iter().filter(|(_, i)| *i == Intent::Warning).count() as u64,
        info_frags: frags.iter().filter(|(_, i)| *i == Intent::Info).count() as u64,
        zero_block: 0,
        boundary_violations: 0,
        frags: frags
            .iter()
            .map(|(r, i)| FragRec {
                rule: r.to_string(),
                intent: *i,
                lines: 1,
            })
            .collect(),
        noise_hits: BTreeMap::new(),
    }
}

/// 分级断崖 (实测形态注入): 注解地板 → restating; 工具规则 → diagnostic;
/// 中间带与纯成功异常 → boundary。
#[test]
fn rule_health_grading_cliff() {
    let (nf, ns) = (943usize, 375usize);
    let base = nf as f64 / (nf + ns) as f64;
    // 地板: P_f=932/943=0.988, P_s=2/375=0.005 → restating, 高可靠零信息
    let floor = l0::rule_health(
        l0::RuleFire {
            fire_failure: 932,
            fire_success: 2,
        },
        nf,
        ns,
        base,
        Intent::Failure,
        "t".to_string(),
    );
    assert_eq!(floor.grade, l0::RuleGrade::Restating);
    assert!((floor.ppv - 0.998).abs() < 0.001, "ppv={}", floor.ppv);
    assert!(floor.ppv_lo >= 0.99, "Wilson 下界={}", floor.ppv_lo);
    assert!(floor.spec_bits < 0.1, "零信息: {}", floor.spec_bits);
    // 工具规则: a=42,b=0 → diagnostic, spec≈4.49b, lift=1.40 (基率上限)
    let tool = l0::rule_health(
        l0::RuleFire {
            fire_failure: 42,
            fire_success: 0,
        },
        nf,
        ns,
        base,
        Intent::Failure,
        "t".to_string(),
    );
    assert_eq!(tool.grade, l0::RuleGrade::Diagnostic);
    assert!((tool.spec_bits - 4.49).abs() < 0.02);
    assert!((tool.lift - 1.0 / base).abs() < 0.01, "lift 封顶 1/基率");
    // 边界带: P_f=0.5 → boundary (断裂带中不应存在, 出现即人工审)
    let mid = l0::rule_health(
        l0::RuleFire {
            fire_failure: 471,
            fire_success: 0,
        },
        nf,
        ns,
        base,
        Intent::Failure,
        "t".to_string(),
    );
    assert_eq!(mid.grade, l0::RuleGrade::Boundary);
    // 纯成功触发 (FP 形态): a=0,b=5 → boundary, ppv=0
    let fp_only = l0::rule_health(
        l0::RuleFire {
            fire_failure: 0,
            fire_success: 5,
        },
        nf,
        ns,
        base,
        Intent::Failure,
        "t".to_string(),
    );
    assert_eq!(fp_only.grade, l0::RuleGrade::Boundary);
    assert_eq!(fp_only.ppv, 0.0);
    // 阈值不变性: 断裂带内 (0.2, 0.8) 任意 P_f 同判 boundary — 抽 0.21/0.79 复核
    for a in [200u32, 746] {
        let g = l0::rule_health(
            l0::RuleFire {
                fire_failure: a,
                fire_success: 0,
            },
            nf,
            ns,
            base,
            Intent::Failure,
            "t".to_string(),
        );
        assert_eq!(g.grade, l0::RuleGrade::Boundary, "a={a}");
    }
    // info 常数签名: 全触发 → lift=1.00 精确独立
    let info = l0::rule_health(
        l0::RuleFire {
            fire_failure: 943,
            fire_success: 375,
        },
        nf,
        ns,
        base,
        Intent::Info,
        "t".to_string(),
    );
    assert!((info.lift - 1.0).abs() < 0.01, "info lift={}", info.lift);
}

/// Wilson 下界数值锚 (与百仓实测对齐): (932,2)→≥0.99; (16,1)→≈0.73; (0,0)→0。
#[test]
fn wilson_lower_bound_sanity() {
    assert!(l0::wilson_lo(932.0, 2.0) >= 0.99);
    let lo = l0::wilson_lo(16.0, 1.0);
    assert!((0.72..0.76).contains(&lo), "ninja 形态下界={lo}");
    assert_eq!(l0::wilson_lo(0.0, 0.0), 0.0);
}

/// health() 聚合口径: 样本级去重 / 三级分级各归其位 / 覆盖只认非 restating 块。
/// 场景 30 失败 + 10 成功: floor 30/30 失败 + 0/10 成功 (restating); diag 2 样本
/// (其一双 fragment 验去重) p_f=0.067 (diagnostic); mid 14 样本 p_f=0.467 (boundary,
/// 亦计覆盖); 仅 floor 的失败样本不计覆盖。
#[test]
fn health_aggregates_sample_level_and_coverage() {
    let mut samples: Vec<L0Sample> = Vec::new();
    for i in 0..30 {
        let mut frags: Vec<(&str, Intent)> = vec![("floor", Intent::Failure)];
        match i {
            0 => frags.extend([("diag", Intent::Failure), ("diag", Intent::Failure)]), // 去重验证
            1 => frags.push(("diag", Intent::Failure)),
            3..=16 => frags.push(("mid", Intent::Failure)),
            _ => {}
        }
        samples.push(hsample("failure", &frags));
    }
    for _ in 0..10 {
        samples.push(hsample("success", &[]));
    }
    let refs: Vec<&L0Sample> = samples.iter().collect();
    let tools: BTreeMap<String, String> = [
        ("floor".to_string(), "floor".to_string()),
        ("diag".to_string(), "diag".to_string()),
        ("mid".to_string(), "mid".to_string()),
    ]
    .into_iter()
    .collect();
    let (fire, health_map, cov) = l0::health(&refs, &tools);
    // 样本级去重: 样本0 的 diag 双 fragment 只计 1 → fire_failure=2
    assert_eq!(
        (fire["diag"].fire_failure, fire["diag"].fire_success),
        (2, 0)
    );
    assert_eq!(
        (fire["floor"].fire_failure, fire["floor"].fire_success),
        (30, 0)
    );
    assert_eq!(
        (fire["mid"].fire_failure, fire["mid"].fire_success),
        (14, 0)
    );
    assert_eq!(health_map["floor"].grade, l0::RuleGrade::Restating);
    assert_eq!(health_map["diag"].grade, l0::RuleGrade::Diagnostic);
    assert_eq!(health_map["mid"].grade, l0::RuleGrade::Boundary);
    // 覆盖: diag 2 + mid 14 = 16/30 (boundary 计覆盖, 仅 restating 不计);
    // 其余 14 个仅 floor 的失败样本是 "泛化独撑" 形态, 不计
    assert_eq!(cov.total, 30);
    assert_eq!(cov.covered, 16);
    assert!((cov.pct - 53.33).abs() < 0.01, "pct={}", cov.pct);
}

/// serde 兼容: 旧 summary JSON (无健康三字段) 可解析, 字段取默认空值。
#[test]
fn old_summary_without_health_fields_deserializes() {
    let old = r#"{"per_repo_conclusion":{},"per_repo":{},"totals":{"runs":1,
        "input_lines":10,"output_lines":2,"signature_output_lines":1,"fn":0,
        "success_input_lines":4,"fp_frags":0,"fp_lines":0,
        "success_warning_frags":0,"success_info_frags":0,
        "comp_blk_pct":80.0,"comp_sig_pct":90.0,"fp_rate_pct":0.0},
        "per_rule":{},"structural":{"zero_block":0,"boundary_violations":0,"samples_total":1,"samples_excluded":0}}"#;
    let s: Summary = serde_json::from_str(old).expect("旧 summary 可解析");
    assert!(s.per_rule_fire.is_empty());
    assert!(s.per_rule_health.is_empty());
    assert_eq!(s.diagnostic_coverage.covered, 0);
}
