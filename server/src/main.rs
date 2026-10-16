//! log-skimmer-mcp 入口 (bin): MCP server (stdio) 形态向 agent 暴露 skim 能力。
//!
//! # 职责边界
//! 仅两件事: 启动前规则集装载 (fail-fast) + 起 rmcp stdio server。
//! 协议层在 [log_skimmer_server::mcp], 业务视图在 [log_skimmer_server] lib 纯函数,
//! 状态机语义在 engine crate (`log_skimmer`) — 本文件零业务逻辑。
//!
//! # 装载语义
//! 定位优先级 (env > exe 相邻 > 编译期源码树) 的 SSOT 在
//! [log_skimmer_server::resolve_rules_dir] 文档, 此处不重复。
//! 装载失败 (目录缺失 / 校验违例 / 零条规则) fail-fast: stderr 中文报错 + exit 2。
//! stderr 是日志通道 (MCP 协议只占 stdout), 启动行不干扰协议。
//!
//! # 消费方判定矩阵 (引用, SSOT 见 docs/rule-format.md §3)
//! - skim 零块 → 返回 `fallback: "no-blocks"`: 规则集对该日志**结构失明**,
//!   与 job 成败无关, 消费方无条件回退 rg 基线;
//! - job 已知失败而 failure-intent 块为零 → 视同失明回退;
//! - warning / info 与成败判定无关。

fn main() {
    let dir = log_skimmer_server::resolve_rules_dir();
    let rules = match log_skimmer_server::load_ruleset_at(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("规则集装载失败 (来源 {}): {e}", dir.display());
            std::process::exit(2);
        }
    };
    eprintln!(
        "log-skimmer-mcp: 装载 {} 条规则 (来源 {})",
        rules.len(),
        dir.display()
    );
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = runtime.block_on(log_skimmer_server::mcp::serve(rules)) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
