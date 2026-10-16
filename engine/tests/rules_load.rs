//! 规则集加载回归测试 — 全量 `rules/**/*.yaml` 走引擎加载校验 (spec §1: 解析 + 字段约束 +
//! regex 编译 + id 唯一, 违例整体拒绝), 防规则语法漂移进仓。
//! 说明: `rules/_template.yaml` 是自明占位锚的合法规则 (贡献起点), 也在加载覆盖内;
//! 复制模板忘改 id 会被此处的 id 唯一性断言拦下。

use std::path::PathBuf;

#[test]
fn all_rules_load_with_unique_ids() {
    let rules_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rules");
    let rules = log_skimmer::load_rules(&rules_dir)
        .unwrap_or_else(|e| panic!("规则集非法 (语法漂移或字段违例): {e}"));

    assert!(
        !rules.is_empty(),
        "规则集为空 — 至少应含双侧平台安全网与模板规则"
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

    // 模板自身必须可加载且 id 未偏离约定 (贡献起点; 加载失败 = 模板与引擎校验漂移)
    assert!(
        rules.iter().any(|r| r.id == "template-my-rule"),
        "rules/_template.yaml 未被加载或 id 已偏离模板约定"
    );
}
