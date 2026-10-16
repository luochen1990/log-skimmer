//! 状态机公理单测 (spec §2 A1-A8) + 加载校验单测 (spec §1) + 端到端组合。
//! 职责边界: 语义回归防线 — 每次 skim 语义变更必须先在这里红。

use log_skimmer::*;
use std::path::PathBuf;

/// 从内联 YAML 构造规则的测试助手 (绕过文件系统)。
fn rules(yaml: &str) -> Vec<Rule> {
    let r: Rule = serde_yaml::from_str(yaml).expect("测试规则 YAML 应合法");
    // 测试规则直接视为已校验 (regex 编译在 validate 中做, 这里补编译)
    let mut r = r;
    for p in &mut r.patterns {
        p.compiled = Some(regex::Regex::new(&p.regex).unwrap());
    }
    vec![r]
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// 多组单规则测试数组展平。
fn flat(vs: &[Vec<Rule>]) -> Vec<Rule> {
    vs.iter().flatten().cloned().collect()
}

fn blocks_of(res: &SkimResult) -> Vec<(usize, usize, Vec<&str>)> {
    res.blocks
        .iter()
        .map(|b| {
            (
                b.start_line,
                b.end_line,
                b.rules.iter().map(|x| x.as_str()).collect(),
            )
        })
        .collect()
}

// A1 独立扫描: 两规则互不干扰, 各自产出。
#[test]
fn a1_independent_machines() {
    let rs = [
        rules(
            "id: t-a\ntitle: a\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^A'\n",
        ),
        rules(
            "id: t-b\ntitle: b\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^B'\n",
        ),
    ];
    let res = skim("-", &flat(&rs), &lines(&["A1", "x", "B1", "x", "A2"]));
    assert_eq!(
        blocks_of(&res),
        vec![
            (1, 1, vec!["t-a"]),
            (3, 3, vec!["t-b"]),
            (5, 5, vec!["t-a"])
        ],
        "A/B 各自独立开块 (行间隔开以排除 A5 合并)"
    );
}

// A2 锚是开块唯一途径: 无锚匹配则零产出; 锚单行即成块。
#[test]
fn a2_anchor_only_entry() {
    let rs = rules("id: t-e\ntitle: e\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^error'\n  - regex: '^ '\n    loop: true\n");
    let res0 = skim("-", &rs, &lines(&["no match here", "xx"]));
    assert_eq!(res0.blocks.len(), 0);
    let res1 = skim("-", &rs, &lines(&["x", "error: boom"]));
    assert_eq!(
        blocks_of(&res1),
        vec![(2, 2, vec!["t-e"])],
        "仅锚匹配 → 单行块"
    );
}

// A3 扩展指针: loop 贪心停留 + 不命中透传下一 pattern 同行再试 + 越界闭块。
#[test]
fn a3_pointer_loop_and_passthrough() {
    let rs = rules(
        "id: t-p\ntitle: p\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^ERR'\n  - regex: '^\\+'\n    loop: true\n  - regex: '^~'\n",
    );
    // loop 贪心: ++++ 连续消费; ~ 不命中 loop pattern 但透传到 pattern[2] 命中 (非 loop, 指针走尽)。
    let res = skim("-", &rs, &lines(&["ERR", "++", "++", "~ctx", "++", "tail"]));
    // 块 = ERR..~ctx (第4行后指针=3 越界, 第5行 '++' 闭块)
    assert_eq!(
        res.blocks[0].fragments[0].block,
        (1, 4),
        "贪心消费 + 透传命中 + 越界闭块"
    );
    // 第5行 '++' 不再属于块 (闭块后当前行不回收)
    assert_eq!(res.blocks.len(), 1);
}

// A3 推论: 仅锚匹配、零扩展命中也闭块产出 (单行块, 无断链惩罚) — 已在 a2 覆盖,
// 此处验证锚后紧跟不匹配行时块恰为锚行。
#[test]
fn a3_no_extension_still_emits() {
    let rs = rules("id: t-s\ntitle: s\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^E: '\n  - regex: '^\\s'\n    loop: true\n");
    let res = skim("-", &rs, &lines(&["E: x", "plain", "E: y"]));
    assert_eq!(
        blocks_of(&res),
        vec![(1, 1, vec!["t-s"]), (3, 3, vec!["t-s"])]
    );
}

// A4 context 扩展: 只扩输出区间, 不吞并后续行进状态机, clamp 到边界。
#[test]
fn a4_context_extends_output_only() {
    let rs = rules(
        "id: t-c\ntitle: c\ntool: t\nscope: global\nkind: signal\ncontext: {before: 1, after: 2}\npatterns:\n  - regex: '^boom'\n",
    );
    let res = skim("-", &rs, &lines(&["a", "b", "boom", "c", "d", "e", "f"]));
    assert_eq!(
        res.blocks[0].fragments[0].block,
        (3, 3),
        "block 不受 context 影响"
    );
    assert_eq!(
        (res.blocks[0].start_line, res.blocks[0].end_line),
        (2, 5),
        "输出区间 = block±context"
    );
    // clamp: 锚在第 1 行时 before 不越界
    let res2 = skim("-", &rs, &lines(&["boom", "x", "y"]));
    assert_eq!(res2.blocks[0].start_line, 1);
}

// A5 合并: block 重叠或相邻 (gap ≤ 1 行) 合并, 输出块区间 = 成员输出区间 hull。
#[test]
fn a5_merge_overlap_and_adjacent() {
    let rs = [
        rules("id: t-m1\ntitle: m\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^A'\n"),
        rules("id: t-m2\ntitle: m\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^B'\n"),
    ];
    // A(1) B(2) 相邻 → 合并; C 隔 2 行 → 独立
    let res = skim("-", &flat(&rs), &lines(&["A", "B", "z", "z", "C"]));
    // C 无规则, 只 A B 合并
    assert_eq!(res.blocks.len(), 1);
    assert_eq!(res.blocks[0].rules.len(), 2);
    // 紧邻 (block_end+1 == next_start, 无间隔行) → 合并; 隔 1 行 → 不合并
    let rs3 = [
        rules("id: t-m1\ntitle: m\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^A'\n"),
        rules("id: t-m3\ntitle: m\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^C'\n"),
    ];
    let res3 = skim("-", &flat(&rs3), &lines(&["A", "C"]));
    assert_eq!(res3.blocks.len(), 1, "block 紧邻合并");
    let res4 = skim("-", &flat(&rs3), &lines(&["A", "z", "C"]));
    assert_eq!(res4.blocks.len(), 2, "block 隔 1 行不合并");
}

// A6 noise 剔除: noise 覆盖行从 signal 呈现中剔除; signal 全覆盖则丢弃。
#[test]
fn a6_noise_dedup() {
    let rs = [
        rules("id: t-sig\ntitle: s\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^SIG'\n"),
        rules("id: t-noi\ntitle: n\ntool: t\nscope: global\nkind: noise\npatterns:\n  - regex: '^NOI'\n"),
    ];
    let res = skim("-", &flat(&rs), &lines(&["SIG head", "NOI junk", "tail"]));
    // SIG(1) 与 tail 无 signal → 只有 SIG 块; NOI 行在块外不影响其呈现
    assert_eq!(res.blocks.len(), 1);
    // signal 块被 noise 完全覆盖 → 丢弃
    let res2 = skim("-", &flat(&rs), &lines(&["NOI x", "SIG gone", "NOI y"]));
    // SIG(2) 是 signal 单行块, 但未被 noise 覆盖 (noise 在 1,3) → 保留
    assert_eq!(res2.blocks.len(), 1);
    // noise 与 signal 同行起始: noise 覆盖 SIG 行本身
    let rs3 = [
        rules("id: t-sig\ntitle: s\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^SIG'\n"),
        rules("id: t-noi\ntitle: n\ntool: t\nscope: global\nkind: noise\npatterns:\n  - regex: '^SIG'\n"),
    ];
    let res3 = skim("-", &flat(&rs3), &lines(&["SIG only"]));
    assert_eq!(res3.blocks.len(), 0, "signal block 全被 noise 覆盖 → 丢弃");
    assert_eq!(res3.stats.noise_lines_removed, 1);
}

// A7 输出序: 块按行序排列 (稳定性)。
#[test]
fn a7_output_order() {
    let rs = rules(
        "id: t-o\ntitle: o\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^X'\n",
    );
    let res = skim("-", &rs, &lines(&["a", "X1", "b", "X2"]));
    let spans: Vec<_> = res.blocks.iter().map(|b| b.start_line).collect();
    assert_eq!(spans, vec![2, 4]);
}

// A8 正则约束: regex crate 编译失败在加载期拒绝。
#[test]
fn a8_regex_compile_gate() {
    let dir = std::env::temp_dir().join("ls-test-a8");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bad.yaml"),
        "id: t-bad\ntitle: b\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '(?<=x)y'\n",
    )
    .unwrap();
    let err = load_rules(&dir).expect_err("lookahead 须被 regex crate 拒绝");
    assert!(err.0.iter().any(|e| e.contains("编译失败")));
}

// 加载校验: 重复 id / scope-repo 互斥 / patterns 空。
#[test]
fn load_validation_errors() {
    let dir = std::env::temp_dir().join("ls-test-load");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("t")).unwrap();
    std::fs::write(
        dir.join("t/a.yaml"),
        "id: t-dup\ntitle: a\ntool: t\nscope: repo\nkind: signal\npatterns:\n  - regex: '^a'\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("t/b.yaml"),
        "id: t-dup\ntitle: b\ntool: t\nscope: global\nrepo: o/r\nkind: signal\npatterns:\n  - regex: '^b'\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("t/c.yaml"),
        "id: t-empty\ntitle: c\ntool: t\nscope: global\nkind: signal\npatterns: []\n",
    )
    .unwrap();
    let err = load_rules(&dir).unwrap_err();
    assert!(err.0.iter().any(|e| e.contains("id 重复")));
    assert!(err.0.iter().any(|e| e.contains("scope=repo 但缺")));
    assert!(err.0.iter().any(|e| e.contains("scope=global 但 repo")));
    assert!(err.0.iter().any(|e| e.contains("patterns 为空")));
}

// 端到端: 锚 + loop + context + noise 组合。
#[test]
fn e2e_anchor_loop_context_noise() {
    let rs = [
        rules(
            "id: t-err\ntitle: err\ntool: t\nscope: global\nkind: signal\ncategory: build\nconfidence: high\ncontext: {after: 1}\npatterns:\n  - regex: '^error: '\n  - regex: '^  '\n    loop: true\n",
        ),
        rules("id: t-dl\ntitle: dl\ntool: t\nscope: global\nkind: noise\npatterns:\n  - regex: '^downloading '\n    loop: true\n"),
    ];
    let res = skim(
        "-",
        &flat(&rs),
        &lines(&[
            "downloading pkg1", // 1 noise
            "downloading pkg2", // 2 noise
            "error: undefined", // 3 锚
            "  at foo.nix:1:2", // 4 loop
            "  frame more",     // 5 loop
            "downloading x",    // 6 noise (闭块行, 不属于 signal 块)
            "done",             // 7 (context.after 把块输出延到 6, 但 6 被 noise 剔)
        ]),
    );
    assert_eq!(res.blocks.len(), 1);
    let f = &res.blocks[0].fragments[0];
    assert_eq!(f.block, (3, 5));
    assert_eq!(f.out, (3, 6), "context.after=1 → 输出区间 3..6");
    assert_eq!(
        f.text, "error: undefined\n  at foo.nix:1:2\n  frame more",
        "noise 行从 text 剔除"
    );
    assert_eq!(res.stats.noise_lines_removed, 3);
    assert_eq!(res.stats.input_lines, 7);
    assert_eq!(res.stats.boundary_violations.len(), 0);
}

// 真实规则集冒烟: 加载 rules/ 全部 18 条须通过校验 (相对路径, cargo test 在 engine/ 跑)。
#[test]
fn real_rules_load() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rules");
    if !dir.exists() {
        eprintln!("(跳过: 未找到真实规则目录)");
        return;
    }
    let rs = load_rules(&dir).expect("真实规则集应全部通过加载校验");
    assert!(rs.len() >= 15, "规则数: {}", rs.len());
}

// intent 语义 (spec §1): signal 缺省默认 Failure; 显式 warning 继承到 fragment;
// noise 规则显式 intent 被加载校验拒绝。
#[test]
fn intent_defaults_and_inheritance() {
    let def = rules(
        "id: t-i1\ntitle: i\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^E'\n",
    );
    let res = skim("-", &def, &lines(&["E1"]));
    assert_eq!(
        res.blocks[0].fragments[0].intent,
        Intent::Failure,
        "缺省默认 failure"
    );

    let warn = rules(
        "id: t-i2\ntitle: i\ntool: t\nscope: global\nkind: signal\nintent: warning\npatterns:\n  - regex: '^W: '\n",
    );
    let res2 = skim("-", &warn, &lines(&["W: unused"]));
    assert_eq!(
        res2.blocks[0].fragments[0].intent,
        Intent::Warning,
        "显式 warning 继承"
    );

    let dir = std::env::temp_dir().join("ls-test-intent");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bad.yaml"),
        "id: t-bad\ntitle: b\ntool: t\nscope: global\nkind: noise\nintent: failure\npatterns:\n  - regex: '^n'\n",
    )
    .unwrap();
    let err = load_rules(&dir).unwrap_err();
    assert!(err.0.iter().any(|e| e.contains("noise 时 intent 必须缺省")));
}

// 签名行级粒度 (spec §3): signature_output_lines = 各 fragment 锚行去重计数。
#[test]
fn signature_granularity_stats() {
    let rs = rules(
        "id: t-sg\ntitle: s\ntool: t\nscope: global\nkind: signal\npatterns:\n  - regex: '^ERR'\n  - regex: '^ '\n    loop: true\n",
    );
    // 两个块: 锚行 1 和 5; 第二块 3 行
    let res = skim("-", &rs, &lines(&["ERR", " a", " b", "x", "ERR", " c"]));
    assert_eq!(res.stats.output_lines, 5, "块级: 2+3 行");
    assert_eq!(res.stats.signature_output_lines, 2, "签名级: 两个锚行");
}

// --context 覆盖 (spec §3): 显示层全局覆盖, 不改规则文件。
#[test]
fn context_override() {
    let rs = rules("id: t-co\ntitle: c\ntool: t\nscope: global\nkind: signal\ncontext: {before: 0, after: 0}\npatterns:\n  - regex: '^boom'\n");
    let overridden = override_context(&rs, 1);
    let res = skim("-", &overridden, &lines(&["a", "boom", "b"]));
    assert_eq!(
        (res.blocks[0].start_line, res.blocks[0].end_line),
        (1, 3),
        "覆盖后 before/after=1"
    );
    let res_orig = skim("-", &rs, &lines(&["a", "boom", "b"]));
    assert_eq!(
        (res_orig.blocks[0].start_line, res_orig.blocks[0].end_line),
        (2, 2),
        "原规则不受影响"
    );
}

