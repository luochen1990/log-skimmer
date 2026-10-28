//! MCP 协议层 (rmcp 3.x): tool 注册 / 参数结构约束 / 结果包装。
//!
//! 职责边界: 仅做协议转换 — 参数用宏结构体约束 (优于自由文档约定), 业务与
//! 校验全部委托 lib 纯函数 (可单测); 输入校验错误 (值域错 + 类型错/缺字段)
//! 统一转 MCP `invalid_params` JSON-RPC 错误 (结构化错误, 非 crash, issue #48),
//! 成功结果为 text content 内嵌 JSON。

use crate::{list_rules_view, read_around_view, skim_view, DynamicRuleset};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::{MaybeSendFuture, RequestContext};
use rmcp::{
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler,
    ServiceExt,
};
use serde::Deserialize;
use std::future::Future;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SkimParams {
    /// Full CI log text (multi-line raw text; empty/whitespace-only and over 32MB are both parameter errors)
    pub log_text: String,
    /// Override context globally to before=after=N, range 0-10; defaults to each rule's own context
    #[serde(default)]
    // 非 dead 标记: 向 inputSchema 发布 default: null 提示 (实测去掉会改变 tools/list 输出)
    // 同界单源: 上界引用 engine 常量 (issue #28), schema 数值断言见 tests 内锁界用例
    #[schemars(range(min = 0, max = log_skimmer::CONTEXT_MAX as i32))]
    pub context: Option<i32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadAroundParams {
    /// Full CI log text (the same log as the skim call, so line numbers align)
    pub log_text: String,
    /// Window start line (1-based)
    #[schemars(range(min = 1))]
    pub line_start: i64,
    /// Window end line (1-based, inclusive; shrinks to the last line if beyond it)
    pub line_end: i64,
}

/// Adapter 创建指南 SOP 文档 (issue #58): `adapter_guide` tool 的载荷。
/// 源文件 `server/adapter-guide.md`, 编译期嵌入 (部署零文件依赖), 文档与
/// 代码同仓同 PR 演进; 供 skim 零块 advice 引导而至的 agent 消费。
const ADAPTER_GUIDE: &str = include_str!("../adapter-guide.md");

pub struct SkimServer {
    #[allow(dead_code)] // tool_router 字段由 #[tool_handler] 宏底层读取, 编译器误报
    tool_router: ToolRouter<SkimServer>,
    /// 运行期规则集 (issue #58): 项目层按目录签名动态感知 — 会话中创建/修改
    /// `.log-skimmer/rules/` 后的下一次调用即时生效, 无需重启。
    rules: DynamicRuleset,
}

/// 序列化输出为 text content 内嵌 pretty JSON (oss-cache 同款形态, 生产 dogfood 验证过)。
fn json_result(v: impl serde::Serialize) -> std::result::Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(&v).map_err(|e| {
        McpError::internal_error(crate::messages::result_serialize_failed(&e), None)
    })?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// 项目层重载失败的错误映射 (issue #58): internal_error 通道 + 自明文案
/// (非参数错 — 需修复的是 `.log-skimmer/rules/` 数据而非调用参数, 调用方按
/// 文案修复后重试同一调用即可)。
fn project_layer_error(msg: String) -> McpError {
    McpError::internal_error(msg, None)
}

#[tool_router]
impl SkimServer {
    pub fn new(rules: DynamicRuleset) -> Self {
        Self {
            tool_router: SkimServer::tool_router(),
            rules,
        }
    }

    #[tool(
        description = "Skim a CI log: high-signal extraction of failure evidence / warnings / environment snapshots. Returns blocks (block summaries: coordinates / rules / intent / category / out-lines / signature-lines, ordered by intent priority failure>warning>info, ties by line order) + fragments (the representative fragment of each block with full fields, one-to-one with blocks in the same order) + stats (total/out/signature line counts always present; the two compression-ratio fields are omitted on zero blocks). On zero blocks returns fallback=\"no-blocks\": the ruleset is blind to this log — unconditionally fall back to the rg baseline (e.g. rg -i 'error|fail|panic'), and follow the advice to call adapter_guide to create a project-layer adapter for this repo. Consumer decision matrix: a failed job with zero failure-intent blocks = treat as blind and fall back (docs/rule-format.md §3). Use read_around with block coordinates to fetch surrounding raw lines. Rule contribution: https://github.com/luochen1990/log-skimmer-rules"
    )]
    fn skim(
        &self,
        Parameters(p): Parameters<SkimParams>,
    ) -> std::result::Result<CallToolResult, McpError> {
        // 取舍记录 (review #58): 规则集新鲜度 (current) 先于参数校验 — 坏项目层
        // 与坏参数同时在场时 internal_error 优先于 invalid_params 暴露。选择不把
        // 参数校验前置复制一份: 校验逻辑单点在 skim_view (DRY 优先), 且「环境错
        // 优先」对调用方反而更可操作 (修项目层是持久修复, 修参数是本次调用)。
        let rules = self.rules.current().map_err(project_layer_error)?;
        skim_view(&rules, &p.log_text, p.context)
            .map_err(|e| McpError::invalid_params(e.message(), None))
            .and_then(json_result)
    }

    #[tool(
        description = "Read raw log lines in a 1-based line-number window (output lines carry an L{n}: prefix). Used to expand the context around skim block coordinates. Window limit is 200 lines; beyond that it truncates and the note gives the resume point; line_end beyond the last line shrinks to it."
    )]
    fn read_around(
        &self,
        Parameters(p): Parameters<ReadAroundParams>,
    ) -> std::result::Result<CallToolResult, McpError> {
        read_around_view(&p.log_text, p.line_start, p.line_end)
            .map_err(|e| McpError::invalid_params(e.message(), None))
            .and_then(json_result)
    }

    #[tool(
        description = "List the loaded ruleset (id/tool/kind/intent/category, ascending by id), for judging the ruleset's coverage of the target log."
    )]
    fn list_rules(&self) -> std::result::Result<CallToolResult, McpError> {
        let rules = self.rules.current().map_err(project_layer_error)?;
        json_result(list_rules_view(&rules))
    }

    #[tool(
        description = "Returns the Adapter Creation Guide (a markdown document): the SOP for creating a project-layer ruleset (.log-skimmer/rules/) for the current repo — sample failing logs → write rules → write to disk → three acceptance checks (load / hit / semantics) → persist via git, plus the upstream contribution flow for OSS projects and a field quick reference. Follow this document when the skim zero-block advice (fallback=\"no-blocks\") tells you to create an adapter; once the project layer is on disk no server restart is needed — the next skim call picks it up immediately."
    )]
    fn adapter_guide(&self) -> std::result::Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            ADAPTER_GUIDE.to_string(),
        )]))
    }
}

/// rmcp 参数反序列化失败错误的 message 前缀 — rmcp 3.x 私有常量
/// `TOOL_ARGUMENT_DESERIALIZATION_ERROR_PREFIX` 的镜像 (handler/server/router/tool.rs):
/// rmcp 以「前缀匹配 + INVALID_PARAMS」识别反序列化错误并**降级**为
/// `result.isError=true` 正常响应 (英文文案), 与值域错的 JSON-RPC `-32602` 双通道
/// (issue #48)。本层用同一前缀识别降级响应并还原为错误通道。
/// 该前缀在 rmcp 侧由其自带测试锁定; 若上游变更, 本仓 e2e 断言
/// (tests/stdio_smoke.rs `stdio_param_error_channel_unified`) 升级时转红暴露。
const RMCP_ARG_DESER_ERROR_PREFIX: &str = "failed to deserialize parameters:";

/// 把 rmcp 降级的参数反序列化错误还原为 JSON-RPC `-32602` (issue #48):
/// 识别 `isError=true` 且首 text content 带上述前缀的响应, 重组为
/// 中文框架文案 + serde 细节透传; 其余响应原样放行 (本仓 tool 逻辑
/// 恒走 `Err` 或成功, 不产 isError=true, 故仅 rmcp 降级路径可命中)。
fn reraise_argument_error(resp: CallToolResponse) -> Result<CallToolResponse, McpError> {
    let detail = match &resp {
        CallToolResponse::Complete(r) if r.is_error == Some(true) => r
            .content
            .first()
            .and_then(|c| c.as_text())
            .and_then(|t| t.text.strip_prefix(RMCP_ARG_DESER_ERROR_PREFIX))
            .map(|d| d.trim().to_string()),
        _ => None,
    };
    match detail {
        Some(d) => Err(McpError::invalid_params(
            crate::messages::invalid_parameter_format(&d),
            None,
        )),
        None => Ok(resp),
    }
}

#[tool_handler]
impl ServerHandler for SkimServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            // issue #49: 协议身份 = 产品 (bin 名 + crate 版本)。rmcp 默认注入 SDK
            // 自身身份 ("rmcp"/"3.5.1" — env! 在 rmcp 源码内展开), 客户端无法
            // 对应本产品; rmcp 版本以 Cargo.lock 为权威, 不硬编码进文案。
            .with_server_info(Implementation::new(
                "log-skimmer-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "log-skimmer: high-signal CI log extraction. tools: skim (structured extraction; \
                      zero blocks = fallback blind warning, follow the advice to create a \
                      project-layer adapter), read_around (raw-line window by block \
                      coordinates), list_rules (ruleset coverage), adapter_guide (SOP for \
                      creating a project-layer adapter — once written to .log-skimmer/rules/ \
                      no restart is needed, the next call picks it up immediately). Consumer \
                      decision matrix (docs/rule-format.md §3): skim zero blocks = blind, \
                      unconditionally fall back to rg; a job known to have failed with zero \
                      failure-intent blocks = treat as blind and fall back; warning/info is \
                      irrelevant to pass/fail judgment.",
            )
    }

    /// issue #48: 手写 call_tool (宏对已定义方法让位, 逻辑 = 宏生成版 + 降级还原) —
    /// 参数错误三形态 (类型错/缺字段/值域错) 统一 JSON-RPC `-32602` 中文文案,
    /// 客户端可单一按 error code 判定, 无需同时检查 result.isError。
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let tcc = ToolCallContext::new(self, request, context);
        reraise_argument_error(self.tool_router.call(tcc).await?)
    }

    /// issue #50: rmcp trait 对 list_resources / list_resource_templates / list_prompts
    /// 默认返回**空成功**, 与 initialize 仅声明 tools capability 矛盾 (客户端误判可
    /// 消费); 覆写为 `-32601 method not found`, 与 trait 对 read_resource / get_prompt
    /// 等方法的默认对齐。capabilities 声明保持不变 (仍只声明 tools)。
    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Err(
            McpError::method_not_found::<ListResourcesRequestMethod>(),
        ))
    }

    /// 同 [Self::list_resources] (issue #50): resources 能力面下的模板清单方法。
    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + MaybeSendFuture + '_
    {
        std::future::ready(Err(McpError::method_not_found::<
            ListResourceTemplatesRequestMethod,
        >()))
    }

    /// 同 [Self::list_resources] (issue #50): prompts 能力面下的清单方法。
    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Err(McpError::method_not_found::<ListPromptsRequestMethod>()))
    }
}

/// 运行 MCP server (stdio transport, 读侧带初始化闸门 — issue #72b:
/// 未 initialize 先发请求回 `-32002` 而非 rmcp 默认的误导性 `-32602`,
/// 见 [transport_gate] 模块头)。返回 Err 仅限 serve 层故障;
/// 规则装载 fail-fast 在 main 完成 (启动前); 项目层运行期重载失败走
/// 工具调用错误通道 (不崩 server, issue #58)。
pub async fn serve(rules: DynamicRuleset) -> Result<(), String> {
    let service = SkimServer::new(rules)
        .serve(crate::transport_gate::gated_stdio())
        .await
        .map_err(|e| crate::messages::mcp_server_start_failed(&format!("{e:?}")))?;
    service
        .waiting()
        .await
        .map_err(|e| crate::messages::mcp_server_terminated(&format!("{e:?}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// tool handler fixture 测试 (直接构造 SkimServer 调 handler, 不走子进程;
// stdio 全链路见 tests/stdio_smoke.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixture_rules, fixture_rules_dir, overlay_root, DynamicRuleset};

    fn server() -> SkimServer {
        // cwd=None: 项目层探测关闭, 规则集恒为 fixture 全局层 — 行为与改造前
        // 的 Vec<Rule> 注入完全一致 (既有断言不受动态层影响)。
        SkimServer::new(DynamicRuleset::new(
            fixture_rules(),
            fixture_rules_dir(),
            None,
        ))
    }

    /// 动态规则集 server: 项目层探测基准指向沙箱 root (issue #58 用例)。
    fn dynamic_server(root: &std::path::Path) -> SkimServer {
        SkimServer::new(DynamicRuleset::new(
            fixture_rules(),
            fixture_rules_dir(),
            Some(root.to_path_buf()),
        ))
    }

    /// 从 CallToolResult 提取内嵌 JSON (text content 形态)。
    fn result_json(r: CallToolResult) -> serde_json::Value {
        let text = r
            .content
            .first()
            .and_then(|c| c.as_text().map(|t| t.text.clone()))
            .expect("text content");
        serde_json::from_str(&text).expect("内嵌 JSON 可解析")
    }

    const CARGO_FAIL_LOG: &str = "   Compiling demo v0.1.0\nerror: could not compile `demo` (bin \"demo\") due to 1 previous error\n";

    #[test]
    fn handler_skim_normal_log() {
        let s = server();
        let r = s
            .skim(Parameters(SkimParams {
                log_text: CARGO_FAIL_LOG.to_string(),
                context: None,
            }))
            .expect("正常日志成功");
        let v = result_json(r);
        let blocks = v["blocks"].as_array().expect("blocks 数组");
        assert!(!blocks.is_empty());
        let rules0: Vec<&str> = blocks[0]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert!(rules0.contains(&"cargo-fixture-build-fail"));
        assert_eq!(blocks[0]["intent"], "failure");
        assert_eq!(v["stats"]["total_lines"], 2);
        assert!(v.get("fallback").is_none(), "非零块无 fallback 字段");
        // issue #37 非零块侧: 压缩比照常在场 — 2 行入 1 行出, 双粒度均 0.5
        assert_eq!(v["stats"]["compression_block"].as_f64(), Some(0.5));
        assert_eq!(v["stats"]["compression_signature"].as_f64(), Some(0.5));
    }

    #[test]
    fn handler_skim_context_override() {
        let s = server();
        // context=2: 块前后各扩 2 行, 呈现行数应多于默认 (单行块默认无扩展)。
        // 对照组/实验组共用同一日志字面量 (结构保证同输入)。
        let log = format!("a\nb\n{CARGO_FAIL_LOG}c\nd\n");
        let base = s
            .skim(Parameters(SkimParams {
                log_text: log.clone(),
                context: None,
            }))
            .unwrap();
        let over = s
            .skim(Parameters(SkimParams {
                log_text: log,
                context: Some(2),
            }))
            .unwrap();
        let base_out = result_json(base)["stats"]["out_lines"].as_u64().unwrap();
        let over_out = result_json(over)["stats"]["out_lines"].as_u64().unwrap();
        assert!(over_out > base_out, "context 覆盖扩大输出窗口");
    }

    #[test]
    fn handler_skim_zero_blocks() {
        let s = server();
        let r = s
            .skim(Parameters(SkimParams {
                log_text: "nothing to see\njust plain lines\n".to_string(),
                context: None,
            }))
            .expect("零块不是错误");
        let v = result_json(r);
        assert_eq!(v["blocks"].as_array().unwrap().len(), 0);
        assert_eq!(v["fallback"], "no-blocks");
        // issue #58: advice 引导 adapter 创建路径 (tool 名 + 项目层位置)
        let advice = v["advice"].as_str().expect("advice 在场");
        assert!(advice.contains("rg"), "回退指令保留: {advice}");
        assert!(advice.contains("adapter_guide"), "advice: {advice}");
        assert!(advice.contains(".log-skimmer/rules"), "advice: {advice}");
        // issue #37: 零块时 compression 字段省略 — 与 fallback 同屏不得自相矛盾
        // (compression: 1.0 意为 100% 压缩, 零命中 ≠ 高压缩)
        assert!(v["stats"].get("compression_block").is_none());
        assert!(v["stats"].get("compression_signature").is_none());
    }

    /// adapter_guide tool (issue #58): 返回 SOP markdown, 关键节齐备 —
    /// 概念/五步 SOP/三项质检/上游贡献/速查 (消费方据此盲执行)。
    #[test]
    fn handler_adapter_guide_returns_sop() {
        let s = server();
        let r = s.adapter_guide().expect("adapter_guide 恒成功");
        let text = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .expect("text content");
        for marker in [
            "Adapter Creation Guide",
            ".log-skimmer/rules",
            "SOP (five steps)",
            "Acceptance check",
            "Contributing upstream",
            "field quick reference",
            "log-skimmer-rules",
        ] {
            assert!(text.contains(marker), "SOP 缺关键节 [{marker}]: {text}");
        }
        assert!(
            text.contains("attribution"),
            "贡献节须含 attribution 必填指引"
        );
    }

    /// 会话内 adapter 闭环 (issue #58 验收路径的 handler 层缩影):
    /// 零块 advice 引导 → 会话中创建项目层 → 同一 server 实例再 skim 即时生效。
    #[test]
    fn handler_skim_session_adapter_closed_loop() {
        let root = overlay_root("mcp-loop");
        let s = dynamic_server(&root);
        // 自定义格式日志: fixture 全局层不命中 → 零块 + advice 引导
        let log = "[projci] step 1 ok\n[projci] error: widget exploded\n";
        let v = result_json(
            s.skim(Parameters(SkimParams {
                log_text: log.to_string(),
                context: None,
            }))
            .unwrap(),
        );
        assert_eq!(v["fallback"], "no-blocks");
        assert!(v["advice"].as_str().unwrap().contains("adapter_guide"));
        // 会话中创建项目层 adapter (锚 ^\[projci\] error) — 无需重建 server
        let dir = root.join(".log-skimmer/rules");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("projci.yaml"),
            "id: projci-fail\ntitle: projci 失败\ntool: projci\nkind: signal\nintent: failure\npatterns:\n  - regex: '^\\[projci\\] error:'\n",
        )
        .unwrap();
        let v2 = result_json(
            s.skim(Parameters(SkimParams {
                log_text: log.to_string(),
                context: None,
            }))
            .expect("项目层装载成功"),
        );
        let blocks = v2["blocks"].as_array().expect("命中块");
        assert!(!blocks.is_empty(), "项目层规则即时生效: {v2}");
        assert_eq!(blocks[0]["rules"][0], "projci-fail");
        assert_eq!(blocks[0]["intent"], "failure");
        assert!(v2.get("fallback").is_none(), "fallback 伴随消失");
    }

    /// 项目层坏 yaml: skim 返回错误 (拒绝降级用旧规则集静默执行), 文案归因
    /// 项目层并携带修复线索 (issue #58 运行期失败语义)。
    #[test]
    fn handler_skim_broken_project_layer_rejects() {
        let root = overlay_root("mcp-broken");
        let s = dynamic_server(&root);
        let dir = root.join(".log-skimmer/rules");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.yaml"), "id: [broken").unwrap();
        let err = s
            .skim(Parameters(SkimParams {
                log_text: "some log\n".to_string(),
                context: None,
            }))
            .expect_err("坏项目层须报错");
        assert!(
            err.message.contains("project-layer ruleset load failed"),
            "报错信息: {}",
            err.message
        );
        // 校验顺序契约 (review #58): current() 先于参数校验 — 坏项目层 × 坏参数
        // 同时在场时, 环境错 (internal_error, 项目层文案) 优先暴露
        let err2 = s
            .skim(Parameters(SkimParams {
                log_text: String::new(), // 空输入 = 参数错 (正常应 invalid_params)
                context: None,
            }))
            .expect_err("坏项目层优先于参数错");
        assert!(
            err2.message.contains("project-layer ruleset load failed"),
            "环境错优先: {}",
            err2.message
        );
        // list_rules 同语义 (同一规则集消费面)
        assert!(s.list_rules().is_err());
    }

    #[test]
    fn handler_skim_empty_input_is_invalid_params() {
        let s = server();
        for empty in ["", " \n\t\n"] {
            let err = s
                .skim(Parameters(SkimParams {
                    log_text: empty.to_string(),
                    context: None,
                }))
                .expect_err("空输入须报错");
            assert!(
                err.message.contains("empty or whitespace-only"),
                "报错信息: {}",
                err.message
            );
        }
    }

    #[test]
    fn handler_read_around_normal_and_bounds() {
        let s = server();
        let log = "one\ntwo\nthree\n";
        let r = s
            .read_around(Parameters(ReadAroundParams {
                log_text: log.to_string(),
                line_start: 2,
                line_end: 3,
            }))
            .unwrap();
        let v = result_json(r);
        let lines: Vec<&str> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(lines, vec!["L2: two", "L3: three"]);

        // line_end 超末尾: 收缩非错误
        let r = s
            .read_around(Parameters(ReadAroundParams {
                log_text: log.to_string(),
                line_start: 2,
                line_end: 99,
            }))
            .unwrap();
        assert_eq!(result_json(r)["window_end"], 3);

        // 越界: start<1 / start>end / start>total 均参数错误
        for (a, b) in [(0, 2), (3, 2), (4, 9)] {
            let err = s
                .read_around(Parameters(ReadAroundParams {
                    log_text: log.to_string(),
                    line_start: a,
                    line_end: b,
                }))
                .expect_err("越界行号须报错");
            assert!(!err.message.is_empty());
        }
    }

    #[test]
    fn handler_list_rules_reports_ruleset() {
        let s = server();
        let r = s.list_rules().expect("list_rules 恒成功");
        let v = result_json(r);
        let arr = v.as_array().expect("规则清单数组");
        assert!(arr.len() >= 2, "fixture 规则集基线");
        let first = &arr[0];
        for key in ["id", "tool", "kind", "intent", "category"] {
            assert!(first.get(key).is_some(), "缺字段 {key}");
        }
        // v0.6: scope/repo 输出字段随规则字段退役 (spec §1.S), 不得再出现在清单
        assert!(first.get("scope").is_none(), "scope 已退役: {first}");
        assert!(first.get("repo").is_none(), "repo 已退役: {first}");
    }

    /// issue #28 锁界: SkimParams 生成的 inputSchema 中 context 数值约束
    /// 与 engine CONTEXT_MAX 同界 — 属性宏展开即使退化为字面量 (被人为改回),
    /// 此处以生成的 schema 数值断言兜底锁死。
    #[test]
    fn skim_params_schema_context_bound_matches_engine_const() {
        let schema = serde_json::to_value(schemars::schema_for!(SkimParams)).unwrap();
        let ctx = &schema["properties"]["context"];
        assert_eq!(
            ctx.get("minimum").and_then(serde_json::Value::as_i64),
            Some(0),
            "schema minimum 须为 0: {ctx}"
        );
        assert_eq!(
            ctx.get("maximum").and_then(serde_json::Value::as_u64),
            Some(log_skimmer::CONTEXT_MAX as u64),
            "schema maximum 须与 CONTEXT_MAX 同界: {ctx}"
        );
    }
}
