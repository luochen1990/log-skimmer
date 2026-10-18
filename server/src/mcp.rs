//! MCP 协议层 (rmcp 3.x): tool 注册 / 参数结构约束 / 结果包装。
//!
//! 职责边界: 仅做协议转换 — 参数用宏结构体约束 (优于自由文档约定), 业务与
//! 校验全部委托 lib 纯函数 (可单测); 输入校验错误 (值域错 + 类型错/缺字段)
//! 统一转 MCP `invalid_params` JSON-RPC 错误 (结构化错误, 非 crash, issue #48),
//! 成功结果为 text content 内嵌 JSON。

use crate::{list_rules_view, read_around_view, skim_view};
use log_skimmer::Rule;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::{MaybeSendFuture, RequestContext};
use rmcp::{
    schemars, tool, tool_handler, tool_router, transport::stdio, ErrorData as McpError, RoleServer,
    ServerHandler, ServiceExt,
};
use serde::Deserialize;
use std::future::Future;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SkimParams {
    /// CI 日志全文 (多行原始文本; 空串/纯白与超过 32MB 均报参数错误)
    pub log_text: String,
    /// 全局覆盖 context 为 before=after=N, 取值 0-10; 缺省用各规则自带 context
    #[serde(default)]
    // 非 dead 标记: 向 inputSchema 发布 default: null 提示 (实测去掉会改变 tools/list 输出)
    // 同界单源: 上界引用 engine 常量 (issue #28), schema 数值断言见 tests 内锁界用例
    #[schemars(range(min = 0, max = log_skimmer::CONTEXT_MAX as i32))]
    pub context: Option<i32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadAroundParams {
    /// CI 日志全文 (与 skim 调用同一份日志, 行号才能对齐)
    pub log_text: String,
    /// 窗口起点行号 (1-based)
    #[schemars(range(min = 1))]
    pub line_start: i64,
    /// 窗口终点行号 (1-based, 含端点; 超出日志末尾自动收缩到末行)
    pub line_end: i64,
}

pub struct SkimServer {
    #[allow(dead_code)] // tool_router 字段由 #[tool_handler] 宏底层读取, 编译器误报
    tool_router: ToolRouter<SkimServer>,
    rules: Vec<Rule>,
}

/// 序列化输出为 text content 内嵌 pretty JSON (oss-cache 同款形态, 生产 dogfood 验证过)。
fn json_result(v: impl serde::Serialize) -> std::result::Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(&v)
        .map_err(|e| McpError::internal_error(format!("结果序列化失败: {e}"), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

#[tool_router]
impl SkimServer {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            tool_router: SkimServer::tool_router(),
            rules,
        }
    }

    #[tool(
        description = "skim CI 日志: 高信噪比提取失败证据/警示/环境快照。返回 blocks (块摘要: 坐标/规则/intent/category/输出行数/签名行数, 按 intent 优先级排序 failure>warning>info, 同级按行序) + fragments (每块代表片段全字段, 与 blocks 同序一一对应) + stats (双粒度压缩比, 零块时省略)。零块时返回 fallback=\"no-blocks\": 规则集对该日志失明, 请无条件回退 rg 基线 (如 rg -i 'error|fail|panic') 并考虑贡献新规则。消费方判定矩阵: job 失败而 failure-intent 块为零 = 视同失明回退 (docs/rule-format.md §3)。需要块周边原文时用 read_around 按块坐标取窗。 规则贡献: https://github.com/luochen1990/log-skimmer-rules"
    )]
    fn skim(
        &self,
        Parameters(p): Parameters<SkimParams>,
    ) -> std::result::Result<CallToolResult, McpError> {
        skim_view(&self.rules, &p.log_text, p.context)
            .map_err(|e| McpError::invalid_params(e.message(), None))
            .and_then(json_result)
    }

    #[tool(
        description = "按 1-based 行号窗口读取日志原文 (输出行带 L{n}: 前缀)。用于展开 skim 块坐标周边的上下文。窗口上限 200 行, 超出截断并在 note 给出续读起点; line_end 超出日志末尾自动收缩。"
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
        description = "列出已装载规则集清单 (id/tool/kind/intent/category/scope/repo, 按 id 升序), 用于判断规则集对目标日志的覆盖面。"
    )]
    fn list_rules(&self) -> std::result::Result<CallToolResult, McpError> {
        json_result(list_rules_view(&self.rules))
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
        Some(d) => Err(McpError::invalid_params(format!("参数格式非法: {d}"), None)),
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
                "log-skimmer: CI 日志高信噪比提取。tools: skim (结构化提取, 零块=fallback 失明警示), \
                     read_around (按块坐标取原文窗口), list_rules (规则覆盖面)。消费方判定矩阵 \
                     (docs/rule-format.md §3): skim 零块=失明, 无条件回退 rg; job 已知失败而 \
                     failure-intent 块为零=视同失明回退; warning/info 与成败判定无关。",
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

/// 运行 MCP server (stdio transport)。返回 Err 仅限 serve 层故障;
/// 规则装载 fail-fast 在 main 完成 (启动前)。
pub async fn serve(rules: Vec<Rule>) -> Result<(), String> {
    let service = SkimServer::new(rules)
        .serve(stdio())
        .await
        .map_err(|e| format!("MCP server 启动失败: {e:?}"))?;
    service
        .waiting()
        .await
        .map_err(|e| format!("MCP server 结束: {e:?}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// tool handler fixture 测试 (直接构造 SkimServer 调 handler, 不走子进程;
// stdio 全链路见 tests/stdio_smoke.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> SkimServer {
        SkimServer::new(crate::fixture_rules())
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
        assert!(v["advice"].as_str().unwrap().contains("rg"));
        // issue #37: 零块时 compression 字段省略 — 与 fallback 同屏不得自相矛盾
        // (compression: 1.0 意为 100% 压缩, 零命中 ≠ 高压缩)
        assert!(v["stats"].get("compression_block").is_none());
        assert!(v["stats"].get("compression_signature").is_none());
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
                err.message.contains("空或纯空白"),
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
        for key in ["id", "tool", "kind", "intent", "category", "scope", "repo"] {
            assert!(first.get(key).is_some(), "缺字段 {key}");
        }
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
