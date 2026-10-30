//! 规则集 manifest 兼容性契约测试 (spec §1.M「规则集 manifest」, issue #59):
//! 版本协商各分支 + manifest 保留名不进规则解析 + 规则未知字段拒载。
//! 全部走公共 [log_skimmer::load_rules] 装载链整体行为 (非 manifest 模块单点),
//! 引擎版本以构建期注入为准 (CARGO_PKG_VERSION)。

use std::path::PathBuf;

/// 最小合法规则 (spec §1 必填字段齐全)。刻意保留 v1 形态 (scope: global) —
/// 弃用窗口 (format_version 2) 下须零破坏装载, 本文件全部用例因此顺带锁定
/// 「带 scope 的旧规则文件经完整 load_rules 链正常装载且值被忽略」。
const RULE: &str = "id: test-x\ntitle: t\ntool: test\nscope: global\npatterns:\n  - regex: '^error'\nkind: signal\n";

/// 临时规则集目录: 任意规则体 (canonical RULE 覆盖不了的用例)。
fn ruleset_with(name: &str, rule: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("log-skimmer-manifest-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("test-x.yaml"), rule).unwrap();
    dir
}

/// 临时规则集目录: 单条合法规则 + 可选 manifest 内容。
fn ruleset(name: &str, manifest: Option<&str>) -> PathBuf {
    let dir = ruleset_with(name, RULE);
    if let Some(m) = manifest {
        std::fs::write(dir.join("manifest.yaml"), m).unwrap();
    }
    dir
}

#[test]
fn manifest_version_match_loads() {
    // 有 manifest 且版本匹配 (取引擎当前支持版本, 常量驱动 — bump FORMAT_VERSION
    // 即本用例自动跟随) → 正常装载; manifest 保留名不计入规则
    let dir = ruleset(
        "match",
        Some(&format!(
            "format_version: {}\nmin_engine_version: 0.0.1\n",
            log_skimmer::manifest::FORMAT_VERSION
        )),
    );
    let rules = log_skimmer::load_rules(&dir).unwrap_or_else(|e| panic!("版本匹配须装载: {e}"));
    assert_eq!(rules.len(), 1, "manifest.yaml 不得计入规则");
    assert_eq!(rules[0].id, "test-x");
}

#[test]
fn manifest_absent_loads() {
    // 无 manifest → 照常装载 (向后兼容窗口, spec §1.M — 现存规则集零破坏)
    let dir = ruleset("absent", None);
    assert!(
        log_skimmer::load_rules(&dir).is_ok(),
        "无 manifest 须维持现状"
    );
}

#[test]
fn format_version_future_rejected() {
    // format_version 未来值 (当前支持版本 +1, 常量驱动 — bump FORMAT_VERSION
    // 即本用例自动跟随) → 拒载, 文案自解释 (版本数值 + 升级建议)
    let dir = ruleset(
        "future-fmt",
        Some(&format!(
            "format_version: {}\n",
            log_skimmer::manifest::FORMAT_VERSION + 1
        )),
    );
    let e = log_skimmer::load_rules(&dir).expect_err("未来格式版本须拒载");
    let msg = e.to_string();
    assert!(msg.contains("format_version"), "须指明失败校验项: {msg}");
    assert!(msg.contains("upgrade the engine"), "须建议升级引擎: {msg}");
}

#[test]
fn min_engine_version_above_rejected() {
    // min_engine_version 高于当前引擎 → 拒载
    let dir = ruleset(
        "min-above",
        Some("format_version: 1\nmin_engine_version: 999.0.0\n"),
    );
    let e = log_skimmer::load_rules(&dir).expect_err("引擎下限不满足须拒载");
    let msg = e.to_string();
    assert!(
        msg.contains("min_engine_version"),
        "须指明失败校验项: {msg}"
    );
    assert!(msg.contains("999.0.0"), "须含需要的版本号: {msg}");
}

#[test]
fn min_engine_version_at_or_below_loads() {
    // min_engine_version 等于/低于引擎版本 (0.1.0 是本仓首个版本, 恒 ≤ 引擎) → 装载
    let dir = ruleset(
        "min-eq",
        Some("format_version: 1\nmin_engine_version: 0.1.0\n"),
    );
    assert!(
        log_skimmer::load_rules(&dir).is_ok(),
        "下限 ≤ 引擎版本须装载"
    );
}

#[test]
fn manifest_unknown_field_rejected() {
    // manifest 未知字段 → 拒载 (deny_unknown_fields, spec §1.M)
    let dir = ruleset("m-unknown", Some("format_version: 1\nfoo: bar\n"));
    let e = log_skimmer::load_rules(&dir).expect_err("manifest 未知字段须拒载");
    assert!(
        e.to_string().contains("manifest"),
        "须指明 manifest 非法: {e}"
    );
}

#[test]
fn manifest_bad_semver_rejected() {
    // min_engine_version 形态非法 (非完整三元组) → 拒载
    let dir = ruleset(
        "bad-semver",
        Some("format_version: 1\nmin_engine_version: 1.2\n"),
    );
    let e = log_skimmer::load_rules(&dir).expect_err("畸形 semver 须拒载");
    assert!(e.to_string().contains("1.2"), "须含非法值原文: {e}");
}

#[test]
fn rule_unknown_field_rejected() {
    // 规则未知字段 → 拒载: 静默忽略会让规则语义偏移无感知 (issue #59 核心)
    let dir = ruleset_with("rule-unknown", &format!("{RULE}future_field: 1\n"));
    let e = log_skimmer::load_rules(&dir).expect_err("规则未知字段须拒载");
    assert!(
        e.to_string().contains("future_field"),
        "未知字段名须出现在报错: {e}"
    );
}

#[test]
fn pattern_unknown_field_rejected() {
    // patterns[0] 内未知字段 → 拒载: pattern 级字段静默偏移是语义漂移入口
    // (如 loop 误写成 loops → 静默退化为非循环单扩展) — deny_unknown_fields
    // 三级同契约 (Rule/Pattern/Context, issue #59), 嵌套层级单独锁定 (issue #112:
    // 规则级已测, pattern/context 级拒载分支此前无测试触达)。
    let dir = ruleset_with(
        "pattern-unknown",
        "id: test-x\ntitle: t\ntool: test\nscope: global\nkind: signal\npatterns:\n  - regex: '^error'\n    bogus: 1\n",
    );
    let e = log_skimmer::load_rules(&dir).expect_err("pattern 未知字段须拒载");
    let msg = e.to_string();
    assert!(
        msg.contains("unknown field"),
        "须为 unknown field 语义: {msg}"
    );
    assert!(msg.contains("bogus"), "未知字段名须出现在报错: {msg}");
}

#[test]
fn context_unknown_field_rejected() {
    // context 内未知字段 → 拒载 (同上, Context 级 — 拼错的边界扩展字段
    // 静默归零会让输出区间悄悄收窄)。
    let dir = ruleset_with(
        "context-unknown",
        &format!("{RULE}context: {{before: 0, bogus: 1}}\n"),
    );
    let e = log_skimmer::load_rules(&dir).expect_err("context 未知字段须拒载");
    let msg = e.to_string();
    assert!(
        msg.contains("unknown field"),
        "须为 unknown field 语义: {msg}"
    );
    assert!(msg.contains("bogus"), "未知字段名须出现在报错: {msg}");
}

#[test]
fn rule_reserved_fields_accepted() {
    // spec §1 保留字段 structured-hint (YAML 连字符形态) / attribution 在场合法 (POC 不消费)
    let dir = ruleset_with(
        "reserved",
        &format!("{RULE}structured-hint: null\nattribution: {{contributor: someone}}\n"),
    );
    assert!(
        log_skimmer::load_rules(&dir).is_ok(),
        "保留字段在场须合法装载"
    );
}
