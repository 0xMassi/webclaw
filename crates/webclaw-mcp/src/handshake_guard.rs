//! Keeps the stdio server alive when a client speaks before the handshake.
//!
//! rmcp's server handshake reads messages in two windows, and each one treats
//! anything it does not expect as fatal:
//!
//! 1. before `initialize` it tolerates only `ping`;
//! 2. between `initialize` and `notifications/initialized` it tolerates only
//!    `ping` and `logging/setLevel`.
//!
//! Anything else makes `serve()` return `ExpectedInitializeRequest` /
//! `ExpectedInitializedNotification` **without writing a reply**, and the
//! transport is dropped with it. On stdio that ends the process, so the client
//! sees a closed pipe rather than an error it can act on.
//!
//! That became a real interop problem with MCP revision `2026-07-28`, whose
//! SEP-2575 defines `server/discover` as a probe a dual-era client sends
//! *before* `initialize` to find out which era it is talking to. The official
//! Go and Python clients run that probe by default and then fall back to the
//! legacy `initialize` handshake **on the same connection**. A server that
//! exits during the probe takes the connection with it, so the fallback never
//! runs and the client cannot connect at all (issue #107).
//!
//! Note the spec does not require a reply here: a legacy server that stays
//! silent and lets the client time out is explicitly allowed. The problem is
//! specifically that we *die*, which is neither a reply nor a timeout.
//!
//! This wrapper sits between rmcp and the real transport and answers those
//! messages itself, so rmcp's state machine only ever sees what it expects:
//!
//! - a request rmcp would reject gets JSON-RPC `-32602` when the method exists
//!   and only the request is malformed, and `-32601` when it does not;
//! - a notification, response or error gets dropped, since JSON-RPC forbids
//!   replying to those and dropping is enough to keep the handshake alive.
//!
//! The handshake rules stay strict: nothing is executed early, and rmcp still
//! decides when the connection is initialized. Once it is, every message is
//! passed straight through and rmcp's own dispatch answers unknown methods.
//!
//! One case is deliberately NOT answered here. rmcp serves a pre-`initialize`
//! request statelessly when it carries every `_meta` key 2026-07-28 requires,
//! and answering such a probe ourselves would tell a modern client we are a
//! legacy server and cost it the newer protocol. `is_stateless_request` mirrors
//! rmcp's own gate so those reach rmcp untouched; only the probes rmcp would
//! die on are absorbed.
//!
//! A line the codec cannot decode is handled by rmcp itself since 1.7.0, which
//! skips the bad frame and keeps reading rather than reporting end-of-stream
//! (issue #109). This wrapper sits above the codec and never sees those.

use rmcp::model::{
    ClientJsonRpcMessage, ClientNotification, ClientRequest, ErrorCode, ErrorData, GetMeta,
    ProtocolVersion, ServerJsonRpcMessage,
};
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;

/// Which of rmcp's handshake windows we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Before `initialize`. rmcp tolerates `ping` only.
    AwaitingInitialize,
    /// After `initialize`, before `notifications/initialized`.
    /// rmcp tolerates `ping` and `logging/setLevel`.
    AwaitingInitialized,
    /// Handshake complete. rmcp handles everything from here.
    Open,
}

/// Wraps a transport so pre-handshake traffic cannot kill the connection.
pub struct PreHandshakeGuard<T> {
    inner: T,
    phase: Phase,
}

impl<T> PreHandshakeGuard<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            phase: Phase::AwaitingInitialize,
        }
    }
}

/// Is this message one rmcp will accept in the given window?
///
/// Mirrors rmcp's two handshake loops. Kept as a free function so the tests can
/// assert on the classification without driving a transport.
fn is_tolerated(msg: &ClientJsonRpcMessage, phase: Phase) -> bool {
    if phase == Phase::Open {
        return true;
    }
    match msg {
        // rmcp serves a request carrying the full 2026-07-28 `_meta` set
        // statelessly, without an `initialize` first. Passing those through is
        // what lets a modern client negotiate 2026-07-28 instead of falling
        // back to the legacy handshake, so the guard must not answer them.
        ClientJsonRpcMessage::Request(req)
            if phase == Phase::AwaitingInitialize && is_stateless_request(&req.request) =>
        {
            true
        }
        ClientJsonRpcMessage::Request(req) => matches!(
            (&req.request, phase),
            (ClientRequest::PingRequest(_), _)
                | (
                    ClientRequest::InitializeRequest(_),
                    Phase::AwaitingInitialize
                )
                | (
                    ClientRequest::SetLevelRequest(_),
                    Phase::AwaitingInitialized
                )
        ),
        ClientJsonRpcMessage::Notification(n) => {
            phase == Phase::AwaitingInitialized
                && matches!(
                    n.notification,
                    ClientNotification::InitializedNotification(_)
                )
        }
        _ => false,
    }
}

/// Can rmcp serve this request without an `initialize` first?
///
/// Mirrors rmcp's own gate: a pre-`initialize` request is dispatched
/// statelessly only when it carries every `_meta` key the 2026-07-28 revision
/// requires. A request missing any of them takes rmcp's fatal path instead,
/// which is the case this guard exists to absorb.
fn is_stateless_request(request: &ClientRequest) -> bool {
    !matches!(request, ClientRequest::InitializeRequest(_))
        && request
            .get_meta()
            .missing_required_keys(&ProtocolVersion::V_2026_07_28)
            .is_empty()
}

/// Which JSON-RPC error to refuse a pre-handshake request with.
///
/// The two cases are genuinely different and a client can act on the
/// difference. A method rmcp has no type for does not exist here, so
/// `-32601 Method not found` is the honest answer. A method it does know
/// reached us in a state it cannot serve, which for a request outside the
/// handshake means the required `_meta` is missing: revision 2026-07-28 makes
/// `io.modelcontextprotocol/protocolVersion` and
/// `io.modelcontextprotocol/clientCapabilities` mandatory on every request.
/// The method exists, so the request is what is wrong, and `-32602 Invalid
/// params` says so.
///
/// Telling a caller "method not found" for a method we implement sends them
/// looking for a missing feature instead of at their own request.
fn refusal_for(request: &ClientRequest, method: &str) -> (ErrorCode, String) {
    if matches!(request, ClientRequest::CustomRequest(_)) {
        (ErrorCode::METHOD_NOT_FOUND, method.to_string())
    } else {
        (
            ErrorCode::INVALID_PARAMS,
            format!(
                "{method} requires the 2026-07-28 request _meta \
                 (io.modelcontextprotocol/protocolVersion and \
                 io.modelcontextprotocol/clientCapabilities), or an \
                 initialize handshake first"
            ),
        )
    }
}

/// Advance the handshake state for a message we are about to pass through.
fn advance(phase: Phase, msg: &ClientJsonRpcMessage) -> Phase {
    match (phase, msg) {
        (Phase::AwaitingInitialize, ClientJsonRpcMessage::Request(req))
            if matches!(req.request, ClientRequest::InitializeRequest(_)) =>
        {
            Phase::AwaitingInitialized
        }
        (Phase::AwaitingInitialized, ClientJsonRpcMessage::Notification(n))
            if matches!(
                n.notification,
                ClientNotification::InitializedNotification(_)
            ) =>
        {
            Phase::Open
        }
        _ => phase,
    }
}

impl<T> Transport<RoleServer> for PreHandshakeGuard<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            let msg = self.inner.receive().await?;

            if is_tolerated(&msg, self.phase) {
                self.phase = advance(self.phase, &msg);
                return Some(msg);
            }

            // rmcp would treat this as fatal. Answer it here instead.
            match &msg {
                ClientJsonRpcMessage::Request(req) => {
                    let method = req.request.method().to_string();
                    let (code, message) = refusal_for(&req.request, &method);
                    tracing::debug!(
                        method = %method,
                        phase = ?self.phase,
                        code = code.0,
                        "answering pre-handshake request"
                    );
                    let reply = ServerJsonRpcMessage::error(
                        ErrorData::new(code, message, None),
                        Some(req.id.clone()),
                    );
                    if let Err(e) = self.inner.send(reply).await {
                        // The pipe is gone; the next receive reports EOF.
                        tracing::debug!(error = %e, "failed to send refusal");
                    }
                }
                // No reply is owed for these, and dropping them is what keeps
                // the handshake alive.
                other => tracing::debug!(
                    phase = ?self.phase,
                    "dropping unexpected pre-handshake message: {}",
                    match other {
                        ClientJsonRpcMessage::Notification(_) => "notification",
                        ClientJsonRpcMessage::Response(_) => "response",
                        ClientJsonRpcMessage::Error(_) => "error",
                        ClientJsonRpcMessage::Request(_) => unreachable!("handled above"),
                    }
                ),
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// Replays a scripted client and records what the server wrote back.
    struct MockTransport {
        incoming: VecDeque<ClientJsonRpcMessage>,
        sent: Arc<Mutex<Vec<ServerJsonRpcMessage>>>,
    }

    /// Hand-rolled so the test does not pull in an error-derive dependency.
    #[derive(Debug)]
    struct MockError;

    impl std::fmt::Display for MockError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("mock transport error")
        }
    }

    impl std::error::Error for MockError {}

    impl MockTransport {
        fn new(lines: &[&str]) -> (Self, Arc<Mutex<Vec<ServerJsonRpcMessage>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            let incoming = lines
                .iter()
                .map(|l| serde_json::from_str(l).expect("test message must parse"))
                .collect();
            (
                Self {
                    incoming,
                    sent: sent.clone(),
                },
                sent,
            )
        }
    }

    impl Transport<RoleServer> for MockTransport {
        type Error = MockError;

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

        async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
            self.incoming.pop_front()
        }

        async fn close(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"t","version":"1"},"capabilities":{}}}"#;
    const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    const DISCOVER: &str = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}"#;

    /// A probe carrying the `_meta` keys 2026-07-28 requires. rmcp serves this
    /// one itself, so the guard must not intercept it.
    const DISCOVER_WITH_META: &str = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#;

    fn method_of(msg: &ClientJsonRpcMessage) -> Option<String> {
        match msg {
            ClientJsonRpcMessage::Request(r) => Some(r.request.method().to_string()),
            _ => None,
        }
    }

    /// The reported case: the probe must be answered, not fatal, and the
    /// `initialize` behind it must still reach rmcp.
    #[tokio::test]
    async fn discover_before_initialize_is_answered_and_initialize_survives() {
        let (mock, sent) = MockTransport::new(&[DISCOVER, INITIALIZE, INITIALIZED]);
        let mut guard = PreHandshakeGuard::new(mock);

        let first = guard.receive().await.expect("initialize must be delivered");
        assert_eq!(method_of(&first).as_deref(), Some("initialize"));

        let second = guard
            .receive()
            .await
            .expect("initialized must be delivered");
        assert!(matches!(second, ClientJsonRpcMessage::Notification(_)));

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "exactly one reply, for the probe");
        let json = serde_json::to_value(&sent[0]).unwrap();
        assert_eq!(json["error"]["code"], -32602, "the method exists here");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("server/discover")
        );
        assert_eq!(json["id"], 1, "reply must carry the probe's id");
    }

    /// A known method before `initialize` is fatal to rmcp too, so the guard
    /// must not special-case `server/discover`.
    #[tokio::test]
    async fn any_unexpected_pre_handshake_request_is_answered() {
        let (mock, sent) = MockTransport::new(&[
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}"#,
            INITIALIZE,
        ]);
        let mut guard = PreHandshakeGuard::new(mock);

        let first = guard.receive().await.expect("initialize must be delivered");
        assert_eq!(method_of(&first).as_deref(), Some("initialize"));

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            serde_json::to_value(&sent[0]).unwrap()["error"]["code"],
            -32602,
            "tools/list exists; the request is what was wrong"
        );
    }

    /// rmcp answers pre-init `ping` itself, so the guard must pass it through
    /// untouched rather than shadowing it with an error.
    #[tokio::test]
    async fn ping_is_passed_through_untouched() {
        let (mock, sent) =
            MockTransport::new(&[r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, INITIALIZE]);
        let mut guard = PreHandshakeGuard::new(mock);

        let first = guard.receive().await.expect("ping must reach rmcp");
        assert_eq!(method_of(&first).as_deref(), Some("ping"));
        assert!(
            sent.lock().unwrap().is_empty(),
            "guard must not reply to ping"
        );
    }

    /// JSON-RPC forbids replying to a notification, and rmcp treats an
    /// unexpected one as fatal, so it has to be dropped silently.
    #[tokio::test]
    async fn unexpected_notification_is_dropped_without_a_reply() {
        let (mock, sent) = MockTransport::new(&[
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
            INITIALIZE,
        ]);
        let mut guard = PreHandshakeGuard::new(mock);

        let first = guard.receive().await.expect("initialize must be delivered");
        assert_eq!(method_of(&first).as_deref(), Some("initialize"));
        assert!(sent.lock().unwrap().is_empty());
    }

    /// `notifications/initialized` is only meaningful after `initialize`.
    /// Honouring it earlier would open the guard and hand rmcp a message its
    /// first window rejects, reintroducing the crash this module prevents.
    #[tokio::test]
    async fn initialized_before_initialize_does_not_open_the_guard() {
        let (mock, _sent) = MockTransport::new(&[
            INITIALIZED,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/list","params":{}}"#,
            INITIALIZE,
        ]);
        let mut guard = PreHandshakeGuard::new(mock);

        // The stray notification is dropped and `tools/list` is still
        // intercepted, proving the guard did not move to Open.
        let first = guard.receive().await.expect("initialize must be delivered");
        assert_eq!(method_of(&first).as_deref(), Some("initialize"));
    }

    /// Between `initialize` and `initialized`, rmcp tolerates `logging/setLevel`
    /// (VS Code sends it there) but nothing else.
    #[tokio::test]
    async fn set_level_passes_only_in_the_second_window() {
        let set_level =
            r#"{"jsonrpc":"2.0","id":3,"method":"logging/setLevel","params":{"level":"info"}}"#;

        // Before `initialize`: not tolerated, so it is answered.
        let (mock, sent) = MockTransport::new(&[set_level, INITIALIZE]);
        let mut guard = PreHandshakeGuard::new(mock);
        let first = guard.receive().await.unwrap();
        assert_eq!(method_of(&first).as_deref(), Some("initialize"));
        assert_eq!(sent.lock().unwrap().len(), 1);

        // After `initialize`: tolerated, so it passes through.
        let (mock, sent) = MockTransport::new(&[INITIALIZE, set_level]);
        let mut guard = PreHandshakeGuard::new(mock);
        let _init = guard.receive().await.unwrap();
        let second = guard.receive().await.unwrap();
        assert_eq!(method_of(&second).as_deref(), Some("logging/setLevel"));
        assert!(sent.lock().unwrap().is_empty());
    }

    /// After the handshake, rmcp's own dispatch answers unknown methods, so the
    /// guard must stop intercepting or it would shadow real responses.
    #[tokio::test]
    async fn guard_stops_intercepting_once_open() {
        let (mock, sent) = MockTransport::new(&[INITIALIZE, INITIALIZED, DISCOVER]);
        let mut guard = PreHandshakeGuard::new(mock);

        let _init = guard.receive().await.unwrap();
        let _initialized = guard.receive().await.unwrap();
        let third = guard.receive().await.expect("must reach rmcp once open");
        assert_eq!(method_of(&third).as_deref(), Some("server/discover"));
        assert!(
            sent.lock().unwrap().is_empty(),
            "rmcp answers this after the handshake, not the guard"
        );
    }

    /// rmcp serves a fully-specified 2026-07-28 probe statelessly. Answering it
    /// here would advertise us as a legacy server and cost a modern client the
    /// newer protocol, so it has to reach rmcp untouched.
    #[tokio::test]
    async fn conformant_probe_is_passed_through_for_rmcp_to_serve() {
        let (mock, sent) = MockTransport::new(&[DISCOVER_WITH_META]);
        let mut guard = PreHandshakeGuard::new(mock);

        let first = guard
            .receive()
            .await
            .expect("a stateless-capable probe must reach rmcp");
        assert_eq!(method_of(&first).as_deref(), Some("server/discover"));
        assert!(
            sent.lock().unwrap().is_empty(),
            "rmcp answers this one, not the guard"
        );
    }

    /// The same method WITHOUT the required `_meta` is the case rmcp dies on,
    /// so it must still be absorbed. This is the pair that keeps both
    /// behaviours honest.
    #[tokio::test]
    async fn probe_missing_required_meta_is_still_answered() {
        let (mock, sent) = MockTransport::new(&[DISCOVER, INITIALIZE, INITIALIZED]);
        let mut guard = PreHandshakeGuard::new(mock);

        let first = guard.receive().await.expect("initialize must be delivered");
        assert_eq!(method_of(&first).as_deref(), Some("initialize"));
        assert_eq!(sent.lock().unwrap().len(), 1, "the bare probe was answered");
    }

    /// A method the server implements, refused because the request lacks the
    /// `_meta` 2026-07-28 requires, is a params problem and not a missing
    /// method. Reporting -32601 sent a conformance tester looking for an
    /// unimplemented feature instead of at its own request.
    #[tokio::test]
    async fn known_method_without_required_meta_is_invalid_params() {
        let (mock, sent) = MockTransport::new(&[DISCOVER, INITIALIZE, INITIALIZED]);
        let mut guard = PreHandshakeGuard::new(mock);
        let _ = guard.receive().await.expect("initialize must be delivered");

        let sent = sent.lock().unwrap();
        let json = serde_json::to_value(&sent[0]).unwrap();
        assert_eq!(json["error"]["code"], -32602, "server/discover exists here");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("clientCapabilities"),
            "the message should name what is missing, got {}",
            json["error"]["message"]
        );
    }

    /// A method that genuinely does not exist still gets -32601, so the two
    /// cases stay distinguishable to a caller.
    #[tokio::test]
    async fn unknown_method_is_still_method_not_found() {
        let (mock, sent) = MockTransport::new(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"nonsense/method","params":{}}"#,
            INITIALIZE,
            INITIALIZED,
        ]);
        let mut guard = PreHandshakeGuard::new(mock);
        let _ = guard.receive().await.expect("initialize must be delivered");

        let sent = sent.lock().unwrap();
        let json = serde_json::to_value(&sent[0]).unwrap();
        assert_eq!(json["error"]["code"], -32601);
        assert_eq!(json["error"]["message"], "nonsense/method");
    }

    /// End of stream must still terminate the loop rather than spin.
    #[tokio::test]
    async fn eof_returns_none() {
        let (mock, _sent) = MockTransport::new(&[DISCOVER]);
        let mut guard = PreHandshakeGuard::new(mock);
        assert!(guard.receive().await.is_none());
    }
}
