//! MCP stdio transport 初始化闸门 (issue #72b): 未 initialize 先发请求 → `-32002`。
//!
//! 职责边界: 纯协议生命周期守卫 (装饰 rmcp transport, 不触碰行解析/提取逻辑)。
//! rmcp 3.5.1 的 pre-init 循环 (`service/server.rs` serve_initialize) 对无 draft
//! `_meta` 的请求自行回 `-32602` 且文案误导 ("request _meta is missing or has
//! malformed required fields" — 实际问题是时序而非参数), 且发生在 ServerHandler
//! 分发之前, handler 层无从拦截。本模块是唯一拦截点: `Transport` trait 公开 +
//! `serve()` blanket 接受任意实现, 在消息**已解析后、上交 rmcp 前**过滤。
//!
//! 拦截语义 (与 rmcp pre-init 判定同构, 只替换 rmcp 会拒绝的那一支):
//! - ping 透传 (MCP spec 2025-11-25 lifecycle: initialize 前仅 ping 被允许,
//!   rmcp serve_initialize 会自答 pong);
//! - 带 2026-07-28 draft 完整内联 `_meta` 的请求透传 (draft discover 生命周期
//!   合法跳过 initialize, rmcp 后续自行做版本校验/分发);
//! - initialize 请求透传并落闸 (此后全透传, serve_inner 接管);
//! - 其余请求 (经典生命周期客户端未握手先发) → 回 `-32002` "Server not
//!   initialized" 并吞掉, 会话存活。
//!
//! 错误码依据: MCP spec 未规定 pre-init 请求的错误码; `-32002` 是官方 TS SDK
//! 家族约定 (ServerNotInitialized)。rmcp 3.5.1 无此常量 (其 -32002 为
//! RESOURCE_NOT_FOUND), 经 `ErrorCode(pub i32)` 自行构造, 文案消歧。

use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, ErrorCode, ErrorData, GetMeta, ProtocolVersion, RequestId,
    ServerJsonRpcMessage,
};
use rmcp::service::{RoleServer, TxJsonRpcMessage};
use rmcp::transport::{async_rw::AsyncRwTransport, Transport};
use std::future::Future;

/// JSON-RPC server-defined 区间内的 ServerNotInitialized (见模块头注释)。
const SERVER_NOT_INITIALIZED: ErrorCode = ErrorCode(-32002);

/// 对外错误文案 (e2e 测试锚定 "not initialized" 子串)。
const SERVER_NOT_INITIALIZED_MSG: &str = "Server not initialized: send initialize first";

/// pre-init 请求是否携带 2026-07-28 draft 完整内联 `_meta`
/// (protocolVersion + clientCapabilities) — 与 rmcp serve_initialize 的
/// `missing_required_keys` 判定同源同参, 保证两道闸门对同一条消息的分流一致。
fn has_complete_draft_meta(req: &ClientRequest) -> bool {
    req.get_meta()
        .missing_required_keys(&ProtocolVersion::V_2026_07_28)
        .is_empty()
}

/// 单条入站消息在 pre-init 阶段的处置判定 (纯函数, 表驱动单测直接覆盖)。
#[derive(Debug)]
enum Verdict {
    /// 透传 (notification/response/ping/带完整 draft `_meta` 的请求)。
    Pass,
    /// 透传并落闸 (initialize 请求)。
    PassAndOpen,
    /// 拦截: 回 `-32002` 并吞掉, 携带原请求 id。
    Reject(RequestId),
}

fn verdict(msg: &ClientJsonRpcMessage) -> Verdict {
    match msg {
        ClientJsonRpcMessage::Request(r) => match &r.request {
            ClientRequest::InitializeRequest(_) => Verdict::PassAndOpen,
            ClientRequest::PingRequest(_) => Verdict::Pass,
            req if has_complete_draft_meta(req) => Verdict::Pass,
            _ => Verdict::Reject(r.id.clone()),
        },
        // notification/response 预初始化原样透传 (rmcp 现行为不变)
        _ => Verdict::Pass,
    }
}

pub struct InitGateTransport<T> {
    inner: T,
    initialized: bool,
}

impl<T> InitGateTransport<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            initialized: false,
        }
    }
}

/// stdio 形态的门控 transport (mcp::serve 消费): 与 `rmcp::transport::stdio()`
/// 同一底层 (tokio stdin/stdout → AsyncRwTransport), 仅在读侧加初始化闸门。
pub fn gated_stdio(
) -> InitGateTransport<AsyncRwTransport<RoleServer, tokio::io::Stdin, tokio::io::Stdout>> {
    InitGateTransport::new(AsyncRwTransport::new_server(
        tokio::io::stdin(),
        tokio::io::stdout(),
    ))
}

impl<T: Transport<RoleServer>> Transport<RoleServer> for InitGateTransport<T> {
    type Error = T::Error;

    /// 取消安全性: 拦截只发生在 pre-init 阶段, 该阶段 rmcp serve_initialize 为
    /// 纯顺序 await (无 select!), 正常运行不存在取消点; 唯一的 select 在
    /// serve_server_with_ct 外层 (整条 serve vs ct.cancelled), 触发即会话终结,
    /// 丢一条消息无影响。落闸标记在 receive 解析后、任何再 await 之前完成。
    ///
    /// 落闸点 = 收到 initialize **请求** (而非握手成功): 版本协商失败时 rmcp
    /// 仍留在 pre-init 等重试, 此窗口内误发请求会回落 rmcp 默认 -32602 —
    /// 但该客户端已发过 initialize, "not initialized" 文案本也不再准确。
    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        loop {
            let msg = self.inner.receive().await?;
            if self.initialized {
                return Some(msg);
            }
            match verdict(&msg) {
                Verdict::PassAndOpen => {
                    self.initialized = true;
                    return Some(msg);
                }
                Verdict::Pass => return Some(msg),
                Verdict::Reject(id) => {
                    let error = ServerJsonRpcMessage::error(
                        ErrorData::new(SERVER_NOT_INITIALIZED, SERVER_NOT_INITIALIZED_MSG, None),
                        Some(id),
                    );
                    if self.inner.send(error).await.is_err() {
                        return None;
                    }
                    // 吞掉该请求继续读 — 会话存活
                }
            }
        }
    }

    /// 委托 send: 直接返回 inner 的 future (trait 要求 `+ 'static` 以支持并发发送,
    /// inner 的 future 由 T: Transport 保证 'static; 不能用 `async fn` 捕获 &mut self)。
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.inner.close().await
    }
}

// ---------------------------------------------------------------------------
// 单测 (内存 fake transport, 不起子进程; stdio 全链路见 tests/stdio_smoke.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// 内存 transport: 预载收件箱 + 记录发件 (错误响应断言面)。
    /// sent 经 Arc<Mutex>: Transport::send 契约要求 future 'static (不借 self),
    /// 记录面必须克隆进 future。
    struct FakeTransport {
        inbox: VecDeque<ClientJsonRpcMessage>,
        sent: std::sync::Arc<std::sync::Mutex<Vec<ServerJsonRpcMessage>>>,
    }

    impl FakeTransport {
        fn new(msgs: Vec<ClientJsonRpcMessage>) -> Self {
            Self {
                inbox: VecDeque::from(msgs),
                sent: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn sent(&self) -> std::sync::MutexGuard<'_, Vec<ServerJsonRpcMessage>> {
            self.sent.lock().unwrap()
        }
    }

    impl Transport<RoleServer> for FakeTransport {
        type Error = std::io::Error;

        fn send(
            &mut self,
            item: TxJsonRpcMessage<RoleServer>,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
            let sent = self.sent.clone();
            async move {
                sent.lock().unwrap().push(item);
                Ok(())
            }
        }

        async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
            self.inbox.pop_front()
        }

        async fn close(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// 从原始 JSON 行构造入站消息 (与线上 stdio 反序列化同链路)。
    fn msg(json: &str) -> ClientJsonRpcMessage {
        serde_json::from_str(json).expect("构造入站消息")
    }

    const TOOLS_LIST_NO_META: &str =
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
    const TOOLS_LIST_DRAFT_META: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#;
    const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gate-test","version":"0"}}}"#;

    /// 已发出消息中唯一的错误响应 (拦截路径断言面)。
    fn sole_error_response(sent: &[ServerJsonRpcMessage]) -> &ErrorData {
        assert_eq!(sent.len(), 1, "恰好发出一条错误响应: {sent:?}");
        match &sent[0] {
            ServerJsonRpcMessage::Error(e) => &e.error,
            other => panic!("须为 error 响应: {other:?}"),
        }
    }

    /// verdict 表驱动: pre-init 分流矩阵 (Pass/PassAndOpen/Reject 三态)。
    /// Reject 侧额外锁 id 透传。
    #[test]
    fn verdict_matrix() {
        for (json, expect_init, expect_reject) in [
            (TOOLS_LIST_NO_META, false, true),
            (TOOLS_LIST_DRAFT_META, false, false),
            (PING, false, false),
            (INITIALIZE, true, false),
            (
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                false,
                false,
            ),
        ] {
            let v = verdict(&msg(json));
            assert_eq!(matches!(v, Verdict::PassAndOpen), expect_init, "{json}");
            assert_eq!(matches!(v, Verdict::Reject(_)), expect_reject, "{json}");
        }
        // Reject 携带原请求 id
        match verdict(&msg(TOOLS_LIST_NO_META)) {
            Verdict::Reject(id) => {
                assert_eq!(id, rmcp::model::NumberOrString::Number(1));
            }
            other => panic!("须为 Reject: {other:?}"),
        }
    }

    #[test]
    fn pre_init_request_rejected_and_swallowed() {
        let fake = FakeTransport::new(vec![msg(TOOLS_LIST_NO_META), msg(INITIALIZE)]);
        let mut gate = InitGateTransport::new(fake);
        let rt = tokio::runtime::Runtime::new().unwrap();
        // 第一次 receive: 吞掉 tools/list, 返回的是 initialize
        let first = rt.block_on(gate.receive()).expect("消息");
        assert!(
            matches!(
                &first,
                ClientJsonRpcMessage::Request(r)
                    if matches!(r.request, ClientRequest::InitializeRequest(_))
            ),
            "pre-init 请求被吞, 下一条是 initialize: {first:?}"
        );
        // 错误响应形态: -32002 + 文案 + 原 id
        let sent = gate.inner.sent();
        let err = sole_error_response(&sent);
        assert_eq!(err.code.0, -32002);
        assert_eq!(err.message, SERVER_NOT_INITIALIZED_MSG);
        assert!(
            err.message.contains("not initialized") && err.message.contains("initialize"),
            "文案自明: {}",
            err.message
        );
        assert!(
            matches!(
                &sent[0],
                ServerJsonRpcMessage::Error(e)
                    if e.id == Some(rmcp::model::NumberOrString::Number(1))
            ),
            "错误响应携带原请求 id: {:?}",
            sent[0]
        );
    }

    #[test]
    fn pre_init_ping_passes_through() {
        let fake = FakeTransport::new(vec![msg(PING)]);
        let mut gate = InitGateTransport::new(fake);
        let out = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(gate.receive())
            .expect("ping 透传");
        assert!(matches!(
            &out,
            ClientJsonRpcMessage::Request(r) if matches!(r.request, ClientRequest::PingRequest(_))
        ));
        assert!(gate.inner.sent().is_empty(), "ping 不触发闸门错误");
    }

    /// draft discover 生命周期: 带完整内联 `_meta` 的请求不依赖 initialize,
    /// 透传给 rmcp 自行处理 (版本校验/分发), 闸门不误伤。
    #[test]
    fn pre_init_draft_meta_request_passes_through() {
        let fake = FakeTransport::new(vec![msg(TOOLS_LIST_DRAFT_META)]);
        let mut gate = InitGateTransport::new(fake);
        let out = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(gate.receive())
            .expect("draft 请求透传");
        assert!(
            matches!(
                &out,
                ClientJsonRpcMessage::Request(r)
                    if matches!(r.request, ClientRequest::ListToolsRequest(_))
            ),
            "原样上交: {out:?}"
        );
        assert!(gate.inner.sent().is_empty());
    }

    #[test]
    fn initialize_opens_gate_for_all_messages() {
        let fake = FakeTransport::new(vec![
            msg(INITIALIZE),
            msg(TOOLS_LIST_NO_META), // 落闸后: 无 _meta 也透传
            msg(PING),
        ]);
        let mut gate = InitGateTransport::new(fake);
        let rt = tokio::runtime::Runtime::new().unwrap();
        for _ in 0..3 {
            assert!(rt.block_on(gate.receive()).is_some(), "全部透传");
        }
        assert!(gate.inner.sent().is_empty(), "落闸后零拦截");
    }

    /// pre-init notification 原样透传 (维持 rmcp 现行为, 本闸门只管 Request)。
    #[test]
    fn pre_init_notification_passes_through() {
        let fake = FakeTransport::new(vec![msg(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )]);
        let mut gate = InitGateTransport::new(fake);
        let out = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(gate.receive())
            .expect("notification 透传");
        assert!(matches!(out, ClientJsonRpcMessage::Notification(_)));
        assert!(gate.inner.sent().is_empty());
    }
}
