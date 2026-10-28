//! Server 用户可见文案单点收敛 (issue #76: CLI/MCP 文案默认英文化 + i18n 结构预留)。
//!
//! 职责边界: MCP server 侧用户交互面文案的唯一来源 — tool 错误文案 / 零块
//! advice / 装载定位诊断 / 项目层叠加报错 / stderr 启动诊断 (含启动横幅与
//! 分形报错前缀) / 协议层框架文案。未来引入 zh 文案映射时只改本模块
//! (消费点零改动)。风格与 engine `messages` 模块一致: 英文、简洁、自解释,
//! 保留信息三要素。
//!
//! 刻意不收敛面 (与 CLI 参数定义同生共演, 未来随 rmcp/clap 属性化一并收敛):
//! - 4 个 `#[tool(description)]` 与 `with_instructions` (mcp.rs): 与 rmcp tool
//!   宏注册同生共演;
//! - SkimParams / ReadAroundParams 的 schemars doc comment (mcp.rs): 与参数
//!   结构定义同生共演, 渲染进 tools/list inputSchema;
//! - transport_gate 的 `SERVER_NOT_INITIALIZED_MSG` (英文常量): 与 -32002
//!   初始化闸门语义同生共演, e2e 锚定。
//!
//! 消费契约 (在消费点维护, 此处只列索引):
//! - `FIXTURE_FALLBACK_WARNING` / `ADVICE_NO_BLOCKS`: lib 侧 `pub use` 重导出,
//!   集成测试的既有路径 `log_skimmer_server::FIXTURE_FALLBACK_WARNING` 不变;
//! - `load_ruleset_at` 族: 消息本体不带路径 (#47 契约 — main 的
//!   「source <path>」前缀只让路径出现一次; engine LoadErrors 透传族例外)。

/// 零块 advice 文案 (issue #58): rg 回退指令 (判定矩阵不变) + adapter 创建
/// 引导 (派遣子任务调用 adapter_guide 拿 SOP, 项目层落盘后无需重启即时
/// 生效) + 通用规则贡献出口。
pub const ADVICE_NO_BLOCKS: &str =
    "zero blocks = blind; fall back to the rg baseline (e.g. rg -i 'error|fail|panic' \
     <log>). To improve: dispatch a subtask to call the adapter_guide tool for the SOP \
     and create a project-layer adapter for this repo (.log-skimmer/rules/, effective \
     immediately once written); rules of general value can be contributed to \
     https://github.com/luochen1990/log-skimmer-rules";

/// 第 3 级 fixture 兜底命中时的 stderr 警示行 (issue #56; 文案 SSOT, 测试锚定)。
pub const FIXTURE_FALLBACK_WARNING: &str =
    "⚠ tier-3 fixture fallback active — loaded the development fixture ruleset \
     (coverage far below the real ruleset); production deployments should set \
     LOG_SKIMMER_RULES or place the ruleset next to the binary";

// ---- 规则集装载定位 (lib.rs load_ruleset_at) ----

/// env 空串 (Path("") 的唯一来源) — 点名 env 并指向 README 三级解析文档。
pub fn rules_env_empty() -> &'static str {
    "LOG_SKIMMER_RULES is set to an empty string — set it to the ruleset directory \
     path, or unset it to use default detection (three-tier resolution order in README)"
}

/// 路径存在但非目录 (#47: 消息本体不带路径)。
pub fn rules_path_not_a_dir() -> &'static str {
    "path exists but is not a directory (a ruleset must be a directory)"
}

/// 规则目录不存在 (#47: 消息本体不带路径)。
pub fn rules_dir_not_found() -> &'static str {
    "rules directory not found"
}

/// 目录内无 *.yaml 规则。
pub fn ruleset_no_yaml() -> &'static str {
    "no *.yaml rules under the directory"
}

/// 超限错误文案 (skim/read_around 两 tool 的 Oversized 共用; 对外契约文案防双份分叉)。
pub fn oversized_log(n: usize, max: usize) -> String {
    format!(
        "log_text {n} bytes exceeds the limit of {max} bytes; preprocess it first \
         (e.g. tail -c) and submit in segments"
    )
}

// ---- 项目层叠加 (lib.rs overlay_merge / DynamicRuleset) ----

/// 同目录自撞 (全局层与项目层解析到同一目录): 指向 env 配置, 不把排障引向
/// 「改名规则」。
pub fn overlay_same_dir(dir: &std::path::Path) -> String {
    format!(
        "the global layer and the project layer resolve to the same directory {} — \
         one directory must not serve as both layer sources: if LOG_SKIMMER_RULES \
         points at the project layer, unset it or point it at the global ruleset \
         directory; if the global layer came from exe-adjacent detection, adjust the \
         layout (decouple the binary from the project layer) or set \
         LOG_SKIMMER_RULES explicitly",
        dir.display()
    )
}

/// 跨层 id 冲突: 并列冲突 id 与两侧来源 (冲突归因优先于 #47 单层路径去重偏好)。
pub fn overlay_id_conflict(
    ids: &str,
    global_dir: &std::path::Path,
    project_dir: &std::path::Path,
) -> String {
    format!(
        "cross-layer id conflict: {ids} — the global layer {} and the project layer {} \
         both define these ids; project-layer rule ids must not duplicate global ones \
         (rename or remove one)",
        global_dir.display(),
        project_dir.display()
    )
}

/// 运行期项目层重载失败前缀 (拒绝降级用过期规则集静默执行)。
pub fn project_layer_load_failed(dir: &std::path::Path, err: &str) -> String {
    format!(
        "project-layer ruleset load failed (source {}): {err}",
        dir.display()
    )
}

// ---- 启动 stderr (main.rs: 分形报错 / 启动横幅 / 项目层叠加行) ----

/// 装载失败 stderr 行 (main 分形报错三臂共用骨架)。source=None 为空串 env 形态
/// (报错文案已自点名 LOG_SKIMMER_RULES, 无「来源」可标); 其余形态带一次来源路径。
pub fn ruleset_load_failed(source: Option<&std::path::Path>, detail: &str) -> String {
    match source {
        None => format!("ruleset load failed: {detail}"),
        Some(p) => format!("ruleset load failed (source {}): {detail}", p.display()),
    }
}

/// 项目层装载失败 stderr 行 (目录由 OverlayError::Project 变体携带 — 不误挂
/// 全局层前缀)。
pub fn project_layer_load_failed_at_startup(dir: &std::path::Path, detail: &str) -> String {
    format!(
        "ruleset load failed (project layer, source {}): {detail}",
        dir.display()
    )
}

/// 启动横幅 (产品版本 + 全局层计数 + 来源; 单复数随计数)。
pub fn startup_banner(version: &str, count: usize, dir: &std::path::Path) -> String {
    let unit = if count == 1 { "rule" } else { "rules" };
    format!(
        "log-skimmer-mcp v{version}: loaded {count} {unit} (source {})",
        dir.display()
    )
}

/// 项目层叠加行 (在场才打印; 条数 + 来源)。
pub fn project_overlay_line(count: usize, dir: &std::path::Path) -> String {
    format!(
        "project-layer rules overlay: +{count} (source {})",
        dir.display()
    )
}

// ---- 协议层框架文案 (mcp.rs) ----

/// tool 结果序列化失败 (envelope 内部错误, 非用户输入错)。
pub fn result_serialize_failed(err: impl std::fmt::Display) -> String {
    format!("failed to serialize result: {err}")
}

/// rmcp 降级参数错误的重组框架 (issue #48: 三形态统一 -32602, serde 细节透传)。
pub fn invalid_parameter_format(detail: &str) -> String {
    format!("invalid parameter format: {detail}")
}

/// serve 层故障 (启动失败 / 服务终止)。
pub fn mcp_server_start_failed(err: &str) -> String {
    format!("MCP server failed to start: {err}")
}

pub fn mcp_server_terminated(err: &str) -> String {
    format!("MCP server terminated: {err}")
}

// ---- tool 输入校验错误 (lib.rs SkimInputError / ReadAroundError) ----

pub fn skim_log_empty() -> &'static str {
    "log_text is empty or whitespace-only — nothing to skim"
}

pub fn skim_invalid_context(n: i32) -> String {
    format!("context must be within 0-10, got {n}")
}

pub fn read_around_log_empty() -> &'static str {
    "log_text is empty or whitespace-only — no window to read"
}

pub fn read_around_bad_range(start: i64, end: i64) -> String {
    format!("invalid line range (must be 1 <= line_start <= line_end), got start={start} end={end}")
}

pub fn read_around_start_beyond_total(total: usize, start: i64) -> String {
    format!("line_start={start} is beyond the last line of the log ({total} lines total)")
}

/// read_around 窗口截断 note: 续读起点 = 截断尾行 + 1。
pub fn read_around_truncated_note(
    start: usize,
    requested_end: i64,
    limit: usize,
    resume_at: usize,
) -> String {
    format!(
        "requested window L{start}-L{requested_end} exceeds the {limit}-line limit; \
         truncated — call again with line_start=L{resume_at} to continue reading"
    )
}
