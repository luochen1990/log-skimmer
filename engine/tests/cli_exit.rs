//! CLI 退出码契约测试 — 子进程级端到端断言 (issue #15: 退出码 2 三重语义过载整改)。
//!
//! 契约 (README 退出码表, 调用方契约):
//! - 0  正常完成 (含 --help / --version 的 stdout 正常退出)
//! - 2  运行期数据错误: 规则集非法 / 日志读取失败
//! - 3  零块失明 (axioms 侧已覆盖, 此处不重复)
//! - 64 用法错误 (clap 解析期, BSD sysexits EX_USAGE) — 与运行期 2 分流,
//!   调用方看到 64 应检查命令拼写而非修数据
//!
//! l0 子命令的 0/1/2 判定码是内部 CI 契约 (l0_cmd 运行期发出), 与本文件的
//! 解析期用法错误正交, 不在此断言。

use std::process::Command;

/// 跑 CLI 二进制, 返回 (exit_code, stdout, stderr)。
fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_log-skimmer"))
        .args(args)
        .output()
        .expect("spawn log-skimmer 失败");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// 断言 args 构成解析期用法错误: exit 64 + stderr 有 clap error 行。
/// (无参调用的 help-on-stderr 形态不适用本 helper, 见其专属用例注释。)
fn assert_usage_error(args: &[&str]) {
    let (code, _, stderr) = run(args);
    assert_eq!(
        code, 64,
        "用法错误应 exit 64, args={args:?}, stderr:\n{stderr}"
    );
    assert!(stderr.contains("error:"), "stderr:\n{stderr}");
}

// ---- 用法错误 → 64 (EX_USAGE) ------------------------------------------------

#[test]
fn usage_error_invalid_context_exits_64() {
    // issue #15 MRE 的两种触发形态, 都是解析期用法错误:
    // 空格传参 → unexpected argument; 等号传参 → invalid value (u32 拒负)。
    assert_usage_error(&["skim", "x.log", "--rules", "R", "--context", "-1"]);
    assert_usage_error(&["skim", "x.log", "--rules", "R", "--context=-1"]);
}

#[test]
fn usage_error_unknown_flag_exits_64() {
    assert_usage_error(&["--bogus"]);
}

#[test]
fn usage_error_missing_required_rules_exits_64() {
    // --rules 是必选项, 缺失属用法错误而非数据错误
    assert_usage_error(&["skim", "x.log"]);
}

#[test]
fn usage_error_l0_missing_required_exits_64() {
    // l0 运行期判定码 0/1/2 是内部 CI 契约, 但其参数用法错误同属解析期 → 64
    assert_usage_error(&["l0"]);
}

/// 无参调用 pin: clap Kind = DisplayHelpOnMissingArgumentOrSubcommand,
/// help 全文走 stderr (无 `error:` 行) 且 use_stderr()==true → exit 64。
/// 注意不能照抄上面断言 `contains("error:")` 的习语 — 此处 stderr 是 help 文本。
/// 该 Kind 的流归属在 clap 4.x 系列内变过, 此用例防未来 clap bump 静默翻转。
#[test]
fn no_args_exits_64_with_help_on_stderr() {
    let (code, stdout, stderr) = run(&[]);
    assert_eq!(code, 64, "无参调用 (subcommand 必选) 是用法错误应 exit 64");
    assert!(stdout.is_empty(), "help 全文应走 stderr 而非 stdout");
    assert!(
        stderr.contains("Usage:"),
        "stderr 应含 help 文本:\n{stderr}"
    );
    assert!(
        !stderr.contains("error:"),
        "该 Kind 不产生 error 行:\n{stderr}"
    );
}

// ---- --help / --version 是正常输出, 不是错误 → 0 ------------------------------

#[test]
fn help_exits_zero_on_stdout() {
    let (code, stdout, stderr) = run(&["--help"]);
    assert_eq!(code, 0, "--help 应 exit 0, stderr:\n{stderr}");
    assert!(
        stdout.contains("Usage:"),
        "帮助应走 stdout, stdout:\n{stdout}"
    );
}

#[test]
fn version_exits_zero_on_stdout() {
    let (code, stdout, stderr) = run(&["--version"]);
    assert_eq!(code, 0, "--version 应 exit 0, stderr:\n{stderr}");
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "--version 应输出版本号 {} 到 stdout, got:\n{stdout}",
        env!("CARGO_PKG_VERSION")
    );
}

// ---- 正统运行期数据错误保持 2 (回归防倒退) -------------------------------------

#[test]
fn invalid_rules_dir_exits_2() {
    let (code, _, stderr) = run(&["skim", "x.log", "--rules", "/nonexistent-rules-dir"]);
    assert_eq!(
        code, 2,
        "规则集非法是运行期数据错误, 应保持 exit 2, stderr:\n{stderr}"
    );
    assert!(stderr.contains("规则目录不存在"), "stderr:\n{stderr}");
}

#[test]
fn unreadable_log_file_exits_2() {
    let rules = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rules");
    let (code, _, stderr) = run(&["skim", "/nonexistent-log-file.log", "--rules", rules]);
    assert_eq!(
        code, 2,
        "日志读取失败是运行期数据错误, 应保持 exit 2, stderr:\n{stderr}"
    );
    assert!(stderr.contains("读取日志失败"), "stderr:\n{stderr}");
}
