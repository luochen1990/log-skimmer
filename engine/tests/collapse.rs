//! repeat-collapse 输出层折叠 (spec §3.R) 行为单测。
//!
//! 职责边界: 折叠语义 (run 划分/锚块/标记) / 阈值边界 / 统计口径 (呈现 vs 全量) /
//! 零行为变更回归 (阈值不满足 → 与 OFF 逐字节一致) / L0 全量口径恢复 / CLI 面。

use log_skimmer::{skim, skim_with, CollapseParams, CollapseStats, Rule, SkimResult};
use serde_json::ser::PrettyFormatter;
use serde_json::Serializer;

fn rules(yaml: &str) -> Vec<Rule> {
    let mut r: Rule = serde_yaml::from_str(yaml).expect("测试规则 YAML 应合法");
    for p in &mut r.patterns {
        p.compiled = Some(regex::Regex::new(&p.regex).unwrap());
    }
    vec![r]
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn flat(vs: &[Vec<Rule>]) -> Vec<Rule> {
    vs.concat()
}

/// 同规则单锚规则 (无扩展): 每个锚行一块 — 折叠谓词的最小可控构件。
fn single_rule(id: &str) -> Vec<Rule> {
    let tool = id.split('-').next().unwrap();
    rules(&format!(
        "id: {id}\ntitle: t\ntool: {tool}\nkind: signal\npatterns:\n  - regex: '^E'\n"
    ))
}

fn to_json(r: &SkimResult) -> String {
    let mut buf = Vec::new();
    let mut ser = Serializer::with_formatter(&mut buf, PrettyFormatter::new());
    serde::Serialize::serialize(r, &mut ser).unwrap();
    String::from_utf8(buf).unwrap()
}

/// N 个单行锚块, 块间隔 gap 行 (间隔行不匹配任何规则)。
fn cascade_log(n_blocks: usize, gap: usize) -> Vec<String> {
    let mut v = Vec::new();
    v.push("E0".to_string());
    for i in 1..n_blocks {
        for _ in 0..gap {
            v.push("--".to_string()); // 无关间隔行
        }
        v.push(format!("E{i}"));
    }
    v
}

// ---- R1/R2: 折叠语义 ----

#[test]
fn run_folds_to_anchor_with_marker() {
    // 5 块 gap=1 的同规则级联 → 1 锚块 + 4 折叠块标记。
    let rs = single_rule("t-c");
    let res = skim("-", &rs, &cascade_log(5, 1));
    assert_eq!(res.blocks.len(), 1, "级联折叠为单锚块");
    let m = res.blocks[0].collapse.as_ref().expect("锚块带折叠标记");
    assert_eq!(m.repeat_blocks, 4);
    assert_eq!(m.rule, "t-c");
    // 锚行 = 被折叠各 fragment 首元素: E1..E4 行号 = 3,5,7,9
    assert_eq!(m.frag_spans, vec![(3, 3), (5, 5), (7, 7), (9, 9)]);
    // 输出区间 hull: E1..E4
    assert_eq!((m.start_line, m.end_line), (3, 9));
    // omitted: 每折叠块 1 行, 均不被锚块呈现 → 4
    assert_eq!(m.omitted_lines, 4);
}

#[test]
fn only_anchor_presented_after_fold() {
    let rs = single_rule("t-c");
    let res = skim("-", &rs, &cascade_log(4, 1));
    // 呈现: 仅锚块 (行 1); 折叠省略 3 行 → output_lines = 1
    assert_eq!(res.output_lines, 1);
    assert_eq!(
        res.stats.collapse,
        CollapseStats {
            folded_blocks: 3,
            omitted_lines: 3
        }
    );
}

// ---- R1: 阈值边界 ----

#[test]
fn gap_boundary_folds_at_max_and_breaks_beyond() {
    let rs = single_rule("t-c");
    // 默认 max_gap=20: gap 恰 20 → 折; gap 21 → 不折。
    let at = skim("-", &rs, &cascade_log(3, 20));
    assert_eq!(at.blocks.len(), 1, "gap == max_gap 仍属同一 run");
    assert_eq!(at.blocks[0].collapse.as_ref().unwrap().repeat_blocks, 2);
    let beyond = skim("-", &rs, &cascade_log(3, 21));
    assert_eq!(beyond.blocks.len(), 3, "gap > max_gap 隔断 run");
    assert!(beyond.blocks[0].collapse.is_none());
    assert_eq!(beyond.stats.collapse, CollapseStats::default());
}

#[test]
fn min_repeat_three_folds_two_never() {
    let rs = single_rule("t-c");
    let three = skim("-", &rs, &cascade_log(3, 1));
    assert_eq!(
        three.blocks[0].collapse.as_ref().unwrap().repeat_blocks,
        2,
        "len=3 (默认阈值) 折叠 2 块"
    );
    let two = skim("-", &rs, &cascade_log(2, 1));
    assert_eq!(two.blocks.len(), 2, "len=2 永不折叠");
    assert!(two.blocks[0].collapse.is_none());
}

#[test]
fn multi_rule_block_breaks_run_and_never_folds() {
    // E1 x E2 x [E3+B9 混合块] x E4 x E5: 混合块隔断 → 两个 len=2 run, 均不折。
    let rs = flat(&[single_rule("t-a"), single_rule("t-b")]);
    let ls = lines(&["E1", "x", "E2", "x", "E3", "B9", "x", "E4", "x", "E5"]);
    let res = skim("-", &rs, &ls);
    assert_eq!(
        res.blocks.len(),
        5,
        "独立 E 块 ×4 + E3/B9 相邻合并的多规则块 ×1"
    );
    assert_eq!(
        res.blocks[2].rules,
        vec!["t-a", "t-b"],
        "E3/B9 相邻 → 多规则块"
    );
    assert!(
        res.blocks.iter().all(|b| b.collapse.is_none()),
        "混合块隔断 run, 两侧 len=2 不折"
    );
    assert_eq!(res.stats.collapse, CollapseStats::default());
}

#[test]
fn custom_params_take_effect() {
    let rs = single_rule("t-c");
    // min_repeat=2: len=2 run 也折。
    let p = CollapseParams {
        max_gap: 10,
        min_repeat: 2,
    };
    let res = skim_with("-", &rs, &cascade_log(2, 1), &p);
    assert_eq!(res.blocks.len(), 1);
    assert_eq!(res.blocks[0].collapse.as_ref().unwrap().repeat_blocks, 1);
    // max_gap=0: 任何 run 不形成 (独立块间隔 ≥1)。
    let p0 = CollapseParams {
        max_gap: 0,
        min_repeat: 3,
    };
    let res0 = skim_with("-", &rs, &cascade_log(5, 1), &p0);
    assert_eq!(res0.blocks.len(), 5);
}

// ---- R4: 零行为变更保证 ----

#[test]
fn below_threshold_byte_identical_to_off() {
    // 阈值不满足 (len=2 run) → 默认参数输出与 OFF 逐字节一致。
    let rs = single_rule("t-c");
    let ls = cascade_log(2, 3);
    let default = skim("-", &rs, &ls);
    let off = skim_with("-", &rs, &ls, &CollapseParams::OFF);
    assert_eq!(to_json(&default), to_json(&off));
    // OFF 在超阈值级联上也与未折叠语义一致 (blocks 全保留)。
    let big = cascade_log(6, 2);
    let off_big = skim_with("-", &rs, &big, &CollapseParams::OFF);
    assert_eq!(off_big.blocks.len(), 6);
    assert_eq!(off_big.stats.collapse, CollapseStats::default());
}

#[test]
fn folded_json_has_collapse_and_off_has_none() {
    let rs = single_rule("t-c");
    let ls = cascade_log(4, 1);
    let on = to_json(&skim("-", &rs, &ls));
    assert!(on.contains("\"collapse\""));
    assert!(on.contains("\"folded_blocks\""));
    let off = to_json(&skim_with("-", &rs, &ls, &CollapseParams::OFF));
    assert!(!off.contains("collapse"), "OFF 输出零折叠字段");
}

// ---- R3: 统计口径 ----

#[test]
fn signature_full_scope_and_output_invariant() {
    // 签名级全量 (折叠块锚行照常) + output = 未折叠值 − omitted 不变式。
    let rs = single_rule("t-c");
    let ls = cascade_log(5, 1);
    let folded = skim("-", &rs, &ls);
    let unfolded = skim_with("-", &rs, &ls, &CollapseParams::OFF);
    assert_eq!(
        folded.stats.signature_output_lines, unfolded.stats.signature_output_lines,
        "签名级口径全量 — 折叠零影响"
    );
    assert_eq!(
        folded.output_lines,
        unfolded.output_lines - folded.stats.collapse.omitted_lines,
        "不变式: output_lines = 未折叠值 − omitted_lines (spec §3.R3)"
    );
}

#[test]
fn zero_block_predicate_intact_under_folding() {
    // 折叠永不把非空输出折为空 (锚块恒保留) — 零块判定不变。
    let rs = single_rule("t-c");
    let res = skim("-", &rs, &cascade_log(10, 2));
    assert!(!res.blocks.is_empty());
    assert!(res.blocks[0].collapse.is_some());
}

#[test]
fn noise_lines_excluded_from_omitted() {
    // 折叠块区间含 noise 行: omitted 按 noise 剔除口径计。
    let rs = flat(&[
        single_rule("t-a"),
        rules("id: t-n\ntitle: n\ntool: t\nkind: noise\npatterns:\n  - regex: '^N'\n"),
    ]);
    // 锚块 E1; 折叠块 E2 (与 E2 同块的 N 行是 noise 剔除); E3 — run len 3。
    let ls = lines(&["E1", "x", "E2", "N", "E3"]);
    let res = skim("-", &rs, &ls);
    assert_eq!(res.blocks.len(), 1);
    let m = res.blocks[0].collapse.as_ref().unwrap();
    assert_eq!(m.repeat_blocks, 2);
    // E2/E3 块各 1 行 (N 行是 noise 剔除) → omitted = 2 而非 3。
    assert_eq!(m.omitted_lines, 2);
    assert_eq!(res.stats.collapse.omitted_lines, 2);
}

#[test]
fn context_overlap_not_double_counted() {
    // 折叠块输出区间与锚块 context 重叠: 重叠行由幸存锚块呈现, 不计 omitted。
    let rs = rules(
        "id: t-x\ntitle: x\ntool: t\nkind: signal\ncontext: {after: 2}\npatterns:\n  - regex: '^E'\n",
    );
    // E1 (锚, 输出延至行3) | 间隔1行 | E2 E3: 块输出区间的 context 互相重叠。
    let ls = lines(&["E1", "p", "q", "E2", "r", "s", "E3"]);
    let res = skim("-", &rs, &ls);
    assert_eq!(res.blocks.len(), 1, "run len=3 折叠");
    let m = res.blocks[0].collapse.as_ref().unwrap();
    // 未折叠 output = 7 (三块输出区间 hull 覆盖全行); 折叠后锚块输出 1..3 → 3 行;
    // omitted = 7 − 3 = 4 (E2/E3 块的独有行 4..7), 重叠行 3 由锚块呈现不计。
    assert_eq!(res.output_lines, 3);
    assert_eq!(m.omitted_lines, 4);
    assert_eq!(res.stats.collapse.omitted_lines, 4);
}

// ---- R2: 人类可读折叠行 ----

#[test]
fn render_line_format_exact() {
    let m = log_skimmer::CollapseMarker {
        rule: "t-c".into(),
        intent: log_skimmer::Intent::Failure,
        repeat_blocks: 2,
        start_line: 3,
        end_line: 7,
        omitted_lines: 4,
        frag_spans: vec![(3, 4), (6, 7)],
    };
    assert_eq!(
        m.render_line(),
        "⟨×2 t-c 折叠块 [L3-L7] 省略 4 行; 锚行 L3 L6⟩"
    );
    // >8 锚行: 前 8 + …(共 N 行) 收尾。
    let big = log_skimmer::CollapseMarker {
        rule: "t-c".into(),
        intent: log_skimmer::Intent::Failure,
        repeat_blocks: 9,
        start_line: 3,
        end_line: 30,
        omitted_lines: 18,
        frag_spans: (0..10).map(|i| (3 + i * 3, 4 + i * 3)).collect(),
    };
    let line = big.render_line();
    assert!(
        line.starts_with("⟨×9 t-c 折叠块 [L3-L30] 省略 18 行; 锚行 L3 L6 L9 L12 L15 L18 L21 L24")
    );
    assert!(line.ends_with("…(共 10 行)⟩"));
}

// ---- R3: L0 全量口径恢复 (折叠块仍计有块, fragment 逐条计数) ----

#[test]
fn l0_stats_full_granularity_under_folding() {
    // 5 块 failure 规则级联: skim 折叠后 l0 collect 仍数出 5 fragment。
    let dir = std::env::temp_dir().join("ls-test-collapse-l0");
    let _ = std::fs::remove_dir_all(&dir);
    let rules_dir = dir.join("r");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        rules_dir.join("r.yaml"),
        "id: t-c\ntitle: t\ntool: t\nkind: signal\npatterns:\n  - regex: '^E'\n",
    )
    .unwrap();
    let corpus = dir.join("corpus/github.com/x/y/2026-10-22");
    std::fs::create_dir_all(&corpus).unwrap();
    let log = corpus.join("1-1-build-failure.log");
    std::fs::write(&log, cascade_log(5, 1).join("\n") + "\n").unwrap();
    std::fs::write(
        corpus.join("1-1-build-failure.log.json"),
        r#"{"repo":"github.com/x/y","conclusion":"failure","attempt":1}"#,
    )
    .unwrap();
    let collected = log_skimmer::l0::collect(&dir.join("corpus"), &rules_dir).unwrap();
    assert_eq!(collected.samples.len(), 1);
    let s = &collected.samples[0];
    // 全量口径: 5 fragment (折叠块照常), 非 1。
    assert_eq!(s.failure_frags, 5, "折叠块仍计有块 — FN 谓词不受折叠影响");
    assert_eq!(s.frags.len(), 5);
    assert_eq!(
        s.failure_frag_lines, 5,
        "FP 分子全量 (每 fragment 区间行数)"
    );
    assert_eq!(s.zero_block, 0);
    assert_eq!(s.signature_output_lines, 5, "签名级全量");
    // 呈现口径: 锚块 1 行。
    assert_eq!(s.output_lines, 1);
    // 压缩率分子 = 呈现行; 聚合后 comp_blk 按 1/9 计。
    let summary = log_skimmer::l0::summarize(&collected);
    assert_eq!(
        *summary.per_rule_hits.get("t-c").unwrap(),
        5,
        "per_rule_hits 全量 (N 非 1)"
    );
}

// ---- CLI 面: --no-collapse 与折叠行渲染 ----

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let bin = env!("CARGO_BIN_EXE_log-skimmer");
    let dir = std::env::temp_dir().join("ls-test-collapse-cli");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("r")).unwrap();
    std::fs::write(
        dir.join("r/r.yaml"),
        "id: t-c\ntitle: t\ntool: t\nkind: signal\npatterns:\n  - regex: '^E'\n",
    )
    .unwrap();
    let log = dir.join("casc.log");
    std::fs::write(&log, cascade_log(4, 1).join("\n") + "\n").unwrap();
    let rules_arg = dir.join("r");
    let mut argv = vec![
        bin,
        "skim",
        log.to_str().unwrap(),
        "--rules",
        rules_arg.to_str().unwrap(),
    ];
    argv.extend_from_slice(args);
    let out = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn cli_human_output_has_fold_line_and_stats_note() {
    let (stdout, _, code) = run_cli(&[]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("⟨×3 t-c 折叠块 [L3-L7] 省略 3 行; 锚行 L3 L5 L7⟩"),
        "折叠行格式 (spec §3.R2):\n{stdout}"
    );
    assert!(
        stdout.contains("repeat-collapse folded 3 blocks (-3 lines)"),
        "统计行折叠尾注:\n{stdout}"
    );
    let (off_out, _, off_code) = run_cli(&["--no-collapse"]);
    assert_eq!(off_code, 0);
    assert!(!off_out.contains("折叠块"), "--no-collapse 无折叠行");
    assert!(
        !off_out.contains("repeat-collapse"),
        "--no-collapse 无统计尾注"
    );
    assert_eq!(
        off_out.matches("--- [L").count(),
        4,
        "OFF: 每块一块头 (4 块)"
    );
}

#[test]
fn cli_json_collapse_field_presence() {
    let (on, _, _) = run_cli(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&on).unwrap();
    assert!(v["blocks"][0]["collapse"]["repeat_blocks"].is_number());
    assert!(v["stats"]["collapse"]["folded_blocks"].is_number());
    let (off, _, _) = run_cli(&["--json", "--no-collapse"]);
    let v: serde_json::Value = serde_json::from_str(&off).unwrap();
    assert!(v["blocks"][0].get("collapse").is_none());
    assert!(v["stats"].get("collapse").is_none());
}
