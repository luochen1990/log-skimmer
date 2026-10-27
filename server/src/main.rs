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
//! 项目层叠加 (spec §1.S): cwd 下 `.log-skimmer/rules/` 在场时叠加于全局层
//! (探测点 / 跨层 id 冲突语义的 SSOT 在 [log_skimmer_server::load_ruleset_overlay])。
//! 装载失败 (目录缺失 / 校验违例 / 零条规则 / 跨层 id 冲突) fail-fast: stderr
//! 中文报错 + exit 2。
//! 运行期 (issue #58): 启动装载的全局层移交 [log_skimmer_server::DynamicRuleset];
//! 项目层由其按目录签名在每次工具调用时动态感知 (会话中创建/修改/删除即时
//! 生效, 重载失败走工具调用错误通道 — 不崩 server), 详见其文档。
//! 第 3 级 fixture 兜底命中时额外打 stderr 醒目警示行 (issue #56 — 兜底是
//! 开发形态, 生产部署命中即配置缺失, 不做静默降级)。
//! stderr 是日志通道 (MCP 协议只占 stdout), 启动行不干扰协议。
//!
//! # 消费方判定矩阵 (引用, SSOT 见 docs/rule-format.md §3)
//! - skim 零块 → 返回 `fallback: "no-blocks"`: 规则集对该日志**结构失明**,
//!   与 job 成败无关, 消费方无条件回退 rg 基线 (advice 同时引导创建项目层
//!   adapter, issue #58);
//! - job 已知失败而 failure-intent 块为零 → 视同失明回退;
//! - warning / info 与成败判定无关。

fn main() {
    let (dir, level) = log_skimmer_server::resolve_rules();
    // 项目层探测 (spec §1.S): cwd 注入 — current_dir 获取失败由装载函数容错为
    // 无项目层 (cwd 不可知则探测无从谈起, 不阻塞启动)。
    let cwd = std::env::current_dir().ok();
    let rs = match log_skimmer_server::load_ruleset_overlay(&dir, cwd.as_deref()) {
        Ok(rs) => rs,
        // 分形报错 (归因到层, 各层文案契约见 OverlayError):
        Err(log_skimmer_server::OverlayError::Global(e)) => {
            // 来源前缀携带路径唯一一次 (Err 消息本体不带路径, #47 防双次出现);
            // 空串路径 (resolve_rules 对 env 空串的返回, 唯一来源) 无「来源」可标
            // — 报错文案已自点名 LOG_SKIMMER_RULES。
            if dir.as_os_str().is_empty() {
                eprintln!("规则集装载失败: {e}");
            } else {
                eprintln!("规则集装载失败 (来源 {}): {e}", dir.display());
            }
            std::process::exit(2);
        }
        Err(log_skimmer_server::OverlayError::Project(pdir, e)) => {
            // 项目层失败: 目录由错误变体携带, 不得误挂全局层来源前缀 — 错层归因
            // 会把排障引向错误方向。注意「路径只出现一次」在此**不适用**: engine
            // LoadErrors 按设计自带源路径 (issue #14B grep-ability 契约), 与 #47
            // 的预检文案族是两条文案轴 — 前缀与消息本体路径双现是该契约下的
            // 既定形态 (测试侧 startup_diagnostics 同口径)。
            eprintln!("规则集装载失败 (项目层 来源 {}): {e}", pdir.display());
            std::process::exit(2);
        }
        Err(log_skimmer_server::OverlayError::Conflict(e)) => {
            // 冲突文案自带两侧来源路径, 不再加前缀 (防路径双现)
            eprintln!("规则集装载失败: {e}");
            std::process::exit(2);
        }
    };
    // serverInfo (协议身份) 在 mcp::get_info; stderr 启动行带产品版本备诊断 (issue #49)
    eprintln!(
        "log-skimmer-mcp v{}: 装载 {} 条规则 (来源 {})",
        env!("CARGO_PKG_VERSION"),
        rs.global_count,
        dir.display()
    );
    // 项目层在场才打印 (spec §1.S; 不在场静默 — 可选增强不刷屏)
    if let Some(p) = &rs.project {
        eprintln!("项目层规则叠加: +{} 条 (来源 {})", p.count, p.dir.display());
    }
    if level == log_skimmer_server::RulesLevel::FixtureFallback {
        eprintln!("{}", log_skimmer_server::FIXTURE_FALLBACK_WARNING);
    }
    // 启动装载的项目层仅作 fail-fast 校验 (上方 exit 2) 与启动日志; 全局层移交
    // DynamicRuleset, 项目层由其运行期按签名重建 (首调用装载一次 — 语义恒由
    // current() 重建, 消除启动装载与签名采集间的竞态窗口, 见其文档)。
    let ruleset = log_skimmer_server::DynamicRuleset::new(rs.into_global(), dir, cwd);
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = runtime.block_on(log_skimmer_server::mcp::serve(ruleset)) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
