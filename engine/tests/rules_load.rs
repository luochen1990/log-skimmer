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
