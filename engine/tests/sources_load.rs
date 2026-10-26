//! 内存源装载契约测试 ([load_rules_from_sources], wasm 宿主装载路径):
//! manifest §1.M 版本协商 (保留名经源列表传入) / 保留名与子目录同名派发 /
//! 空集拒绝 / 共用装载路径的校验与 id 去重 / 与目录式 [load_rules] 的等序等集。
//! 与 tests/manifest.rs (目录式) 互补 — 两形态共用 negotiate/load_rule_source,
//! 本文件锁定「内存源形态不得分叉」。

/// 最小合法规则 (spec §1 必填字段齐全, id 以 tool 为前缀)。
const RULE_A: &str =
    "id: src-x\ntitle: t\ntool: src\npatterns:\n  - regex: '^error'\nkind: signal\n";
const RULE_B: &str =
    "id: src-y\ntitle: t\ntool: src\nintent: info\npatterns:\n  - regex: '^Runner'\nkind: signal\n";

/// 版本匹配的合法 manifest 文本 (常量驱动 — bump FORMAT_VERSION 用例自动跟随)。
fn ok_manifest() -> String {
    format!(
        "format_version: {}\nmin_engine_version: 0.0.1\n",
        log_skimmer::manifest::FORMAT_VERSION
    )
}

fn src(name: &str, raw: &str) -> (String, String) {
    (name.to_string(), raw.to_string())
}

/// 同一规则集经临时目录 (load_rules) 与内存源 (load_rules_from_sources) 双路装载,
/// 断言等序等集 — 两形态共用装载路径的最强性质 (规则 id 序列即装载结果指纹)。
#[test]
fn sources_match_directory_load() {
    let dir = std::env::temp_dir().join("log-skimmer-sources-parity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("b.yaml"), RULE_A).unwrap();
    std::fs::write(dir.join("sub/a.yaml"), RULE_B).unwrap();
    std::fs::write(dir.join("manifest.yaml"), ok_manifest()).unwrap();
    let via_dir = log_skimmer::load_rules(&dir).unwrap_or_else(|e| panic!("目录式须装载: {e}"));
    let sources = vec![
        src("b.yaml", RULE_A),
        src("sub/a.yaml", RULE_B),
        src("manifest.yaml", &ok_manifest()),
    ];
    let via_sources = log_skimmer::load_rules_from_sources(&sources)
        .unwrap_or_else(|e| panic!("内存源式须装载: {e}"));
    fn ids(rules: &[log_skimmer::Rule]) -> Vec<&str> {
        rules.iter().map(|r| r.id.as_str()).collect()
    }
    assert_eq!(ids(&via_dir), ids(&via_sources), "两形态装载须等序等集");
    assert_eq!(via_dir.len(), 2, "manifest.yaml 不得计入规则");
}

#[test]
fn manifest_version_match_loads() {
    // 有 manifest 且版本匹配 → 正常装载; manifest 保留名不计入规则
    let sources = vec![src("manifest.yaml", &ok_manifest()), src("x.yaml", RULE_A)];
    let rules = log_skimmer::load_rules_from_sources(&sources)
        .unwrap_or_else(|e| panic!("版本匹配须装载: {e}"));
    assert_eq!(rules.len(), 1, "manifest 源不得计入规则");
    assert_eq!(rules[0].id, "src-x");
}

#[test]
fn format_version_future_rejected() {
    // format_version 未来值 → 拒载, 文案自解释 (与目录式 tests/manifest.rs 同款断言)
    let sources = vec![
        src(
            "manifest.yaml",
            &format!(
                "format_version: {}\n",
                log_skimmer::manifest::FORMAT_VERSION + 1
            ),
        ),
        src("x.yaml", RULE_A),
    ];
    let e = log_skimmer::load_rules_from_sources(&sources).expect_err("未来格式版本须拒载");
    let msg = e.to_string();
    assert!(msg.contains("format_version"), "须指明失败校验项: {msg}");
    assert!(msg.contains("升级引擎"), "须建议升级引擎: {msg}");
}

#[test]
fn min_engine_version_above_rejected() {
    let sources = vec![
        src(
            "manifest.yaml",
            "format_version: 1\nmin_engine_version: 999.0.0\n",
        ),
        src("x.yaml", RULE_A),
    ];
    let e = log_skimmer::load_rules_from_sources(&sources).expect_err("引擎下限不满足须拒载");
    let msg = e.to_string();
    assert!(
        msg.contains("min_engine_version"),
        "须指明失败校验项: {msg}"
    );
}

#[test]
fn manifest_parse_error_names_source() {
    // manifest 文本非法 → 拒载文案定位内存源名 (与目录式的路径定位对齐)
    let sources = vec![
        src("manifest.yaml", "format_version: 1\nfoo: 1\n"),
        src("x.yaml", RULE_A),
    ];
    let e = log_skimmer::load_rules_from_sources(&sources).expect_err("manifest 未知字段须拒载");
    assert!(
        e.to_string().contains("manifest.yaml"),
        "解析失败须定位源名: {e}"
    );
}

#[test]
fn empty_sources_rejected() {
    // 空列表 → 拒绝: 空规则集 = 引擎失明 (与「目录下无 *.yaml」同族)
    let e = log_skimmer::load_rules_from_sources(&[]).expect_err("空源列表须拒载");
    assert!(
        e.to_string().contains("引擎失明"),
        "文案须点明失明语义: {e}"
    );
}

#[test]
fn manifest_only_rejected() {
    // 仅 manifest 无规则源 → 拒绝 (manifest 不是规则; 与目录式 manifest-only 同族)
    let sources = vec![src(
        "manifest.yaml",
        &format!(
            "format_version: {}\n",
            log_skimmer::manifest::FORMAT_VERSION
        ),
    )];
    let e = log_skimmer::load_rules_from_sources(&sources).expect_err("仅 manifest 须拒载");
    assert!(
        e.to_string().contains("无规则源"),
        "文案须点明无规则源: {e}"
    );
}

#[test]
fn unknown_rule_field_rejected() {
    // 共用解析路径: 规则未知字段拒载 (deny_unknown_fields, spec §1.M)
    let sources = vec![src(
        "x.yaml",
        "id: src-x\ntitle: t\ntool: src\nbogus: 1\nkind: signal\n",
    )];
    let e = log_skimmer::load_rules_from_sources(&sources).expect_err("未知字段须拒载");
    assert!(e.to_string().contains("x.yaml"), "须定位违规源: {e}");
}

#[test]
fn id_dup_names_both_sources() {
    // 共用 id 去重: 报错并列首见源与冲突源 (issue #14 B 同款文案契约)
    let sources = vec![src("a.yaml", RULE_A), src("b.yaml", RULE_A)];
    let e = log_skimmer::load_rules_from_sources(&sources).expect_err("id 重复须拒载");
    let msg = e.to_string();
    assert!(msg.contains("src-x"), "须点明冲突 id: {msg}");
    assert!(
        msg.contains("a.yaml") && msg.contains("b.yaml"),
        "须并列两侧来源: {msg}"
    );
}

#[test]
fn subdir_manifest_name_is_rule_source() {
    // 保留名仅限根级: 子目录同名源 (a/manifest.yaml) 按规则源解析 — 与目录语义
    // (load_rules 仅对根级 manifest 免解析) 一致
    let sources = vec![src("a/manifest.yaml", RULE_A)];
    let rules = log_skimmer::load_rules_from_sources(&sources)
        .unwrap_or_else(|e| panic!("子目录同名源是普通规则源, 须装载: {e}"));
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id, "src-x");
}

#[test]
fn manifest_violation_precedes_empty_reject() {
    // 顺序等价: manifest 违例优先于空集判定 (与 load_rules 的检查顺序一致 —
    // 目录式 check 早返回, 内存式 negotiate 先行)
    let e = log_skimmer::load_rules_from_sources(&[src("manifest.yaml", "format_version: 999\n")])
        .expect_err("manifest 违例须先于空集暴露");
    assert!(
        e.to_string().contains("format_version"),
        "须是 manifest 错误而非无规则源: {e}"
    );
}
