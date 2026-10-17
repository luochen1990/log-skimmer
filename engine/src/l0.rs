//! L0 全语料回归门禁 (口径与判定 SSOT) — spec §4。
//!
//! 职责边界: 采集 (collect: 递归遍历语料仓, 单次加载规则跑全量 skim) →
//!   汇总 (summarize: 三层聚合, 口径单点) → 判定 (gate: 断言族 + 双测对比)。
//!   CLI 形态 `log-skimmer l0` (main.rs 接线); bash/jq 时代脚本的等价移植
//!   (l0-collect.sh / l0-summarize.jq / l0-gate.sh 已退役), 判定逻辑自此可单测。
//!
//! 口径 (spec §4, 与 jq 时代逐条等价):
//!   - 主指标滤除 rerun-through (attempt>1 且 conclusion=success);
//!   - FN 谓词只数 failure/timed_out 家族样本上 failure-intent fragment 数为 0;
//!     cancelled 截断日志不计缺失 (三态);
//!   - FP 分子 = success 样本上 failure-intent fragment 的 block 区间行数,
//!     率分母 = 该聚合 success 样本输入总行数;
//!   - 零块/边界违例全量统计 (不滤 excluded);
//!   - 压缩率 blk/sig = 聚合行数比 (1-Σout/Σin), 非样本内率平均。
//!
//! 判定断言族: 语料在场 ≥50 / FN=0 / FP率≤2% / 边界违例=0 / 零块=0;
//!   压缩率防退化 = 双测对比 (base summary 提供 per-repo 基准, 相对退化 >2% 判红;
//!   未提供则单测模式显式声明 — 引擎仓传 base 引擎实测, 规则仓传 base 规则实测,
//!   同语料下纯单变量归因)。

use crate::{load_rules, skim, Intent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ---- 阈值 (判定 SSOT; 原 l0-gate.sh 头注释的职责迁移至此) ----
/// 语料在场下限: 防空转假绿 (获取链路整体故障时拒判)。50 = 兼容单源环境
/// (rules 仓 CI 仅 corpus 57 对; 主仓双源 67 对) — 防空转语义在 50 与 67 无差。
pub const MIN_SAMPLES: usize = 50;
pub const FP_MAX_PCT: f64 = 2.0;
pub const REGRESSION_MAX_PCT: f64 = 2.0;

// ---- 采集层 ----

/// 单样本实测记录 (sidecar 元数据 × skim 结果聚合)。
#[derive(Debug, Clone, Serialize)]
pub struct L0Sample {
    pub repo: String,
    pub conclusion: String,
    pub attempt: u32,
    pub sample: String,
    pub excluded: bool,
    pub input_lines: usize,
    pub output_lines: usize,
    pub signature_output_lines: usize,
    pub failure_frags: u64,
    pub failure_frag_lines: u64,
    pub warning_frags: u64,
    pub info_frags: u64,
    pub zero_block: u32,
    pub boundary_violations: usize,
    pub frags: Vec<FragRec>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FragRec {
    pub rule: String,
    pub intent: Intent,
    pub lines: u64,
}

/// sidecar 的最小投影 (l0 只消费这三个字段; 全量 sidecar schema 见 spec §5)。
#[derive(Deserialize)]
struct Sidecar {
    repo: String,
    conclusion: String,
    #[serde(default = "one")]
    attempt: u32,
}
fn one() -> u32 {
    1
}

/// 递归收集目录下全部 *.log.json (路径字典序, 确定性遍历 — 等价 `find | sort`)。
fn walk_sidecars(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk_sidecars(&p, out);
        } else if p.extension().is_some_and(|x| x == "json")
            && p.file_stem()
                .is_some_and(|s| s.to_string_lossy().ends_with(".log"))
        {
            out.push(p);
        }
    }
}

/// 采集全语料: 规则单次加载, 逐样本 skim。非法规则集/日志读取失败 → Err (fail-fast,
/// 门禁无兜底); 零块是正常记录 (违例判定在 gate — 结构承诺的失明信号)。
pub fn collect(corpus_dir: &Path, rules_dir: &Path) -> Result<Vec<L0Sample>, String> {
    let rules = load_rules(rules_dir).map_err(|e| format!("规则集非法: {e}"))?;
    let mut sidecars = Vec::new();
    walk_sidecars(corpus_dir, &mut sidecars);
    let mut samples = Vec::with_capacity(sidecars.len());
    for j in &sidecars {
        let log_path = PathBuf::from(j.to_string_lossy().trim_end_matches(".json"));
        if !log_path.is_file() {
            continue;
        }
        let sc: Sidecar = serde_json::from_str(
            &std::fs::read_to_string(j)
                .map_err(|e| format!("sidecar 读取失败 {}: {e}", j.display()))?,
        )
        .map_err(|e| format!("sidecar 解析失败 {}: {e}", j.display()))?;
        let raw = std::fs::read_to_string(&log_path)
            .map_err(|e| format!("日志读取失败 {}: {e}", log_path.display()))?;
        let lines = crate::normalize_lines(&raw);
        let r = skim(&log_path.display().to_string(), &rules, &lines);
        let sample = log_path
            .strip_prefix(corpus_dir)
            .unwrap_or(&log_path)
            .display()
            .to_string();
        let mut failure_frags = 0u64;
        let mut failure_frag_lines = 0u64;
        let mut warning_frags = 0u64;
        let mut info_frags = 0u64;
        let mut frags = Vec::new();
        for b in &r.blocks {
            for f in &b.fragments {
                let lines = (f.block.1 - f.block.0 + 1) as u64;
                match f.intent {
                    Intent::Failure => {
                        failure_frags += 1;
                        failure_frag_lines += lines;
                    }
                    Intent::Warning => warning_frags += 1,
                    Intent::Info => info_frags += 1,
                }
                frags.push(FragRec {
                    rule: f.rule.clone(),
                    intent: f.intent,
                    lines,
                });
            }
        }
        let excluded = sc.attempt > 1 && sc.conclusion == "success";
        samples.push(L0Sample {
            repo: sc.repo,
            conclusion: sc.conclusion,
            attempt: sc.attempt,
            sample,
            excluded,
            input_lines: r.input_lines,
            output_lines: r.output_lines,
            signature_output_lines: r.stats.signature_output_lines,
            failure_frags,
            failure_frag_lines,
            warning_frags,
            info_frags,
            zero_block: u32::from(r.blocks.is_empty()),
            boundary_violations: r.stats.boundary_violations.len(),
            frags,
        });
    }
    Ok(samples)
}

// ---- 汇总层 ----

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Agg {
    pub runs: usize,
    pub input_lines: u64,
    pub output_lines: u64,
    pub signature_output_lines: u64,
    #[serde(rename = "fn")]
    pub fn_samples: usize,
    pub success_input_lines: u64,
    pub fp_frags: u64,
    pub fp_lines: u64,
    pub success_warning_frags: u64,
    pub success_info_frags: u64,
    pub comp_blk_pct: f64,
    pub comp_sig_pct: f64,
    pub fp_rate_pct: f64,
}

fn ratio_pct(num: f64, den: f64) -> f64 {
    if den > 0.0 {
        100.0 * num / den
    } else {
        0.0
    }
}

fn agg(samples: &[&L0Sample]) -> Agg {
    let mut a = Agg {
        runs: samples.len(),
        ..Default::default()
    };
    for s in samples {
        a.input_lines += s.input_lines as u64;
        a.output_lines += s.output_lines as u64;
        a.signature_output_lines += s.signature_output_lines as u64;
        let failure_family = s.conclusion == "failure" || s.conclusion == "timed_out";
        if failure_family && s.failure_frags == 0 {
            a.fn_samples += 1;
        }
        if s.conclusion == "success" {
            a.success_input_lines += s.input_lines as u64;
            a.fp_frags += s.failure_frags;
            a.fp_lines += s.failure_frag_lines;
            a.success_warning_frags += s.warning_frags;
            a.success_info_frags += s.info_frags;
        }
    }
    a.comp_blk_pct = ratio_pct(
        a.input_lines as f64 - a.output_lines as f64,
        a.input_lines as f64,
    );
    a.comp_sig_pct = ratio_pct(
        a.input_lines as f64 - a.signature_output_lines as f64,
        a.input_lines as f64,
    );
    a.fp_rate_pct = ratio_pct(a.fp_lines as f64, a.success_input_lines as f64);
    a
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleAgg {
    pub frags: usize,
    pub lines: u64,
    pub fp_frags: usize,
    pub hit_repos: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Structural {
    pub zero_block: u32,
    pub boundary_violations: usize,
    pub samples_total: usize,
    pub samples_excluded: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub per_repo_conclusion: BTreeMap<String, Agg>,
    pub per_repo: BTreeMap<String, Agg>,
    pub totals: Agg,
    pub per_rule: BTreeMap<String, RuleAgg>,
    pub structural: Structural,
}

pub fn summarize(all: &[L0Sample]) -> Summary {
    let main: Vec<&L0Sample> = all.iter().filter(|s| !s.excluded).collect();
    let mut per_repo_conclusion = BTreeMap::new();
    let mut per_repo: BTreeMap<String, Vec<&L0Sample>> = BTreeMap::new();
    for &s in &main {
        per_repo_conclusion.insert(format!("{}|{}", s.repo, s.conclusion), agg(&[s]));
        per_repo.entry(s.repo.clone()).or_default().push(s);
    }
    let per_repo: BTreeMap<String, Agg> =
        per_repo.iter().map(|(k, v)| (k.clone(), agg(v))).collect();
    let mut per_rule: BTreeMap<String, Vec<(Intent, u64, String, String)>> = BTreeMap::new();
    for &s in &main {
        for f in &s.frags {
            per_rule.entry(f.rule.clone()).or_default().push((
                f.intent,
                f.lines,
                s.repo.clone(),
                s.conclusion.clone(),
            ));
        }
    }
    let per_rule: BTreeMap<String, RuleAgg> = per_rule
        .into_iter()
        .map(|(rule, recs)| {
            (
                rule,
                RuleAgg {
                    frags: recs.len(),
                    lines: recs.iter().map(|(_, l, _, _)| *l).sum(),
                    fp_frags: recs
                        .iter()
                        .filter(|(i, _, _, c)| *i == Intent::Failure && *c == "success")
                        .count(),
                    hit_repos: recs
                        .iter()
                        .filter(|(_, _, _, c)| c != "success")
                        .map(|(_, _, r, _)| r.clone())
                        .collect::<Vec<_>>()
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len(),
                },
            )
        })
        .collect();
    let totals = agg(&main);
    let structural = Structural {
        zero_block: all.iter().map(|s| s.zero_block).sum(),
        boundary_violations: all.iter().map(|s| s.boundary_violations).sum(),
        samples_total: all.len(),
        samples_excluded: all.iter().filter(|s| s.excluded).count(),
    };
    Summary {
        per_repo_conclusion,
        per_repo,
        totals,
        per_rule,
        structural,
    }
}

// ---- 判定层 ----

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Regression {
    pub repo: String,
    pub delta_blk_pct: f64,
    pub cur_blk_pct: f64,
    pub base_blk_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompressionCompare {
    pub mode: &'static str,
    pub regressions: Vec<Regression>,
    pub drift_notices: Vec<String>,
    pub base_fail: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    pub verdict: &'static str,
    pub stats: serde_json::Value,
    pub violations: Vec<Violation>,
    pub compression_compare: CompressionCompare,
}

/// 判定入口: 断言族 (全量语料) + 双测压缩率对比 (base 在场时)。
pub fn gate(pr: &Summary, base: Option<&Summary>) -> Verdict {
    let mut violations = Vec::new();
    let t = &pr.totals;
    let st = &pr.structural;
    if st.samples_total < MIN_SAMPLES {
        violations.push(Violation {
            kind: "corpus-missing",
            note: format!(
                "语料样本数异常 ({} <{MIN_SAMPLES}), 疑获取链路故障 — 拒绝在空语料上判定",
                st.samples_total
            ),
            detail: None,
        });
    }
    if t.fn_samples > 0 {
        let detail: Vec<_> = pr
            .per_repo_conclusion
            .iter()
            .filter(|(_, a)| a.fn_samples > 0)
            .map(|(k, a)| serde_json::json!({k: a.fn_samples}))
            .collect();
        violations.push(Violation {
            kind: "fn",
            note: "失败样本零 failure-intent fragment (完全漏检, 召回优先不变量)".into(),
            detail: Some(serde_json::Value::Array(detail)),
        });
    }
    if t.fp_rate_pct > FP_MAX_PCT {
        violations.push(Violation {
            kind: "fp-rate",
            note: format!(
                "success 样本 failure-intent 假阳性率超阈 ({:.2}% >{FP_MAX_PCT}%)",
                t.fp_rate_pct
            ),
            detail: Some(serde_json::json!({"fp_rate_pct": t.fp_rate_pct, "fp_lines": t.fp_lines})),
        });
    }
    if st.boundary_violations > 0 {
        violations.push(Violation {
            kind: "boundary",
            note: "context before/after 越界 [0,10]".into(),
            detail: None,
        });
    }
    if st.zero_block > 0 {
        violations.push(Violation {
            kind: "zero-block",
            note: "存在 skim 零块样本 — 规则集对该日志失明 (结构承诺)".into(),
            detail: None,
        });
    }

    let cmp = match base {
        Some(b) => {
            let mut regressions = Vec::new();
            let mut drifts = Vec::new();
            for (repo, cur) in &pr.per_repo {
                match b.per_repo.get(repo) {
                    None => {
                        drifts.push(format!("{repo}: base 侧无此 repo (语料集漂移?), 人工核对"))
                    }
                    Some(base_agg) => {
                        let delta = if base_agg.comp_blk_pct == 0.0 {
                            0.0
                        } else {
                            (cur.comp_blk_pct - base_agg.comp_blk_pct) / base_agg.comp_blk_pct
                                * 100.0
                        };
                        if delta < -REGRESSION_MAX_PCT {
                            regressions.push(Regression {
                                repo: repo.clone(),
                                delta_blk_pct: delta,
                                cur_blk_pct: cur.comp_blk_pct,
                                base_blk_pct: base_agg.comp_blk_pct,
                            });
                        }
                    }
                }
            }
            let base_fail = !regressions.is_empty();
            CompressionCompare {
                mode: "双测对比",
                regressions,
                drift_notices: drifts,
                base_fail,
            }
        }
        None => CompressionCompare {
            mode: "单测 (未提供 base summary, 压缩率对比跳过 — CI 编排应提供 base 以启用退化哨兵)",
            regressions: Vec::new(),
            drift_notices: Vec::new(),
            base_fail: false,
        },
    };

    let verdict = if violations.is_empty() && !cmp.base_fail {
        "pass"
    } else {
        "fail"
    };
    Verdict {
        verdict,
        stats: serde_json::json!({
            "fn": t.fn_samples, "fp_rate_pct": t.fp_rate_pct,
            "zero_blocks": st.zero_block, "boundary_violations": st.boundary_violations,
            "samples": st.samples_total,
            "comp_blk_pct": t.comp_blk_pct, "comp_sig_pct": t.comp_sig_pct,
        }),
        violations,
        compression_compare: cmp,
    }
}
