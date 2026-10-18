//! CLI 输出契约 e2e (issues #10/#13/#16/#17): 退出码族 / stdout-stderr 流分离 / 人类可读渲染形态。
//! 真进程形态 (CARGO_BIN_EXE): 断言对象是 main.rs 渲染层 (用户可见面), 纯函数层已由
//! axioms.rs 覆盖 — 此处只锁「调用方契约」, 不重复引擎语义断言。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// failure 规则 (intent 缺省默认 failure, category=test)。
const RULE_FAILURE: &str = "id: t-hit\ntitle: h\ntool: t\nscope: global\nkind: signal\ncategory: test\npatterns:\n  - regex: '^ERR'\n";
/// warning 规则 (相邻行可与 RULE_FAILURE 合并进同块, 验证代表 fragment 选择)。
const RULE_WARNING: &str = "id: t-warn\ntitle: w\ntool: t\nscope: global\nkind: signal\nintent: warning\ncategory: lint\npatterns:\n  - regex: '^WARN'\n";

/// 引擎仓 fixture 规则集 (l0 e2e 用 — 与 engine/tests/l0.rs 同源)。
fn fixtures_rules() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rules")
}

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

// #46: 孤儿样本 (单侧在场) 可见性 — 混在场时 stderr 逐条警告 (文件名 + 缺侧 + 已跳过),
// 不计数不升级 (是否硬错误留需求方裁决); 门禁不因配对缺口静默缩水。
#[test]
fn l0_orphan_sample_warned_and_skipped() {
    let dir = std::env::temp_dir().join("ls-cli-contract-l0-orphan");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 1 个配对样本 (failure, 命中 fixture gha 规则) + 1 个孤儿 (有 .log 无 sidecar)
    std::fs::write(dir.join("a.log"), "##[error]boom\n").unwrap();
    std::fs::write(
        dir.join("a.log.json"),
        r#"{"repo":"github.com/x/y","conclusion":"failure","attempt":1}"#,
    )
    .unwrap();
    std::fs::write(dir.join("b.log"), "##[error]orphan\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_log-skimmer"))
        .args(["l0", "--corpus", dir.to_str().unwrap(), "--rules", fixtures_rules()])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "1<50 → 判定层 fail (corpus-missing), 孤儿只警告不升级"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("b.log"), "孤儿文件名应出现在警告: {stderr}");
    assert!(
        stderr.contains("缺 sidecar") && stderr.contains("跳过"),
        "警告应含缺侧与跳过语义: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout 应为 verdict JSON ({e}): {stdout}"));
    assert_eq!(v["stats"]["samples"], 1, "孤儿不计数: {stdout}");
}

// #46: --corpus 说明声明配对语义 (样本 = *.log + 同名 .log.json, 单侧缺失警告跳过) —
// 帮助面与采集行为一致, 自组语料者的命名手误可自查。
#[test]
fn l0_help_documents_pairing_semantics() {
    let out = Command::new(env!("CARGO_BIN_EXE_log-skimmer"))
        .args(["l0", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("同名"),
        "l0 --help 的 --corpus 应声明配对语义: {stdout}"
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
