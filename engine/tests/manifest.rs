//! 规则集 manifest 兼容性契约测试 (spec §1.M「规则集 manifest」, issue #59):
//! 版本协商各分支 + manifest 保留名不进规则解析 + 规则未知字段拒载。
//! 全部走公共 [log_skimmer::load_rules] 装载链整体行为 (非 manifest 模块单点),
//! 引擎版本以构建期注入为准 (CARGO_PKG_VERSION)。

use std::path::PathBuf;

/// 最小合法规则 (spec §1 必填字段齐全)。
const RULE: &str = "id: test-x\ntitle: t\ntool: test\nscope: global\npatterns:\n  - regex: '^error'\nkind: signal\n";

/// 临时规则集目录: 单条合法规则 + 可选 manifest 内容。
fn ruleset(name: &str, manifest: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("log-skimmer-manifest-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("test-x.yaml"), RULE).unwrap();
    if let Some(m) = manifest {
        std::fs::write(dir.join("manifest.yaml"), m).unwrap();
    }
    dir
}

#[test]
fn manifest_version_match_loads() {
    // 有 manifest 且版本匹配 → 正常装载; manifest 保留名不计入规则
    let dir = ruleset(
        "match",
        Some("format_version: 1\nmin_engine_version: 0.0.1\n"),
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
    // format_version 未来值 → 拒载, 文案自解释 (版本数值 + 升级建议)
    let dir = ruleset("future-fmt", Some("format_version: 2\n"));
    let e = log_skimmer::load_rules(&dir).expect_err("未来格式版本须拒载");
    let msg = e.to_string();
    assert!(msg.contains("format_version"), "须指明失败校验项: {msg}");
    assert!(msg.contains("升级引擎"), "须建议升级引擎: {msg}");
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
    let dir = std::env::temp_dir().join("log-skimmer-manifest-rule-unknown");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("test-x.yaml"), format!("{RULE}future_field: 1\n")).unwrap();
    let e = log_skimmer::load_rules(&dir).expect_err("规则未知字段须拒载");
    assert!(
        e.to_string().contains("future_field"),
        "未知字段名须出现在报错: {e}"
    );
}

#[test]
fn rule_reserved_fields_accepted() {
    // spec §1 保留字段 structured-hint (YAML 连字符形态) / attribution 在场合法 (POC 不消费)
    let dir = std::env::temp_dir().join("log-skimmer-manifest-reserved");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("test-x.yaml"),
        format!("{RULE}structured-hint: null\nattribution: {{contributor: someone}}\n"),
    )
    .unwrap();
    assert!(
        log_skimmer::load_rules(&dir).is_ok(),
        "保留字段在场须合法装载"
    );
}
