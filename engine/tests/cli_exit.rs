//! CLI 退出码契约测试 — 子进程级端到端断言 (issue #15: 退出码 2 三重语义过载整改)。
//!
//! 契约 (README 退出码表, 调用方契约):
//! - 0  正常完成 (含 --help / --version 的 stdout 正常退出)
//! - 2  运行期数据错误: 规则集非法 / 日志读取失败 / 日志为空 (0 行或纯空白, issues #71/#78)
//! - 3  零块失明 (axioms 侧已覆盖, 此处不重复)
//! - 64 用法错误 (clap 解析期, BSD sysexits EX_USAGE) — 与运行期 2 分流,
//!   调用方看到 64 应检查命令拼写而非修数据
//!
//! l0 子命令的 0/1/2 门禁码 (issue #44 分层): 1 = 判定层违例 (质量门禁红),
//! 2 = 采集层语料/规则非法 (数据错误, 与 skim 侧 exit 2 同族), 解析期用法错误同 64。

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

// #52b: `--context -1` 的空格形态须抵达值校验器 (invalid value ... not in 0..=10),
// 不得停在 clap 分词器的死路 tip ("to pass '-1' as a value, use '-- -1'" —
// 照做仍报错, 无效指引)。等号形态是既有回归基线。
#[test]
fn context_negative_space_form_reaches_value_validation() {
    for form in [
        vec!["skim", "x.log", "--rules", "R", "--context", "-1"],
        vec!["skim", "x.log", "--rules", "R", "--context=-1"],
    ] {
        let (code, _, stderr) = run(&form);
        assert_eq!(
            code, 64,
            "越界 context 是用法错误, args={form:?}, stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("invalid value '-1'"),
            "应抵达值校验器给出 invalid value: {stderr}"
        );
        assert!(
            stderr.contains("0..=10"),
            "越界报错应指明合法区间: {stderr}"
        );
        assert!(
            !stderr.contains("to pass"),
            "不得给出 '-- --context' 死路 tip: {stderr}"
        );
    }
}

// #52b 配套: 值缺失形态不因 allow_negative_numbers 而退化 — `--context` 后跟
// 已知 flag 时仍是「缺值」报错, 不把 flag 吞成值 (文案级断言, 防语义漂移)。
#[test]
fn context_missing_value_still_detected() {
    let (code, _, stderr) = run(&["skim", "x.log", "--rules", "R", "--context", "--json"]);
    assert_eq!(code, 64, "stderr:\n{stderr}");
    assert!(
        stderr.contains("a value is required"),
        "应报缺值而非吞 flag 为值: {stderr}"
    );
}

// #52a: 多日志文件是解析期用法错误 (LOG_FILE 只接受一个), exit 64 + clap
// unexpected argument 文案 (多文件说明走 --help 与 README, 此处钉行为)。
#[test]
fn multiple_log_files_is_usage_error() {
    let (code, _, stderr) = run(&["skim", "a.log", "b.log", "--rules", "R"]);
    assert_eq!(code, 64, "多文件应 exit 64, stderr:\n{stderr}");
    assert!(stderr.contains("unexpected argument"), "stderr:\n{stderr}");
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

// #47a CLI 面: --rules 误指已存在文件 → 如实报「不是目录」, 不误报「不存在」。
#[test]
fn rules_path_is_file_exits_2_truthfully() {
    let rules_file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rules/cargo/cargo-could-not-compile.yaml"
    );
    let (code, _, stderr) = run(&["skim", "x.log", "--rules", rules_file]);
    assert_eq!(
        code, 2,
        "规则路径非目录是运行期数据错误, 应保持 exit 2, stderr:\n{stderr}"
    );
    assert!(stderr.contains("不是目录"), "stderr:\n{stderr}");
    assert!(
        !stderr.contains("规则目录不存在"),
        "路径在场不得误报不存在: {stderr}"
    );
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

// ---- --rules 值文案: 空串 ≠ 缺值 (issue #73a) -----------------------------------

// #73a: `--rules ""` / `--rules=` (空串) ≠ 真缺值 — clap 原生 PathBuf parser 把
// 空串短路成空 invalid_value, 渲染层对空值统一渲染 "a value is required ... but
// none was supplied", 与真缺值逐字节同文案, 与「用户提供了空串」事实不符。
// 专属文案钉: 指明空串、不冒充缺值; 退出码仍 64 (空串仍是解析期用法错误)。
#[test]
fn empty_rules_value_gets_dedicated_copy() {
    assert_dedicated_empty_copy(&["skim", "x.log", "--rules", ""]); // 空格形态
    assert_dedicated_empty_copy(&["skim", "x.log", "--rules="]); // 等号形态
}

// 空串专属文案契约 (issue #73a): exit 64 + 指明空串 + 不冒充缺值文案 —
// skim --rules 原发与 l0/hygiene 推广入口共用断言面。
fn assert_dedicated_empty_copy(form: &[&str]) {
    let (code, _, stderr) = run(form);
    assert_eq!(
        code, 64,
        "空串是解析期用法错误, form={form:?}, stderr:\n{stderr}"
    );
    assert!(stderr.contains("不能为空串"), "空串须有专属文案: {stderr}");
    assert!(
        !stderr.contains("but none was supplied"),
        "空串不得冒充缺值文案: {stderr}"
    );
}

// #73a 推广回归钉: 空串专属文案泛化到 l0/hygiene 路径参数 —
// l0 --rules / l0 --corpus / hygiene --patterns 同样不得冒充缺值文案。
#[test]
fn empty_path_values_get_dedicated_copy_across_subcommands() {
    for form in [
        vec!["l0", "--corpus", "x", "--rules", ""],
        vec!["l0", "--corpus", "", "--rules", "x"],
        vec!["l0", "--corpus", "x", "--rules", "y", "--base", ""],
        vec!["hygiene", "--patterns", "", "dir"],
    ] {
        assert_dedicated_empty_copy(&form);
    }
}

// #73a 回归钉: 真缺值 (后面跟已知 flag) 的原有正确文案不因空串修复而漂移 —
// 空串拦截挂 value parser 层, 缺值发生在值抵达 parser 之前 (clap verify_num_args)。
#[test]
fn rules_missing_value_keeps_missing_copy() {
    let (code, _, stderr) = run(&["skim", "x.log", "--rules", "--json"]);
    assert_eq!(code, 64, "stderr:\n{stderr}");
    assert!(
        stderr.contains("a value is required"),
        "真缺值文案保持: {stderr}"
    );
}

// ---- l0 门禁码分层 (issue #44): 采集层语料非法 → 2 / 判定层违例 → 1 --------------

/// l0 跑一个小语料目录 (指定 tag 建临时目录), 返回 (exit_code, stdout, stderr)。
fn run_l0_on(tag: &str) -> (i32, String, String) {
    let corpus = std::env::temp_dir().join(format!("ls-cli-exit-l0-{tag}"));
    let _ = std::fs::remove_dir_all(&corpus);
    std::fs::create_dir_all(&corpus).unwrap();
    prepare(tag, &corpus);
    run(&[
        "l0",
        "--corpus",
        corpus.to_str().unwrap(),
        "--rules",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rules"),
    ])
}

/// 各用例的语料布景: empty = 空目录; orphan = 仅 b.log (无 sidecar);
/// small = 1 个配对样本 (failure, 命中 fixture gha 规则)。
fn prepare(tag: &str, corpus: &std::path::Path) {
    match tag {
        "empty" => {} // 不布景: 空目录
        "orphan" => {
            std::fs::write(corpus.join("b.log"), "##[error]orphan\n").unwrap();
        }
        "small" => {
            std::fs::write(corpus.join("a.log"), "##[error]boom\n").unwrap();
            std::fs::write(
                corpus.join("a.log.json"),
                r#"{"repo":"github.com/x/y","conclusion":"failure","attempt":1}"#,
            )
            .unwrap();
        }
        // P0-2: sidecar 缺失 attempt — 统计按 1 兜底, 缺口单行 stderr 警告可见
        "no-attempt" => {
            std::fs::write(corpus.join("a.log"), "##[error]boom\n").unwrap();
            std::fs::write(
                corpus.join("a.log.json"),
                r#"{"repo":"github.com/x/y","conclusion":"success"}"#,
            )
            .unwrap();
        }
        _ => unreachable!("未布景的 tag: {tag}"),
    }
}

#[test]
fn l0_missing_corpus_dir_exits_2() {
    // 语料目录不存在 = 采集层语料非法 (与 --rules /nonexistent 的规则侧对称),
    // 不是质量门禁红 — CI 按退出码分诊时「修环境」不得误判为「规则质量回归」。
    let (code, _, stderr) = run(&[
        "l0",
        "--corpus",
        "/nonexistent-corpus-dir-l0-cli",
        "--rules",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rules"),
    ]);
    assert_eq!(
        code, 2,
        "语料目录不存在应 exit 2 (语料非法), stderr:\n{stderr}"
    );
    assert!(stderr.contains("语料目录不存在"), "stderr:\n{stderr}");
}

#[test]
fn l0_zero_paired_samples_exits_2() {
    // 目录存在但零配对样本 = 采集层面失败 (空语料上拒绝判定/出 summary)。
    let (code, _, stderr) = run_l0_on("empty");
    assert_eq!(code, 2, "零配对样本应 exit 2, stderr:\n{stderr}");
    assert!(stderr.contains("零配对样本"), "stderr:\n{stderr}");
    // --emit-summary 路径同守卫: CI base 侧消费 summary, 空语料 summary =
    // 防空转要拦的伪基线 (否则双测对比对空 base 出伪结论)。
    let corpus = std::env::temp_dir().join("ls-cli-exit-l0-empty");
    let (code, stdout, _) = run(&[
        "l0",
        "--corpus",
        corpus.to_str().unwrap(),
        "--rules",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rules"),
        "--emit-summary",
    ]);
    assert_eq!(code, 2, "emit-summary 亦不得在空语料上出 summary");
    assert!(stdout.is_empty(), "不得输出伪基线 summary: {stdout}");
}

#[test]
fn l0_orphan_only_corpus_exits_2_with_warning() {
    // 孤儿-only = 零配对样本: exit 2 (issue #44), 且孤儿警告先行给出线索 (#46)。
    let (code, _, stderr) = run_l0_on("orphan");
    assert_eq!(
        code, 2,
        "孤儿-only 语料 = 零配对样本 → exit 2, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("b.log") && stderr.contains("缺 sidecar"),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains("零配对样本"), "stderr:\n{stderr}");
}

// P0-2: sidecar 缺失 attempt 字段 — 不中断判定 (exit 1 来自小语料下限, 与
// 缺口无关), 但缺口以聚合计数单行警告出现在 stderr (计数 + 按 attempt=1
// 处理说明), 不得静默。
#[test]
fn l0_missing_attempt_warns_on_stderr() {
    let (code, _, stderr) = run_l0_on("no-attempt");
    assert_eq!(
        code, 1,
        "缺失 attempt 不升级采集层错误 (统计按 1 兜底), stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("1 个 sidecar 缺失 attempt 字段"),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains("按 attempt=1 处理"), "stderr:\n{stderr}");
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.contains("缺失 attempt"))
            .count(),
        1,
        "单行聚合警告 (非逐样本): stderr:\n{stderr}"
    );
}

#[test]
fn l0_small_corpus_fails_exit_1_with_targeted_note() {
    // 0<n<50 合法小语料 (定向验证场景) 保持判定层 fail + exit 1 — note 双读法,
    // 不再单以「疑获取链路故障」指控定向验证的贡献者 (issue #44b)。
    let (code, stdout, stderr) = run_l0_on("small");
    assert_eq!(code, 1, "小语料 = 语料在场下限违例 → fail + exit 1");
    assert!(stderr.contains("verdict=fail"), "stderr:\n{stderr}");
    assert!(
        stdout.contains("定向验证"),
        "corpus-missing note 应分流读法: {stdout}"
    );
    assert!(
        stdout.contains("per_rule_hits"),
        "note 应指向定向验证正向信号: {stdout}"
    );
}

// #73b: l0 规则集非法 stderr 前缀不得叠成「l0 采集失败: 规则集非法: 规则集非法
// (1 处):」— collect 曾在 LoadErrors Display 自带前缀 ("规则集非法 (N 处):") 外
// 再包一层同名前缀; 三个变体 (目录不存在/空目录无 yaml/坏 yaml) 全走同一
// Display, 一并钉单层。
#[test]
fn l0_invalid_rules_stderr_single_prefix() {
    let tmp = std::env::temp_dir().join("ls-cli-exit-l0-rules-73b");
    let _ = std::fs::remove_dir_all(&tmp);
    let (empty, bad) = (tmp.join("empty"), tmp.join("bad"));
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("bad.yaml"), "id: x\nintent: bogus\n").unwrap();
    let corpus = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/l0-corpus");
    for rules in [
        "/nonexistent-rules-73b",
        empty.to_str().unwrap(),
        bad.to_str().unwrap(),
    ] {
        let (code, _, stderr) = run(&["l0", "--corpus", corpus, "--rules", rules]);
        assert_eq!(
            code, 2,
            "规则集非法是采集层数据错误, rules={rules}, stderr:\n{stderr}"
        );
        assert!(
            !stderr.contains("规则集非法: 规则集非法"),
            "前缀不得连续重复: {stderr}"
        );
        assert!(
            stderr.contains("规则集非法 ("),
            "单层前缀 (N 处形态) 仍须在场: {stderr}"
        );
    }
}
