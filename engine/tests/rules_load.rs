//! 规则集加载回归测试 — fixture 规则集走引擎加载校验 (spec §1: 解析 + 字段约束 +
//! regex 编译 + id 唯一, 违例整体拒绝), 防引擎加载逻辑漂移。
//! 真实规则集 (log-skimmer-rules 仓, 独立成仓后主仓不携带) 的合法性由 L0 链把守:
//! l0-collect 调引擎加载全量规则, 单条违例整集拒载 fail-fast — CI 每轮覆盖。
//! env RULES_DIR 可覆盖 fixture 路径 (指向真实规则集副本跑同一断言族)。

use std::path::PathBuf;

#[test]
fn all_rules_load_with_unique_ids() {
    let rules_dir = std::env::var("RULES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules"));
    let rules = log_skimmer::load_rules(&rules_dir)
        .unwrap_or_else(|e| panic!("规则集非法 (语法漂移或字段违例): {e}"));

    assert!(
        rules.len() >= 2,
        "规则集异常稀少 ({}) — fixture 缺失或路径漂移",
        rules.len()
    );

    // 隐藏路径跳过 (issue #8): .github/ 是仓级元数据而非规则 — fixture 镜像真实
    // 规则仓布局 (.github/_template.yaml 在场), 装载结果不得含模板 id。
    assert!(
        !rules.iter().any(|r| r.id == "template-my-rule"),
        "隐藏路径 (.github/) 内的模板不得被装载为生产规则"
    );

    // id 全局唯一 (load_rules 已校验 Err 路径; 此处显式断言契约, 防 lib 内部实现演进松动)
    let mut ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    let dups: Vec<&str> = ids
        .windows(2)
        .filter(|w| w[0] == w[1])
        .map(|w| w[0])
        .collect();
    assert!(dups.is_empty(), "规则 id 重复: {dups:?}");
}

/// 空集防线: 目录不存在 / 目录存在但零 *.yaml 必须硬拒绝 (静默空集 = 引擎失明,
/// L0 聚合侧表现为全样本 100% 压缩假象 — rules 仓 lint 路径分叉事故的教训)。
#[test]
fn empty_rules_dir_rejected() {
    let e = log_skimmer::load_rules(std::path::Path::new("/nonexistent-rules-dir"))
        .expect_err("不存在的规则目录必须 Err");
    assert!(
        e.to_string().contains("rules directory not found"),
        "got: {e}"
    );

    let empty = std::env::temp_dir().join("log-skimmer-empty-rules");
    std::fs::create_dir_all(&empty).unwrap();
    let e = log_skimmer::load_rules(&empty).expect_err("零 yaml 目录必须 Err");
    assert!(e.to_string().contains("no *.yaml files"), "got: {e}");
}

/// #47a: 路径存在但非目录 (如误指某个 yaml 文件) — 如实报「不是目录」,
/// 不再误报「目录不存在」(与事实相悖, 误导排障方向)。
#[test]
fn rules_path_not_a_dir_rejected_truthfully() {
    let f = std::env::temp_dir().join("log-skimmer-rules-not-dir.yaml");
    std::fs::write(&f, "x").unwrap(); // 内容无关 — 判定只看路径类型
    let e = log_skimmer::load_rules(&f).expect_err("文件路径 (非目录) 必须 Err");
    let msg = e.to_string();
    assert!(msg.contains("is not a directory"), "应如实报非目录: {msg}");
    assert!(
        !msg.contains("not found"),
        "路径在场时不得误报不存在: {msg}"
    );
}

/// 规则 context 字段上界跟随 engine 单源常量 (issue #28): 恰好 CONTEXT_MAX 合法,
/// 超一档整集拒载 — 边界值从常量派生, 改常量即本测试边界同步变。
#[test]
fn rule_context_bound_follows_engine_const() {
    let dir = std::env::temp_dir().join("log-skimmer-ctx-bound");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let rule_yaml = |n: u32| {
        format!(
            "id: t-x\ntitle: x\ntool: t\nkind: signal\ncontext: {{before: {n}, after: {n}}}\npatterns:\n  - regex: '^x'\n"
        )
    };
    std::fs::write(dir.join("r.yaml"), rule_yaml(log_skimmer::CONTEXT_MAX)).unwrap();
    log_skimmer::load_rules(&dir).unwrap_or_else(|e| panic!("context=CONTEXT_MAX 须合法: {e}"));

    std::fs::write(dir.join("r.yaml"), rule_yaml(log_skimmer::CONTEXT_MAX + 1)).unwrap();
    let e = log_skimmer::load_rules(&dir).expect_err("context 超上界必须整集拒载");
    assert!(e.to_string().contains("context out of range"), "got: {e}");
    let _ = std::fs::remove_dir_all(&dir);
}
