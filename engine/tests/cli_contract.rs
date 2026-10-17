//! CLI 输出契约 e2e (issues #10/#13/#16/#17): 退出码族 / stdout-stderr 流分离 / 人类可读渲染形态。
//! 真进程形态 (CARGO_BIN_EXE): 断言对象是 main.rs 渲染层 (用户可见面), 纯函数层已由
//! axioms.rs 覆盖 — 此处只锁「调用方契约」, 不重复引擎语义断言。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// failure 规则 (intent 缺省默认 failure, category=test)。
const RULE_FAILURE: &str = "id: t-hit\ntitle: h\ntool: t\nscope: global\nkind: signal\ncategory: test\npatterns:\n  - regex: '^ERR'\n";
/// warning 规则 (相邻行可与 RULE_FAILURE 合并进同块, 验证代表 fragment 选择)。
const RULE_WARNING: &str = "id: t-warn\ntitle: w\ntool: t\nscope: global\nkind: signal\nintent: warning\ncategory: lint\npatterns:\n  - regex: '^WARN'\n";

/// 唯一 temp 目录下建规则目录 + 日志文件, 返回 (rules_dir, log_path)。
fn fixture(name: &str, rule_yamls: &[&str], log_content: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("ls-cli-contract-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let rules = dir.join("t");
    std::fs::create_dir_all(&rules).unwrap();
    for (i, y) in rule_yamls.iter().enumerate() {
        std::fs::write(rules.join(format!("r{i}.yaml")), y).unwrap();
    }
    let log = dir.join("sample.log");
    std::fs::write(&log, log_content).unwrap();
    (rules, log)
}

fn run_skim(rules: &Path, log: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_log-skimmer"))
        .args([
            "skim",
            log.to_str().unwrap(),
            "--rules",
            rules.to_str().unwrap(),
        ])
        .args(extra)
        .output()
        .unwrap()
}

// #13: 空日志 (0 行) = 数据错误族 → exit 2 硬拒绝, 文案点明疑似下载截断。
// 与「空规则集硬拒绝」对称: 宁可显式失败, 不静默通过。
#[test]
fn empty_log_rejected_exit_2() {
    let (rules, log) = fixture("empty13", &[RULE_FAILURE], "");
    let out = run_skim(&rules, &log, &[]);
    assert_eq!(out.status.code(), Some(2), "空日志 → exit 2 (数据错误族)");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("日志为空 (0 行)"), "stderr: {stderr}");
    assert!(stderr.contains("下载截断"), "stderr: {stderr}");
    assert!(
        !stderr.contains("失明"),
        "空日志是数据错误, 不进入零块 (exit 3) 判定; stderr: {stderr}"
    );
}

// #10: 人类模式统计行连同前导空行与块内容同走 stdout — `> out.txt` 完整保留输出成果;
// 有块场景无诊断信息, stderr 应为空。
#[test]
fn human_mode_stats_line_on_stdout() {
    let (rules, log) = fixture("stats10", &[RULE_FAILURE], "ERR boom\n");
    let out = run_skim(&rules, &log, &[]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("input 1 lines -> output 1 lines (0.0% compression)"),
        "stdout 尾部应含统计行: {stdout}"
    );
    assert!(
        stdout.contains("\n\ninput 1 lines"),
        "统计行前应有前导空行 (分隔块内容与统计区): {stdout:?}"
    );
    assert!(
        out.stderr.is_empty(),
        "人类模式有块场景 stderr 应无输出; 实际: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// #10 守护: --json 模式流分离维持现状 — stdout 纯 JSON, 统计行走 stderr。
#[test]
fn json_mode_stream_separation_unchanged() {
    let (rules, log) = fixture("json10", &[RULE_FAILURE], "ERR boom\n");
    let out = run_skim(&rules, &log, &["--json"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim_start().starts_with('{'),
        "stdout 应为纯 JSON: {stdout}"
    );
    assert!(
        !stdout.contains("compression)"),
        "统计行不得污染 JSON stdout: {stdout}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("compression"),
        "统计行维持 stderr (诊断流)"
    );
}

// #10 + #17 组合: --json 零块场景 — stdout 仍纯 JSON, n/a 统计行走 stderr。
#[test]
fn json_mode_zero_block_na_stats_on_stderr() {
    let (rules, log) = fixture("jsonz10", &[RULE_FAILURE], "nothing\n");
    let out = run_skim(&rules, &log, &["--json"]);
    assert_eq!(out.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim_start().starts_with('{'),
        "零块 --json stdout 应为纯 JSON: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("(n/a compression)"), "stderr: {stderr}");
    assert!(stderr.contains("失明"), "stderr: {stderr}");
}

// #17-A: 零块时压缩率无意义 → 统计行显示 n/a (拒绝 100.0% 伪统计), 统计行本身保留。
#[test]
fn zero_block_compression_is_na() {
    let (rules, log) = fixture("na17", &[RULE_FAILURE], "all good\nno errors here\n");
    let out = run_skim(&rules, &log, &[]);
    assert_eq!(out.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("input 2 lines -> output 0 lines (n/a compression)"),
        "统计行保留且百分比为 n/a: {stdout}"
    );
    assert!(
        !stdout.contains("100.0%") && !stderr.contains("100.0%"),
        "零块不得出现 100.0% 伪统计; stdout: {stdout} stderr: {stderr}"
    );
}

// #17-B: 用户可见文案 (help / 错误 / 警告) 不含 "spec §N" 内部文档指针。
#[test]
fn user_visible_output_has_no_spec_refs() {
    let help = Command::new(env!("CARGO_BIN_EXE_log-skimmer"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        !help_text.contains("spec §"),
        "--help 不得引用内部 spec 章节: {help_text}"
    );

    let (rules, log) = fixture("spec17", &[RULE_FAILURE], "nothing\n");
    let zero = run_skim(&rules, &log, &[]);
    assert_eq!(zero.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&zero.stderr);
    assert!(
        !stderr.contains("spec §"),
        "零块警告不得引用内部 spec 章节: {stderr}"
    );
    let empty_log = log.with_extension("empty");
    std::fs::write(&empty_log, "").unwrap();
    let empty = run_skim(&rules, &empty_log, &[]);
    let stderr = String::from_utf8_lossy(&empty.stderr);
    assert!(
        !stderr.contains("spec §"),
        "错误消息不得引用内部 spec 章节: {stderr}"
    );
}

// #16-A: 人类可读块头携带代表 fragment 的 intent/category。
#[test]
fn block_header_carries_intent_category() {
    let (rules, log) = fixture("hdr16", &[RULE_FAILURE, RULE_WARNING], "ok\nERR a\n");
    let out = run_skim(&rules, &log, &[]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--- [L2-L2] [failure/test] t-hit"),
        "块头形态应为 `--- [L区间] [intent/category] rules`: {stdout}"
    );
}

// #16-A: 相邻合并块 (failure+warning 混合) 块头取最高信息优先级 intent (failure) 的代表 fragment。
#[test]
fn merged_block_header_takes_failure_representative() {
    let (rules, log) = fixture("merge16", &[RULE_FAILURE, RULE_WARNING], "ERR a\nWARN b\n");
    let out = run_skim(&rules, &log, &[]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--- [L1-L2] [failure/test] t-hit,t-warn"),
        "合并块代表 fragment 应为 failure 侧: {stdout}"
    );
}

// #16-B: l0 是内部 CI 工具, 隐藏于公共帮助面 (命令本身保留可用)。
// 逐行断言 Commands 区无 l0 条目 (整文子串匹配会对偶然出现的 "l0" 子串误红)。
#[test]
fn l0_hidden_from_public_help() {
    let help = Command::new(env!("CARGO_BIN_EXE_log-skimmer"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        !help_text.lines().any(|l| l.trim_start().starts_with("l0")),
        "l0 子命令不应出现在公共帮助 Commands 区: {help_text}"
    );
}

// #38: hygiene 是内部发布链工具 (publish-stage 机械防线), 与 l0 (#16-B) 同款隐藏于
// 公共帮助面; 但 hide ≠ 移除 — 子命令级帮助与解析路径保留 (justfile publish-stage
// 直接调用不受影响)。断言钉住两面: 公共面不列 hygiene, 直接调用仍走 hygiene 解析。
#[test]
fn hygiene_hidden_from_public_help() {
    let bin = env!("CARGO_BIN_EXE_log-skimmer");
    // 面 1: 公共 --help 的 Commands 区无 hygiene 条目 (逐行断言, 与 l0 用例同法)
    let help = Command::new(bin).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        !help_text
            .lines()
            .any(|l| l.trim_start().starts_with("hygiene")),
        "hygiene 子命令不应出现在公共帮助 Commands 区: {help_text}"
    );
    // 面 2: 只隐藏不移除 — 子命令级 --help 仍可用
    let sub_help = Command::new(bin)
        .args(["hygiene", "--help"])
        .output()
        .unwrap();
    assert!(
        sub_help.status.success(),
        "hygiene --help 应仍可用 (hide ≠ 移除)"
    );
    assert!(
        String::from_utf8_lossy(&sub_help.stdout).contains("--patterns"),
        "hygiene 子命令级帮助应含 --patterns 参数面"
    );
    // 面 3: 直接调用仍走 hygiene 的 clap 解析 — 缺必选参数报 usage (exit 64),
    // 而非 unrecognized subcommand (防 hide 语义被误改成移除)。
    let bare = Command::new(bin).arg("hygiene").output().unwrap();
    assert_eq!(bare.status.code(), Some(64));
    let stderr = String::from_utf8_lossy(&bare.stderr);
    assert!(
        !stderr.contains("unrecognized subcommand"),
        "hygiene 应仍被 clap 识别为子命令: {stderr}"
    );
    assert!(
        stderr.contains("required arguments were not provided"),
        "缺参错误应来自 hygiene 自身解析路径: {stderr}"
    );
}
