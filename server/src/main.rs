//! log-skimmer-mcp 入口 (bin): MCP server (stdio) 形态向 agent 暴露 skim 能力。
//!
//! # 职责边界
//! 仅两件事: 启动前规则集装载 (fail-fast) + 起 rmcp stdio server。
//! 协议层在 [log_skimmer_server::mcp], 业务视图在 [log_skimmer_server] lib 纯函数,
//! 状态机语义在 engine crate (`log_skimmer`) — 本文件零业务逻辑。
//!
//! # 装载语义
//! 定位优先级 (env > exe 相邻 > 编译期源码树) 与命中层级的 SSOT 在
//! [log_skimmer_server::resolve_rules] 文档, 此处不重复。
//! 装载失败 (目录缺失 / 校验违例 / 零条规则) fail-fast: stderr 中文报错 + exit 2。
//! 第 3 级 fixture 兜底命中时额外打 stderr 醒目警示行 (issue #56 — 兜底是
//! 开发形态, 生产部署命中即配置缺失, 不做静默降级)。
//! stderr 是日志通道 (MCP 协议只占 stdout), 启动行不干扰协议。
//!
//! # 消费方判定矩阵 (引用, SSOT 见 docs/rule-format.md §3)
//! - skim 零块 → 返回 `fallback: "no-blocks"`: 规则集对该日志**结构失明**,
//!   与 job 成败无关, 消费方无条件回退 rg 基线;
//! - job 已知失败而 failure-intent 块为零 → 视同失明回退;
//! - warning / info 与成败判定无关。

fn main() {
    let (dir, level) = log_skimmer_server::resolve_rules();
    let rules = match log_skimmer_server::load_ruleset_at(&dir) {
        Ok(r) => r,
        Err(e) => {
            // 来源前缀携带路径唯一一次 (Err 消息本体不带路径, #47 防双次出现);
            // 空串路径无「来源」可标 — 报错文案已自点名 LOG_SKIMMER_RULES。
            if dir.as_os_str().is_empty() {
                eprintln!("规则集装载失败: {e}");
            } else {
                eprintln!("规则集装载失败 (来源 {}): {e}", dir.display());
            }
            std::process::exit(2);
        }
    };
    // serverInfo (协议身份) 在 mcp::get_info; stderr 启动行带产品版本备诊断 (issue #49)
    eprintln!(
        "log-skimmer-mcp v{}: 装载 {} 条规则 (来源 {})",
        env!("CARGO_PKG_VERSION"),
        rules.len(),
        dir.display()
    );
    if level == log_skimmer_server::RulesLevel::FixtureFallback {
        eprintln!("{}", log_skimmer_server::FIXTURE_FALLBACK_WARNING);
    }
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = runtime.block_on(log_skimmer_server::mcp::serve(rules)) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
