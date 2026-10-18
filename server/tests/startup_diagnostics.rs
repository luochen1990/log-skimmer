//! MCP server 启动期装载诊断 e2e (issue #47): LOG_SKIMMER_RULES 三种坏值形态下,
//! 真子进程 fail-fast exit 2, stderr 文案可归因且**路径只出现一次**。
//! 职责边界: 只覆盖启动失败路径 (协议握手之前); 协议链路由 stdio_smoke 覆盖,
//! 本层消息文案 (路径去重契约) 的纯函数断言在 src/lib.rs 单测。

use std::process::Command;

/// 以指定 LOG_SKIMMER_RULES 起真实 server 子进程, 返回 (exit_code, stderr)。
/// 启动失败发生在 stdin 消费之前 — 无须握手, stdin 默认 null 即可。
fn run_with_env(rules: &str) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_log-skimmer-mcp"))
        .env("LOG_SKIMMER_RULES", rules)
        .output()
        .expect("spawn log-skimmer-mcp 失败");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// #47c: env 空串 → 明确点名 LOG_SKIMMER_RULES 为空串 (不空值裸奔), 指向 README
// 三级解析文档; 不输出空的「来源 ()」前缀。
#[test]
fn env_empty_string_names_variable_and_docs() {
    let (code, stderr) = run_with_env("");
    assert_eq!(code, 2, "装载失败 fail-fast exit 2, stderr:\n{stderr}");
    assert!(stderr.contains("LOG_SKIMMER_RULES 为空串"), "{stderr}");
    assert!(stderr.contains("README"), "应指向三级解析文档: {stderr}");
    assert!(
        !stderr.contains("(来源 )") && !stderr.contains("(来源:)"),
        "不得输出空来源前缀: {stderr}"
    );
}

// #47a+b: env 指向已存在文件 → 如实报「不是目录」(不误报不存在),
// 且路径在整条报错里只出现一次 (来源前缀一次, 消息本体不再重复)。
#[test]
fn env_file_path_reports_not_dir_once() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rules/generic-fixture-error-line.yaml"
    );
    let (code, stderr) = run_with_env(path);
    assert_eq!(code, 2, "stderr:\n{stderr}");
    assert!(stderr.contains("不是目录"), "{stderr}");
    assert!(
        !stderr.contains("规则目录不存在"),
        "路径在场不得误报不存在: {stderr}"
    );
    assert_eq!(
        stderr.matches("generic-fixture-error-line.yaml").count(),
        1,
        "路径只出现一次: {stderr}"
    );
}

// #47b 回归面: 不存在路径维持「规则目录不存在」文案, 路径同样只出现一次。
#[test]
fn env_missing_dir_reports_not_found_once() {
    let (code, stderr) = run_with_env("/nonexistent-rules-dir");
    assert_eq!(code, 2, "stderr:\n{stderr}");
    assert!(stderr.contains("规则目录不存在"), "{stderr}");
    assert_eq!(
        stderr.matches("/nonexistent-rules-dir").count(),
        1,
        "路径只出现一次: {stderr}"
    );
}
