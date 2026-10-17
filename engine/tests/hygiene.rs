//! 发布面卫生检查测试 — publish 机械防线 (issue #18) 的 fail/pass 双路径回归。
//! fixture 模式用文档保留段/假域名 (零真实内网端点 — 测试文件自身可被扫描,
//! 防线代码与机密清单分离的设计约束, 见 engine/src/hygiene.rs 头注释)。
//! 覆盖: 清单装载校验 (注释/空行/空清单/重复/缺文件) / 干净树零命中 /
//! 全模式命中定位 (file:line:pattern) / 二进制跳过 (NUL 探测) / 精确行号 /
//! 符号链接不跟随。

use std::fs;
use std::path::{Path, PathBuf};

use log_skimmer::hygiene;

/// fixture 模式集 (TEST-NET 文档保留段 + 假域名 — 无真实内网情报)。
const FIXTURE_PATTERNS: &[&str] = &["example.intranet", "192.0.2.", "secret-host"];

/// 每用例独立 scratch 目录 (temp_dir 下前缀隔离; 重复运行幂等覆盖)。
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("log-skimmer-hygiene-{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn patterns() -> Vec<String> {
    FIXTURE_PATTERNS.iter().map(|s| s.to_string()).collect()
}

#[test]
fn load_patterns_parses_comments_and_blank_lines() {
    let d = scratch("load-ok");
    let f = d.join("patterns.txt");
    fs::write(
        &f,
        "# 注释\n\nexample.intranet\n  192.0.2.  \nsecret-host\n",
    )
    .unwrap();
    let mut expect = patterns();
    expect.sort_unstable();
    assert_eq!(hygiene::load_patterns(&f).unwrap(), expect);
}

#[test]
fn load_patterns_rejects_empty_duplicate_and_missing() {
    let d = scratch("load-bad");
    let empty = d.join("empty.txt");
    fs::write(&empty, "# 只有注释\n\n").unwrap();
    assert!(
        hygiene::load_patterns(&empty).is_err(),
        "空清单 = 防线失效, 拒绝"
    );

    let dup = d.join("dup.txt");
    fs::write(&dup, "example.intranet\nexample.intranet\n").unwrap();
    assert!(
        hygiene::load_patterns(&dup).is_err(),
        "重复模式是笔误信号, 拒绝"
    );

    // 行内注释 (gitignore 事故同款笔误形态): 整行成为含空格的字面模式, 永不匹配
    // → 单条防线静默失效, 装载时拒绝 (注释须独占一行)
    let inline = d.join("inline.txt");
    fs::write(&inline, "example.intranet # 主域\n").unwrap();
    assert!(hygiene::load_patterns(&inline).is_err(), "模式行内 # 拒绝");

    assert!(
        hygiene::load_patterns(&d.join("absent.txt")).is_err(),
        "缺文件拒绝"
    );
}

#[test]
fn clean_tree_passes() {
    let d = scratch("clean");
    write(&d, "README.md", "# log skimmer\n公开内容\n");
    write(&d, "engine/src/lib.rs", "pub fn f() {}\n");
    write(
        &d,
        "docs/nested/deep.md",
        "github.com/luochen1990/log-skimmer\n",
    );
    assert!(hygiene::scan(&d, &patterns()).is_empty(), "干净树不得命中");
}

#[test]
fn every_pattern_fails_with_location() {
    let d = scratch("patterns");
    for (i, pat) in FIXTURE_PATTERNS.iter().enumerate() {
        write(&d, &format!("f{i}.txt"), &format!("第1行\n含 {pat} 的行\n"));
    }
    let hits = hygiene::scan(&d, &patterns());
    assert_eq!(
        hits.len(),
        FIXTURE_PATTERNS.len(),
        "每模式恰好一命中: {hits:?}"
    );
    for (i, pat) in FIXTURE_PATTERNS.iter().enumerate() {
        let h = hits
            .iter()
            .find(|h| h.pattern == *pat && h.path.ends_with(format!("f{i}.txt")))
            .unwrap_or_else(|| panic!("模式 {pat} 未被报告: {hits:?}"));
        assert_eq!(h.line, 2, "行号须为命中行: {h:?}");
    }
}

#[test]
fn binary_files_are_skipped() {
    let d = scratch("binary");
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47];
    bytes.extend_from_slice(b"example.intranet");
    bytes.push(0); // NUL → 二进制探测
    fs::write(d.join("logo.png"), bytes).unwrap();
    assert!(
        hygiene::scan(&d, &patterns()).is_empty(),
        "二进制文件跳过, 不计命中"
    );
}

#[test]
fn multi_line_hit_reports_exact_line() {
    let d = scratch("multiline");
    write(&d, "a.md", "l1\nl2\nl3 has example.intranet inside\nl4\n");
    let hits = hygiene::scan(&d, &patterns());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 3);
}

#[test]
fn symlink_targets_are_not_followed() {
    let d = scratch("symlink");
    let outside = scratch("symlink-outside");
    write(&outside, "secret.txt", "example.intranet 敏感内容\n");
    std::os::unix::fs::symlink(outside.join("secret.txt"), d.join("leak.txt")).unwrap();
    assert!(
        hygiene::scan(&d, &patterns()).is_empty(),
        "符号链接不跟随 (扫描域=发布树自身)"
    );
}
