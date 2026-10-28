//! log-skimmer 引擎的 wasm 宿主薄壳 — wasm 交付形态 (浏览器/wasm runtime 侧
//! skim, README 计 CLI/MCP/wasm 三形态)。手写 C ABI, 零 wasm-bindgen 依赖: 接口面极小且稳定 (四导出, 主调用 wasm_skim_json
//! 一次入三段字节出一段字节), wasm-bindgen 的类型化绑定收益趋零, 反引入其 CLI
//! 版本配对约束 (选型对比见 spike-wasm 分支 SPIKE-REPORT.md; 切换信号: 需要 JS
//! 侧回调/多入口富接口/非标准 API 宿主)。
//!
//! ABI 契约 (宿主 = 任意标准 WebAssembly API 环境, 单线程假设):
//! - `wasm_alloc(len) -> ptr` — 宿主申请入参缓冲区并写入
//! - `wasm_free(ptr, cap)` — 归还 wasm_alloc/wasm_skim_json/wasm_engine_version
//!   的缓冲区 (cap 须与申请时一致)
//! - `wasm_skim_json(rules_ptr, rules_len, file_ptr, file_len, log_ptr, log_len) -> ptr`
//!   一次调用: 三段 UTF-8 入参 → 一段 JSON 出参
//! - `wasm_engine_version() -> ptr` — payload = engine crate 版本字符串 (无 JSON
//!   包装), 版本协商 (spec §1.M / bundle manifest `min_engine_version`) 的 wasm
//!   侧事实来源
//!
//! 入参:
//! - rules: JSON 数组 `[[源名, YAML 文本], ...]` — 规则目录收集语义镜像
//!   engine 目录遍历 (递归 *.yaml|*.yml 跳隐藏路径, 按路径排序), 含根级
//!   manifest.yaml (引擎侧保留名识别 + spec §1.M 版本协商)
//! - file: 样本名 (仅回显进 SkimResult.file, 不参与匹配)
//! - log: 原始日志全文 (BOM/切行/\r 归一由引擎 normalize_lines 单点处理)
//!
//! 返回: 自有缓冲区, 前 4 字节 LE u32 = payload 字节长, 其后为 payload:
//! `{"ok":true,"result":<SkimResult>}` (与 CLI --json 同一 serde 协议) 或
//! `{"ok":false,"error":"..."}` (装载失败/输入拒斥, 文案均对齐 CLI stderr —
//! engine input_reject_message 与 LoadErrors 的单一来源);
//! 宿主读毕以总长 (4 + payload_len) 调 wasm_free。
//! `ok:true` 且 blocks 为空 = 零块失明 (规则集对该日志盲) — CLI 的 exit 3 警示
//! 语义由宿主按 blocks.length 承接 (spec §3 零块 fallback 判定矩阵)。
//!
//! 宿主注意 (spike 踩坑, 参考实现 reconcile.mjs):
//! - wasm_alloc 须按 UTF-8 字节长 (JS string.length 是 UTF-16 单位数, 非 ASCII
//!   时小于字节长, 越界写破坏堆)。
//! - 三段入参先全部 alloc 再统一写入 (alloc 可能触发 memory 增长 detach 旧视图)。
//! - wasm_skim_json 若 trap (引擎 panic/OOM), 已分配的入参缓冲仍归宿主 —
//!   长驻宿主 (未来 web 仓) 的异常路径应照常 wasm_free。
//! - 导出名带 `wasm_` 前缀: 裸 `free` 会在任何 host 侧编译目标里 interpose
//!   libc free (双参被单参调用 = 堆损坏) — cargo test 的 host 单测 harness 即
//!   SIGSEGV 实证; 前缀使其在任何宿主环境可安全共存。cdylib-only 维持产物
//!   形态单一 (host 消费一律经 WebAssembly API 加载 .wasm, 对账走 reconcile.mjs)。

use log_skimmer::{input_reject_message, load_rules_from_sources, normalize_lines, skim};

#[no_mangle]
pub extern "C" fn wasm_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// 宿主经 exports.wasm_free 调用 (wasm32 无 libc, 前缀防 host 侧 interpose
/// libc 同名符号 — 见模块头注释)。
///
/// # Safety
///
/// 调用方须保证 (ptr, cap) 是本实例 wasm_alloc 或任一返回长度前缀缓冲区的
/// 导出 (wasm_skim_json / wasm_engine_version) 返回的配对值
/// — 裸指针 ABI 的调用契约; 传入其他来源的指针 = UB。
#[no_mangle]
pub unsafe extern "C" fn wasm_free(ptr: *mut u8, cap: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe { drop(Vec::from_raw_parts(ptr, 0, cap)) };
}

/// 长度前缀 (4B LE u32) + payload 写入自有缓冲区, 返回裸指针交宿主。
fn write_out(payload: &str) -> *mut u8 {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload.as_bytes());
    let ptr = out.as_mut_ptr();
    std::mem::forget(out);
    ptr
}

unsafe fn str_from(ptr: *const u8, len: usize) -> Result<String, String> {
    // null 守卫: from_raw_parts(null, 0) 即 UB — 第三方宿主传 (0, 0) 的集成 bug
    // 转为 envelope 错误而非 UB (与 wasm_free 的 null 早退对称)。
    if ptr.is_null() {
        return Err("input pointer is null".to_string());
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    String::from_utf8(slice.to_vec()).map_err(|e| format!("input is not valid UTF-8: {e}"))
}

/// # Safety
///
/// 调用方须保证三段 (ptr, len) 指向有效 UTF-8 缓冲 (宿主按 alloc 写入的配对值)
/// — 裸指针 ABI 的调用契约; 语义错误 (装载失败/输入拒斥) 经返回的 envelope
/// 报告, 不是 UB。
#[no_mangle]
pub unsafe extern "C" fn wasm_skim_json(
    rules_ptr: *const u8,
    rules_len: usize,
    file_ptr: *const u8,
    file_len: usize,
    log_ptr: *const u8,
    log_len: usize,
) -> *mut u8 {
    // envelope 序列化不可失败 (SkimResult 全 String/u32 字段); 失败视作构建问题。
    let payload = match unsafe { run(rules_ptr, rules_len, file_ptr, file_len, log_ptr, log_len) } {
        Ok(result) => format!(
            r#"{{"ok":true,"result":{}}}"#,
            serde_json::to_string(&result).expect("SkimResult 序列化不可失败")
        ),
        Err(e) => format!(
            r#"{{"ok":false,"error":{}}}"#,
            serde_json::to_string(&e).expect("String 序列化不可失败")
        ),
    };
    write_out(&payload)
}

unsafe fn run(
    rules_ptr: *const u8,
    rules_len: usize,
    file_ptr: *const u8,
    file_len: usize,
    log_ptr: *const u8,
    log_len: usize,
) -> Result<log_skimmer::SkimResult, String> {
    let rules_raw = str_from(rules_ptr, rules_len)?;
    let file = str_from(file_ptr, file_len)?;
    let log_raw = str_from(log_ptr, log_len)?;
    // ABI 的 rules 入参类型 = engine RuleSource (serde 数组形态, 见其 doc)。
    let sources: Vec<log_skimmer::RuleSource> =
        serde_json::from_str(&rules_raw).map_err(|e| format!("rules JSON parse failed: {e}"))?;
    let rules = load_rules_from_sources(&sources).map_err(|e| e.to_string())?;
    let lines = normalize_lines(&log_raw);
    // 拒斥文案 SSOT: engine input_reject_message (与 CLI stderr 共用, 不分叉)。
    if let Some(msg) = input_reject_message(&lines) {
        return Err(msg);
    }
    Ok(skim(&file, &rules, &lines))
}

/// 引擎版本 (长度前缀字符串, 无 JSON 包装 — 见模块头 ABI 契约)。SSOT =
/// engine crate 的 `VERSION` 常量 (其 Cargo.toml version); 本薄壳 crate 的
/// 版本与 engine 无同步机制, 不得用作引擎版本。
#[no_mangle]
pub extern "C" fn wasm_engine_version() -> *mut u8 {
    write_out(log_skimmer::VERSION)
}

/// 转发契约单测: 锁定 "薄壳导出 == engine crate 版本" (wasm-check 不进 CI,
/// 此断言是常驻防护)。`wasm_` 前缀使 host 目标链接安全 (见模块头注释)。
#[test]
fn engine_version_forwards_engine_crate_version() {
    let ptr = wasm_engine_version();
    // 4B LE 前缀 → payload 长度; 总 cap = 4 + len (与宿主 free 配对惯例一致)
    let prefix = unsafe { std::slice::from_raw_parts(ptr, 4) };
    let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
    let payload = unsafe { std::slice::from_raw_parts(ptr.add(4), len) };
    assert_eq!(std::str::from_utf8(payload).unwrap(), log_skimmer::VERSION);
    unsafe { wasm_free(ptr, 4 + len) };
}
