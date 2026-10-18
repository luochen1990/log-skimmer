//! MCP server 启动期装载诊断 e2e, 覆盖两条互补路径:
//! - 启动失败诊断 (issue #47): LOG_SKIMMER_RULES 三种坏值形态下, 真子进程
//!   fail-fast exit 2, stderr 文案可归因且**路径只出现一次**。
//! - 兜底降级警示 (issue #56): 三级解析第 3 级 fixture 兜底命中时, stderr
//!   须有醒目警示行 — 「静默成功」会让构建机上忘配 env 的部署以降级覆盖面
//!   (开发 fixture 远小于真规则集) 无感运行; 反之 env/相邻层级命中时警示
//!   不得出现 (警示语义绑定「兜底命中」, 不绑定装载成功)。
//!
//! 职责边界: 只覆盖启动装载路径 (失败在协议握手之前, 成功仅断言启动 stderr);
//! 协议链路由 stdio_smoke 覆盖, 消息文案 (路径去重契约) 的纯函数断言在 src/lib.rs 单测。
//!
//! 隔离性 (hermetic): exe 相邻探测以 current_exe 为基准, 而**原 bin 的位置不可控**
//! — CI runner 的 CARGO_TARGET_DIR 恰在 ../../rules 处有规则仓持久缓存 (issue #36;
//! 本测试组首个红即坐实该形态: 相邻探测合法装载 30 条真规则, 兜底未命中)。
//! 故兜底/相邻用例把 bin 拷贝到完全自持的临时沙箱 (`<tag>/x/bin/mcp` 布局使三级
//! 探测路径全部落在沙箱内) 再 spawn, 层级判定只取决于测试铺设的目录形态。

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ---------- 启动失败诊断 (issue #47) ----------

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

// ---------- 兜底降级警示 (issue #56) ----------

/// 最小握手请求 (initialize; 协议版本单点, 与 stdio_smoke 同族)。
const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"startup-diagnostics","version":"0.0.1"}}}"#;

/// 起指定 bin → 完成最小 MCP 握手 → EOF → 收集启动 stderr 后 join。
/// 先握手再 EOF: 无 initialize 的裸 EOF 被 rmcp 视为 ConnectionClosed 错误退出
/// (stdio_smoke 的握手请求同款); initialize 响应 (~1KB) 远小于 pipe 容量,
/// 无读写并发死锁风险。
fn startup_stderr(exe: &Path, env_rules: Option<&Path>) -> String {
    let mut cmd = Command::new(exe);
    match env_rules {
        Some(p) => {
            cmd.env("LOG_SKIMMER_RULES", p);
        }
        None => {
            cmd.env_remove("LOG_SKIMMER_RULES");
        }
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped()) // 协议通道占位 (响应无人读, 量级安全见上)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {}: {e}", exe.display()));
    {
        let stdin = child.stdin.take().expect("stdin piped");
        let mut w = BufWriter::new(stdin);
        writeln!(w, "{INITIALIZE}").expect("写 initialize");
        w.flush().expect("flush initialize");
    } // drop → stdin EOF
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr piped")
        .read_to_string(&mut stderr)
        .expect("读尽 stderr");
    assert!(
        child.wait().expect("join 子进程").success(),
        "握手后 EOF 须正常退出, stderr:\n{stderr}"
    );
    stderr
}

/// 拷贝真实 bin 到自持沙箱 `<tmp>/log-skimmer-startup-{tag}-{pid}/x/bin/mcp`, 返回
/// (bin 路径, 沙箱根)。三级探测路径 (bin 旁 rules / x/rules / 沙箱根 rules)
/// 全部落在沙箱内 — 命中与否完全由测试铺设决定, 与原 bin 位置/系统布局无关。
/// 隔离性双维度: 进程内各测试由 tag 区分; 跨进程由 pid 区分 (issue #66 —
/// 固定路径下并发测试进程的 fs::copy 写句柄与对方 exec 重叠 → ETXTBSY)。
fn sandbox(tag: &str) -> (PathBuf, PathBuf) {
    let root =
        std::env::temp_dir().join(format!("log-skimmer-startup-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("x/bin")).unwrap();
    let exe = root.join("x/bin/mcp");
    std::fs::copy(env!("CARGO_BIN_EXE_log-skimmer-mcp"), &exe).expect("拷贝 bin");
    (exe, root)
}

/// 第 3 级兜底命中 (无 env; 沙箱内三级探测路径全空; 编译期 fixture 路径恒在场)
/// → stderr 必须含警示行 (issue #56: 静默成功是配置缺失的无感降级)。
#[test]
fn fixture_fallback_emits_loud_warning() {
    let (exe, _root) = sandbox("fallback");
    let stderr = startup_stderr(&exe, None);
    assert!(
        stderr.contains(log_skimmer_server::FIXTURE_FALLBACK_WARNING),
        "第 3 级兜底命中须有警示行, stderr 实际:\n{stderr}"
    );
}

/// 第 1 级 env 显式指向合法规则集 (server fixture, 含 manifest) → 装载正常
/// 且警示不得出现 (警示语义绑定「兜底层级命中」, 不绑定装载成功)。
#[test]
fn env_ruleset_no_fallback_warning() {
    let rules_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules");
    let stderr = startup_stderr(
        Path::new(env!("CARGO_BIN_EXE_log-skimmer-mcp")),
        Some(&rules_dir),
    );
    assert!(
        !stderr.contains(log_skimmer_server::FIXTURE_FALLBACK_WARNING),
        "env 级命中不得有兜底警示, stderr 实际:\n{stderr}"
    );
}

/// 第 2 级相邻命中 (rules/ 铺在 bin 目录的上两级 = 沙箱根, 探测 `../../rules` 形态)
/// → 正常装载且警示不得出现 — 警示只针对「兜底」形态, 不针对「装载成功」。
/// 回归锚点: CI runner 的 CARGO_TARGET_DIR 旁恰有规则仓持久缓存即此形态。
#[test]
fn exe_adjacent_ruleset_no_fallback_warning() {
    let (exe, root) = sandbox("adjacent");
    let rules = root.join("rules"); // = exe 目录的 ../../rules
    std::fs::create_dir_all(&rules).unwrap();
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rules/cargo-fixture-build-fail.yaml"),
        rules.join("cargo-fixture-build-fail.yaml"),
    )
    .expect("铺设相邻 rules");
    let stderr = startup_stderr(&exe, None);
    assert!(
        !stderr.contains(log_skimmer_server::FIXTURE_FALLBACK_WARNING),
        "第 2 级相邻命中不得有兜底警示, stderr 实际:\n{stderr}"
    );
    // 探测路径不归一 (`x/bin/../../rules` 原样呈现), 用同构 join 构造期望串;
    // 「装载 1 条」(相邻集单条) 亦区分于兜底集 (engine fixture 4 条)。
    let probe_path = root.join("x/bin/../../rules");
    assert!(
        stderr.contains(probe_path.display().to_string().as_str())
            && stderr.contains("装载 1 条规则"),
        "须从相邻规则集装载, stderr 实际:\n{stderr}"
    );
}
