//! stdio 全链路冒烟: 起真实子进程, 走 MCP 握手 (initialize → initialized →
//! tools/list → tools/call), 断言 JSON-RPC 响应结构。
//! 职责边界: 只验证协议链路与响应形态, 语义正确性由 src 内 fixture 单测覆盖。
//!
//! 大文本验证 (CI 日志单条数百 KB 量级): `stdio_big_log_e2e` 默认忽略,
//! 以 env 门控触发 (语料在独立语料仓, 经 just corpus-fetch 拉取; 最大样本 niri 1.4MB):
//! `LOG_SKIMMER_E2E_LOG=<大日志路径> cargo test -p log-skimmer-server --test stdio_smoke -- --ignored`

use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const PROTOCOL_VERSION: &str = "2025-06-18";

fn spawn_server() -> std::process::Child {
    // CARGO_BIN_EXE_*: cargo 自动先构建 bin 并注入路径 (集成测试标准形态)
    // 规则集解耦 (issue #3): 显式 env 指向 server 测试最小 fixture — 子进程默认
    // 装载路径的编译期兜底指向 engine fixture 规则集 (镜像真规则仓, 会演进), 会把
    // 规则演进耦合进协议链路断言; env 是生产解析链最高优先级, 顺带覆盖该分支。
    // (路径表达式与 crate 内 fixture_rules_dir 同源, 集成测试在 crate 外不可见)
    let rules_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules");
    Command::new(env!("CARGO_BIN_EXE_log-skimmer-mcp"))
        .env("LOG_SKIMMER_RULES", &rules_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // 启动日志走 stderr, 不参与协议断言
        .spawn()
        .expect("spawn log-skimmer-mcp")
}

/// 发全部请求 → 关闭 stdin (EOF 触发 server 正常退出) → 读尽 stdout → join。
///
/// 安全前提 (无死锁): 全部请求先写完、后读响应, 要求「server 消费后续请求之前
/// 已可产出的响应总量 < pipe 容量 (~64KB)」。当前各用例均满足 (最大的 skim 响应
/// 在 server 读完整个大请求之后才产出); 新增用例若含「小请求产生大响应且之后
/// 还有请求」的排布, 须改为读写并发 (spawn 读线程)。
fn roundtrip(
    child: &mut std::process::Child,
    requests: &[String],
) -> (String, std::process::ExitStatus) {
    {
        let stdin = child.stdin.take().expect("stdin piped");
        let mut w = BufWriter::new(stdin);
        for r in requests {
            writeln!(w, "{r}").expect("写请求");
        }
        w.flush().expect("flush 请求");
    } // drop → stdin EOF
    let mut out = String::new();
    child
        .stdout
        .take()
        .expect("stdout piped")
        .read_to_string(&mut out)
        .expect("读尽 stdout");
    let status = child.wait().expect("join 子进程");
    (out, status)
}

fn parse_jsonl(out: &str) -> Vec<serde_json::Value> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("stdout 每行均为合法 JSON"))
        .collect()
}

fn req(id: u64, method: &str, params: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// MCP 握手请求对 (initialize + initialized), 三个用例共用; 协议版本单点。
fn handshake_reqs() -> Vec<String> {
    vec![
        req(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "smoke-test", "version": "0.0.1"}
            }),
        ),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
    ]
}

/// 按 id 取响应, 缺失即 panic (断言具失败能力)。
fn by_id(responses: &[serde_json::Value], id: i64) -> &serde_json::Value {
    responses
        .iter()
        .find(|v| v["id"].as_i64() == Some(id))
        .unwrap_or_else(|| panic!("缺 id={id} 响应"))
}

const CARGO_FAIL_LOG: &str = "   Compiling demo v0.1.0\nerror: could not compile `demo` (bin \"demo\") due to 1 previous error\n";

#[test]
fn stdio_smoke_handshake_list_and_skim() {
    let mut child = spawn_server();
    let requests = [
        handshake_reqs(),
        vec![
            req(2, "tools/list", serde_json::json!({})),
            req(
                3,
                "tools/call",
                serde_json::json!({"name": "skim", "arguments": {"log_text": CARGO_FAIL_LOG}}),
            ),
        ],
    ]
    .concat();
    let (out, status) = roundtrip(&mut child, &requests);
    assert!(status.success(), "EOF 正常退出, 实际 {status}");

    let responses = parse_jsonl(&out);
    assert_eq!(responses.len(), 3, "3 请求各 1 响应 (notification 无响应)");

    // initialize: serverInfo + tools capability
    let init = by_id(&responses, 1);
    assert!(init["result"]["serverInfo"]["name"].is_string());
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // tools/list: 3 个 tool, 名称精确
    let list = by_id(&responses, 2);
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .expect("tools 数组")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name"))
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, vec!["list_rules", "read_around", "skim"]);

    // tools/call skim: text content 内嵌 JSON, 命中 cargo 规则, 无 fallback
    let call = by_id(&responses, 3);
    assert!(call["result"]["isError"].is_null() || call["result"]["isError"] == false);
    let text = call["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    let payload: serde_json::Value = serde_json::from_str(text).expect("内嵌 JSON 可解析");
    let blocks = payload["blocks"].as_array().expect("blocks 数组");
    assert!(!blocks.is_empty());
    let rules0: Vec<&str> = blocks[0]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert!(rules0.contains(&"cargo-fixture-build-fail"));
    assert!(payload.get("fallback").is_none());
}

#[test]
fn stdio_smoke_read_around_and_list_rules() {
    let mut child = spawn_server();
    let log = "alpha\nbeta\ngamma\n";
    let requests = [
        handshake_reqs(),
        vec![
            req(
                2,
                "tools/call",
                serde_json::json!({"name": "read_around", "arguments": {"log_text": log, "line_start": 2, "line_end": 3}}),
            ),
            req(3, "tools/call", serde_json::json!({"name": "list_rules", "arguments": {}})),
            // 空输入: 协议层结构化错误 (非 crash — 后续仍可继续调用)
            req(
                4,
                "tools/call",
                serde_json::json!({"name": "skim", "arguments": {"log_text": ""}}),
            ),
        ],
    ]
    .concat();
    let (out, status) = roundtrip(&mut child, &requests);
    assert!(status.success());
    let responses = parse_jsonl(&out);
    assert_eq!(
        responses.len(),
        4,
        "4 请求各 1 响应, 空 input 不 crash 进程"
    );

    let ra = by_id(&responses, 2);
    let text = ra["result"]["content"][0]["text"].as_str().expect("text");
    let payload: serde_json::Value = serde_json::from_str(text).expect("JSON");
    let lines: Vec<&str> = payload["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(lines, vec!["L2: beta", "L3: gamma"]);

    let lr = by_id(&responses, 3);
    let text = lr["result"]["content"][0]["text"].as_str().expect("text");
    let arr: Vec<serde_json::Value> = serde_json::from_str(text).expect("规则清单 JSON");
    assert!(arr.len() >= 2, "fixture 规则集基线");

    // 空输入 → JSON-RPC error 响应 (invalid params), code/message 结构在场
    let err = by_id(&responses, 4);
    assert!(err.get("error").is_some(), "参数错误走 error 响应: {err}");
    assert!(err["error"]["message"]
        .as_str()
        .expect("错误信息")
        .contains("空或纯空白"));
}

/// 大日志全链路: 验证 rmcp stdio 传输对 CI 级大文本 (数百 KB~MB) 无截断/超时。
/// 语料在独立语料仓 (经 just corpus-fetch 拉到 data/logs), env 门控:
/// LOG_SKIMMER_E2E_LOG 指向本地大日志 (如语料库最大样本
/// data/logs/github.com/niri-wm/niri/<日期>/38707552842-116330622870-Dependabot-success.log,
/// ~1.36MB; 路径相对仓根, 而 cargo 测试进程 cwd 是 server/ — 建议传绝对路径)。
/// 2026-10-16 实测结论: 1.36MB 请求/响应全链路完整 (记录于 M2 报告)。
/// 断言面是传输完整性与响应结构, 不断言命中 — 真实语料行首带时间戳/ANSI 码,
/// 是否命中取决于装载规则集的锚容忍度 (规则集性质, 非 transport 义务, issue #3)。
#[test]
#[ignore = "需真实大语料: LOG_SKIMMER_E2E_LOG=<路径> cargo test -p log-skimmer-server --test stdio_smoke -- --ignored"]
fn stdio_big_log_e2e() {
    let Ok(path) = std::env::var("LOG_SKIMMER_E2E_LOG") else {
        panic!("门控未满足: 须设 LOG_SKIMMER_E2E_LOG 指向大日志文件");
    };
    let log_text = std::fs::read_to_string(&path).expect("读语料");
    assert!(
        log_text.len() > 100 * 1024,
        "语料应具规模 (>100KB), 当前 {} 字节 — 换更大的样本",
        log_text.len()
    );
    let mut child = spawn_server();
    let requests = [
        handshake_reqs(),
        vec![req(
            2,
            "tools/call",
            serde_json::json!({"name": "skim", "arguments": {"log_text": log_text}}),
        )],
    ]
    .concat();
    let (out, status) = roundtrip(&mut child, &requests);
    assert!(status.success());
    let responses = parse_jsonl(&out);
    let call = by_id(&responses, 2);
    assert!(
        call.get("error").is_none(),
        "大文本不产生协议错误: {}",
        call["error"]
    );
    let text = call["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    let payload: serde_json::Value =
        serde_json::from_str(text).expect("大文本响应完整可解析 (无截断)");
    let blocks = payload["blocks"].as_array().expect("blocks");
    // 结构不变量 (与命中无关, 恒可断言): 非零块 ⇔ 无 fallback 字段 — 大文本路径上
    // 钉住 skim 输出契约的互斥性 (零块侧顺带覆盖 fallback 形态)。
    assert_eq!(
        blocks.is_empty(),
        payload.get("fallback").is_some(),
        "blocks 非空 ⇔ 无 fallback (大文本响应结构自洽)"
    );
    eprintln!(
        "大文本 e2e: 请求 {} 字节, 响应 {} 块 / {} 签名行, total_lines {}",
        log_text.len(),
        blocks.len(),
        payload["stats"]["signature_lines"],
        payload["stats"]["total_lines"]
    );
}
