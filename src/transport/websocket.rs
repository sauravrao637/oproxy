use axum::body::Body;
use futures_util::{SinkExt, StreamExt};
use hyper::body::Incoming;
use hyper::{Request, Response};
use tokio::sync::watch;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{
        handshake::client::generate_key,
        protocol::{CloseFrame, Message, Role},
    },
};

use crate::middleware::plugins::mock::{MockBehavior, SharedMockRules, WsFrameAction};
use crate::session::SharedSessionManager;
use crate::transport::TransportContext;
use crate::transport::lifecycle::{ConnectionSupervisor, wait_for_shutdown};

struct WebSocketRequest {
    headers: hyper::HeaderMap,
    host: String,
    port: u16,
    scheme: &'static str,
    url: String,
    connection_id: Option<String>,
    stream_id: Option<u64>,
    downstream_protocol: Option<String>,
}

struct MockWebSocketContext {
    session_manager: SharedSessionManager,
    connections: ConnectionSupervisor,
    session_id: String,
    peer: Option<std::net::SocketAddr>,
    shutdown: watch::Receiver<bool>,
}

type UpstreamWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

impl WebSocketRequest {
    fn prepare(req: &Request<Incoming>) -> Self {
        let uri = req.uri();
        let headers = req.headers().clone();
        let header_host = headers
            .get("host")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let host = uri.host().map(str::to_string).unwrap_or_else(|| {
            header_host
                .split(':')
                .next()
                .unwrap_or_default()
                .to_string()
        });
        let requested_scheme = uri.scheme_str().unwrap_or("ws");
        let default_port = if matches!(requested_scheme, "wss" | "https") {
            443
        } else {
            80
        };
        let port = uri.port_u16().unwrap_or(default_port);
        let scheme = if matches!(requested_scheme, "wss" | "https") || port == 443 {
            "wss"
        } else {
            "ws"
        };
        let path = uri
            .path_and_query()
            .map(hyper::http::uri::PathAndQuery::as_str)
            .unwrap_or("/");
        let connection = req
            .extensions()
            .get::<crate::transport::http::DownstreamConn>();

        Self {
            headers,
            host: host.clone(),
            port,
            scheme,
            url: format!("{scheme}://{host}:{port}{path}"),
            connection_id: connection.map(|value| value.id.clone()),
            stream_id: connection.map(|value| value.next_stream()),
            downstream_protocol: Some(
                crate::core::engine::protocol_label(req.version()).to_string(),
            ),
        }
    }

    fn request_context(&self) -> crate::middleware::RequestContext {
        ws_request_context(
            &self.url,
            &self.headers,
            &self.host,
            self.connection_id.clone(),
            self.stream_id,
            self.downstream_protocol.clone(),
            self.scheme,
        )
    }

    fn response_context(&self, subprotocol: Option<&str>) -> crate::middleware::ResponseContext {
        ws_response_context(
            &self.url,
            &self.headers,
            &self.host,
            self.connection_id.clone(),
            self.stream_id,
            self.scheme,
            subprotocol,
        )
    }

    fn upstream_request(
        &self,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
        let mut request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&self.url)
            .header("host", format!("{}:{}", self.host, self.port))
            .header("upgrade", "websocket")
            .header("connection", "Upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", generate_key());

        for (name, value) in &self.headers {
            if is_websocket_handshake_header(name.as_str()) {
                continue;
            }
            if let Ok(value) = value.to_str() {
                request = request.header(name.as_str(), value);
            }
        }
        request.body(()).map_err(|error| error.to_string())
    }
}

fn is_websocket_handshake_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "upgrade"
            | "connection"
            | "sec-websocket-key"
            | "sec-websocket-version"
            | "sec-websocket-accept"
            | "sec-websocket-extensions"
    )
}

async fn connect_upstream(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
    timeout: std::time::Duration,
) -> Result<(UpstreamWebSocket, Option<String>), Response<Body>> {
    match tokio::time::timeout(
        timeout,
        connect_async_tls_with_config(request, None, false, None),
    )
    .await
    {
        Ok(Ok((socket, response))) => {
            let subprotocol = response
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            Ok((socket, subprotocol))
        }
        Ok(Err(error)) => {
            tracing::warn!(%error, "WS upstream connect failed");
            Err(Response::builder()
                .status(502)
                .body(Body::from("WebSocket upstream connect failed"))
                .expect("static 502 response is always valid"))
        }
        Err(_) => Err(Response::builder()
            .status(504)
            .body(Body::from("WebSocket upstream connect timed out"))
            .expect("static 504 response is always valid")),
    }
}

pub fn is_websocket_upgrade<B>(req: &Request<B>) -> bool {
    req.headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

fn map_tungstenite_message_to_record(
    msg: &Message,
) -> Option<(u8, usize, Option<String>, Option<String>)> {
    match msg {
        Message::Text(text) => {
            let preview = text.chars().take(512).collect::<String>();
            Some((0x1, text.len(), Some(preview), None))
        }
        Message::Binary(data) => {
            let chunk = &data[..data.len().min(64)];
            let hex: String = chunk.iter().map(|b| format!("{:02x}", b)).collect();
            Some((
                0x2,
                data.len(),
                None,
                if hex.is_empty() { None } else { Some(hex) },
            ))
        }
        Message::Ping(_) => Some((0x9, 0, None, None)),
        Message::Pong(_) => Some((0xA, 0, None, None)),
        Message::Close(_) => Some((0x8, 0, None, None)),
        Message::Frame(_) => None,
    }
}

pub async fn handle_websocket(
    req: Request<Incoming>,
    context: TransportContext,
    session_id: String,
    peer: Option<std::net::SocketAddr>,
    mut shutdown: watch::Receiver<bool>,
) -> Response<Body> {
    let sm = context.session_manager.clone();
    let breakpoint_manager = context.breakpoint_manager.clone();
    let mock_rules = context.mock_rules.clone();
    let connections = context.connections.clone();
    let inspect_frames = context.inspect_ws_frames;
    let connect_timeout = context.connect_timeout;
    let handshake_timeout = context.handshake_timeout;

    let prepared = WebSocketRequest::prepare(&req);
    let request_context = prepared.request_context();

    if let Some(script) = select_ws_mock_script(&mock_rules, &request_context).await {
        return handle_mock_websocket(
            req,
            prepared,
            request_context,
            script,
            MockWebSocketContext {
                session_manager: sm,
                connections,
                session_id,
                peer,
                shutdown,
            },
        )
        .await;
    }

    let ws_req = match prepared.upstream_request() {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(%error, "WS request build failed");
            return Response::builder()
                .status(502)
                .body(Body::from("WebSocket request build failed"))
                .expect("static 502 response is always valid");
        }
    };

    // Connect to the upstream WebSocket server (with TLS if required).
    let (upstream_ws, negotiated_subprotocol) =
        match connect_upstream(ws_req, connect_timeout + handshake_timeout).await {
            Ok(connection) => connection,
            Err(response) => return response,
        };

    // Record the session request head.
    sm.record_request(session_id.clone(), request_context);
    sm.record_response_with_metrics(
        session_id.clone(),
        prepared.response_context(negotiated_subprotocol.as_deref()),
        ws_upgrade_metrics(),
    );

    // Complete the h1 upgrade handshake with the downstream client.
    let on_upgrade = hyper::upgrade::on(req);
    connections.spawn_tracked("websocket-tunnel", peer, async move {
        let upgraded = tokio::select! {
            upgraded = on_upgrade => upgraded,
            _ = wait_for_shutdown(&mut shutdown) => {
                tracing::debug!("WS client upgrade stopped by shutdown");
                return;
            }
        };
        let upgraded = match upgraded {
            Ok(u) => u,
            Err(e) => {
                tracing::debug!(error=%e, "WS client upgrade failed");
                return;
            }
        };

        let client_io = hyper_util::rt::TokioIo::new(upgraded);
        let client_ws = WebSocketStream::from_raw_socket(client_io, Role::Server, None).await;

        relay_ws(
            client_ws,
            upstream_ws,
            WsRelayContext {
                session_manager: sm,
                breakpoint_manager,
                session_id,
                frame_uri: prepared.url,
                frame_host: prepared.host,
                inspect_frames,
            },
            &mut shutdown,
        )
        .await;
    });

    websocket_upgrade_response(&prepared.headers, negotiated_subprotocol.as_deref())
}

async fn handle_mock_websocket(
    req: Request<Incoming>,
    prepared: WebSocketRequest,
    request_context: crate::middleware::RequestContext,
    script: WsMockScript,
    context: MockWebSocketContext,
) -> Response<Body> {
    let MockWebSocketContext {
        session_manager,
        connections,
        session_id,
        peer,
        mut shutdown,
    } = context;
    let subprotocol = first_offered_subprotocol(&prepared.headers);
    session_manager.record_request(session_id.clone(), request_context);
    session_manager.record_response_with_metrics(
        session_id.clone(),
        prepared.response_context(subprotocol.as_deref()),
        ws_upgrade_metrics(),
    );
    session_manager.append_event(
        &session_id,
        crate::session::SessionEvent::MockServed {
            rule_id: script.rule_id,
            behavior: "websocket_script".to_string(),
        },
    );

    let response = websocket_upgrade_response(&prepared.headers, subprotocol.as_deref());
    let on_upgrade = hyper::upgrade::on(req);
    connections.spawn_tracked("websocket-mock", peer, async move {
        let upgraded = tokio::select! {
            upgraded = on_upgrade => upgraded,
            _ = wait_for_shutdown(&mut shutdown) => {
                tracing::debug!("WS mock stopped before client upgrade");
                return;
            }
        };
        let upgraded = match upgraded {
            Ok(upgraded) => upgraded,
            Err(error) => {
                tracing::debug!(%error, "WS mock client upgrade failed");
                return;
            }
        };
        let client_io = hyper_util::rt::TokioIo::new(upgraded);
        let client_ws = WebSocketStream::from_raw_socket(client_io, Role::Server, None).await;
        run_ws_mock_script(
            client_ws,
            session_manager,
            session_id,
            script.frames,
            &mut shutdown,
        )
        .await;
    });
    response
}

#[derive(Debug, Clone)]
struct WsMockScript {
    rule_id: String,
    frames: Vec<WsFrameAction>,
}

fn ws_request_context(
    record_uri: &str,
    headers: &hyper::HeaderMap,
    target_host: &str,
    ws_connection_id: Option<String>,
    ws_stream_id: Option<u64>,
    ws_downstream_protocol: Option<String>,
    upstream_scheme: &str,
) -> crate::middleware::RequestContext {
    let mut req_headers_map = crate::middleware::HeaderMap::new();
    for (k, v) in headers {
        if let Ok(v) = v.to_str() {
            req_headers_map.append(k.to_string(), v.to_string());
        }
    }
    crate::middleware::RequestContext {
        method: "WS".to_string(),
        uri: record_uri.to_string(),
        headers: req_headers_map,
        body: bytes::Bytes::new(),
        host: target_host.to_string(),
        connection_id: ws_connection_id.clone(),
        stream_id: ws_stream_id,
        downstream_protocol: ws_downstream_protocol,
        protocol_context: Some(
            crate::core::forward::ProtocolContext::websocket(upstream_scheme)
                .with_identity(ws_connection_id, ws_stream_id),
        ),
        ..Default::default()
    }
}

async fn select_ws_mock_script(
    rules: &SharedMockRules,
    ctx: &crate::middleware::RequestContext,
) -> Option<WsMockScript> {
    let snapshots = {
        let rules = rules.read().await;
        rules
            .iter()
            .filter(|rule| rule.enabled)
            .cloned()
            .collect::<Vec<_>>()
    };

    for rule in snapshots {
        let Some(MockBehavior::WebSocketScript { frames }) = rule.behavior.clone() else {
            continue;
        };
        if !rule.matches(ctx) {
            continue;
        }
        // Look the live rule up by id, not snapshot index (rules may have been
        // edited/reordered concurrently).
        let mut rules = rules.write().await;
        if let Some(live) = rules.iter_mut().find(|r| r.id == rule.id) {
            live.call_count += 1;
        }
        return Some(WsMockScript {
            rule_id: rule.id,
            frames,
        });
    }
    None
}

fn websocket_upgrade_response(
    headers: &hyper::HeaderMap,
    subprotocol: Option<&str>,
) -> Response<Body> {
    let mut builder = Response::builder().status(101);
    builder = builder.header("upgrade", "websocket");
    builder = builder.header("connection", "Upgrade");
    if let Some(key) = headers
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
    {
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
        builder = builder.header("sec-websocket-accept", accept);
    }
    if let Some(proto) = subprotocol.filter(|p| !p.is_empty()) {
        builder = builder.header("sec-websocket-protocol", proto);
    }
    builder.body(Body::empty()).unwrap_or_else(|_| {
        Response::builder()
            .status(500)
            .body(Body::empty())
            .expect("static 500 response is always valid")
    })
}

/// First subprotocol the client offered, used when oproxy itself terminates the
/// handshake (mock scripts) and must accept one to stay RFC 6455-conformant.
fn first_offered_subprotocol(headers: &hyper::HeaderMap) -> Option<String> {
    headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
}

fn ws_response_context(
    record_uri: &str,
    request_headers: &hyper::HeaderMap,
    target_host: &str,
    ws_connection_id: Option<String>,
    ws_stream_id: Option<u64>,
    upstream_scheme: &str,
    subprotocol: Option<&str>,
) -> crate::middleware::ResponseContext {
    let mut headers = crate::middleware::HeaderMap::new();
    headers.append("upgrade".to_string(), "websocket".to_string());
    headers.append("connection".to_string(), "Upgrade".to_string());
    if let Some(key) = request_headers
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
    {
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
        headers.append("sec-websocket-accept".to_string(), accept);
    }
    if let Some(proto) = subprotocol.filter(|p| !p.is_empty()) {
        headers.append("sec-websocket-protocol".to_string(), proto.to_string());
    }

    crate::middleware::ResponseContext {
        status: 101,
        headers,
        body: bytes::Bytes::new(),
        request_uri: record_uri.to_string(),
        session_id: None,
        ttfb_ms: 0,
        body_ms: 0,
        tags: vec!["ws".to_string()],
        request_host: target_host.to_string(),
        request_method: "WS".to_string(),
        protocol: Some("WebSocket".to_string()),
        response_body_observer_pending: false,
        protocol_context: Some(
            crate::core::forward::ProtocolContext::websocket(upstream_scheme)
                .with_identity(ws_connection_id, ws_stream_id),
        ),
    }
}

fn ws_upgrade_metrics() -> crate::session::InspectionMetrics {
    crate::session::InspectionMetrics {
        status_code: 101,
        protocol: Some("WebSocket".to_string()),
        ..Default::default()
    }
}

async fn run_ws_mock_script(
    mut client: WebSocketStream<hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>>,
    sm: crate::session::SharedSessionManager,
    session_id: String,
    frames: Vec<WsFrameAction>,
    shutdown: &mut watch::Receiver<bool>,
) {
    for action in frames {
        if action.delay_ms > 0 {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(action.delay_ms)) => {}
                _ = wait_for_shutdown(shutdown) => return,
            }
        }
        let msg = ws_action_to_message(&action);
        record_frame(
            &sm,
            &session_id,
            &msg,
            crate::session::WsDirection::ServerToClient,
        );
        let is_close = msg.is_close();
        if client.send(msg).await.is_err() || is_close {
            return;
        }
    }
    let _ = client.close(None).await;
}

fn ws_action_to_message(action: &WsFrameAction) -> Message {
    match action.opcode {
        0x1 => Message::Text(action.payload.clone()),
        0x2 => Message::Binary(action.payload.as_bytes().to_vec()),
        0x8 => Message::Close(Some(CloseFrame {
            code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
            reason: action.payload.clone().into(),
        })),
        0x9 => Message::Ping(action.payload.as_bytes().to_vec()),
        0xA => Message::Pong(action.payload.as_bytes().to_vec()),
        _ => Message::Text(action.payload.clone()),
    }
}

struct WsRelayContext {
    session_manager: crate::session::SharedSessionManager,
    breakpoint_manager: std::sync::Arc<crate::middleware::plugins::breakpoints::BreakpointManager>,
    session_id: String,
    frame_uri: String,
    frame_host: String,
    inspect_frames: bool,
}

async fn relay_ws(
    mut client: WebSocketStream<hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>>,
    mut upstream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    relay: WsRelayContext,
    shutdown: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            msg = client.next() => {
                match msg {
                    Some(Ok(m)) => {
                        let Some(m) = maybe_pause_frame(
                            &relay.breakpoint_manager,
                            &relay.session_manager,
                            &relay.session_id,
                            &relay.frame_uri,
                            &relay.frame_host,
                            crate::session::WsDirection::ClientToServer,
                            m,
                        ).await else {
                            continue;
                        };
                        if relay.inspect_frames {
                            record_frame(&relay.session_manager, &relay.session_id, &m, crate::session::WsDirection::ClientToServer);
                        }
                        let is_close = m.is_close();
                        if upstream.send(m).await.is_err() || is_close {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            msg = upstream.next() => {
                match msg {
                    Some(Ok(m)) => {
                        let Some(m) = maybe_pause_frame(
                            &relay.breakpoint_manager,
                            &relay.session_manager,
                            &relay.session_id,
                            &relay.frame_uri,
                            &relay.frame_host,
                            crate::session::WsDirection::ServerToClient,
                            m,
                        ).await else {
                            continue;
                        };
                        if relay.inspect_frames {
                            record_frame(&relay.session_manager, &relay.session_id, &m, crate::session::WsDirection::ServerToClient);
                        }
                        let is_close = m.is_close();
                        if client.send(m).await.is_err() || is_close {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            _ = wait_for_shutdown(shutdown) => {
                tracing::debug!("WS relay stopped by shutdown");
                break;
            }
        }
    }
    let _ = client.close(None).await;
    let _ = upstream.close(None).await;
}

async fn maybe_pause_frame(
    breakpoint_manager: &crate::middleware::plugins::breakpoints::BreakpointManager,
    sm: &crate::session::SharedSessionManager,
    session_id: &str,
    uri: &str,
    host: &str,
    direction: crate::session::WsDirection,
    msg: Message,
) -> Option<Message> {
    // Fast path: no Frame-tier rules configured → skip the per-frame
    // MatchTarget/context construction entirely.
    if !breakpoint_manager.has_frame_rules().await {
        return Some(msg);
    }
    let context = match direction {
        crate::session::WsDirection::ClientToServer => {
            crate::middleware::plugins::breakpoints::BreakpointContext::Request(Box::new(
                frame_request_context(session_id, uri, host, &direction, &msg),
            ))
        }
        crate::session::WsDirection::ServerToClient => {
            crate::middleware::plugins::breakpoints::BreakpointContext::Response(Box::new(
                frame_response_context(session_id, uri, host, &direction, &msg),
            ))
        }
    };
    match breakpoint_manager
        .pause_frame(sm, session_id, context)
        .await
    {
        crate::middleware::plugins::breakpoints::BreakpointResolution::Continue => Some(msg),
        crate::middleware::plugins::breakpoints::BreakpointResolution::Drop => None,
        crate::middleware::plugins::breakpoints::BreakpointResolution::Modify(ctx) => match *ctx {
            crate::middleware::plugins::breakpoints::BreakpointContext::Request(req) => {
                Some(message_with_body(msg, req.body))
            }
            crate::middleware::plugins::breakpoints::BreakpointContext::Response(res) => {
                Some(message_with_body(msg, res.body))
            }
        },
    }
}

fn frame_request_context(
    session_id: &str,
    uri: &str,
    host: &str,
    direction: &crate::session::WsDirection,
    msg: &Message,
) -> crate::middleware::RequestContext {
    crate::middleware::RequestContext {
        method: "WS".to_string(),
        uri: uri.to_string(),
        headers: frame_headers(direction, msg),
        body: message_body(msg),
        host: host.to_string(),
        session_id: Some(session_id.to_string()),
        protocol_context: Some(crate::core::forward::ProtocolContext::websocket(
            if uri.starts_with("wss://") {
                "wss"
            } else {
                "ws"
            },
        )),
        ..Default::default()
    }
}

fn frame_response_context(
    session_id: &str,
    uri: &str,
    host: &str,
    direction: &crate::session::WsDirection,
    msg: &Message,
) -> crate::middleware::ResponseContext {
    crate::middleware::ResponseContext {
        status: 101,
        headers: frame_headers(direction, msg),
        body: message_body(msg),
        request_uri: uri.to_string(),
        session_id: Some(session_id.to_string()),
        request_host: host.to_string(),
        request_method: "WS".to_string(),
        protocol_context: Some(crate::core::forward::ProtocolContext::websocket(
            if uri.starts_with("wss://") {
                "wss"
            } else {
                "ws"
            },
        )),
        ..Default::default()
    }
}

fn frame_headers(
    direction: &crate::session::WsDirection,
    msg: &Message,
) -> crate::middleware::HeaderMap {
    let mut headers = crate::middleware::HeaderMap::new();
    headers.insert(
        "x-oproxy-frame-direction".to_string(),
        match direction {
            crate::session::WsDirection::ClientToServer => "client_to_server",
            crate::session::WsDirection::ServerToClient => "server_to_client",
        }
        .to_string(),
    );
    headers.insert(
        "x-oproxy-frame-opcode".to_string(),
        map_tungstenite_message_to_record(msg)
            .map(|(opcode, _, _, _)| opcode.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
    );
    headers
}

fn message_body(msg: &Message) -> bytes::Bytes {
    match msg {
        Message::Text(text) => bytes::Bytes::from(text.clone()),
        Message::Binary(data) => bytes::Bytes::copy_from_slice(data),
        _ => bytes::Bytes::new(),
    }
}

fn message_with_body(original: Message, body: bytes::Bytes) -> Message {
    match original {
        Message::Text(_) => Message::Text(String::from_utf8_lossy(&body).into_owned()),
        Message::Binary(_) => Message::Binary(body.to_vec()),
        other => other,
    }
}

fn record_frame(
    sm: &crate::session::SharedSessionManager,
    session_id: &str,
    msg: &Message,
    direction: crate::session::WsDirection,
) {
    if let Some((opcode, payload_len, payload_text, payload_hex)) =
        map_tungstenite_message_to_record(msg)
    {
        sm.append_ws_frame(
            session_id,
            crate::session::WsFrame {
                timestamp: chrono::Utc::now(),
                direction,
                opcode,
                payload_len,
                payload_text,
                payload_hex,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::matcher::{Location, MatchMode};
    use crate::middleware::plugins::mock::MockRule;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[test]
    fn websocket_upgrade_header_is_case_insensitive() {
        let req = Request::builder()
            .header("upgrade", "WebSocket")
            .body(())
            .unwrap();

        assert!(is_websocket_upgrade(&req));
    }

    #[test]
    fn non_websocket_upgrade_not_detected() {
        let req = Request::builder()
            .header("upgrade", "h2c")
            .body(())
            .unwrap();
        assert!(!is_websocket_upgrade(&req));
    }

    #[test]
    fn text_message_maps_to_opcode_1() {
        let msg = Message::Text("hello".to_string());
        let (opcode, len, text, hex) = map_tungstenite_message_to_record(&msg).unwrap();
        assert_eq!(opcode, 0x1);
        assert_eq!(len, 5);
        assert_eq!(text.as_deref(), Some("hello"));
        assert!(hex.is_none());
    }

    #[test]
    fn binary_message_maps_to_opcode_2_with_hex() {
        let msg = Message::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let (opcode, len, text, hex) = map_tungstenite_message_to_record(&msg).unwrap();
        assert_eq!(opcode, 0x2);
        assert_eq!(len, 4);
        assert!(text.is_none());
        assert_eq!(hex.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn close_message_maps_to_opcode_8() {
        let msg = Message::Close(None);
        let (opcode, _, _, _) = map_tungstenite_message_to_record(&msg).unwrap();
        assert_eq!(opcode, 0x8);
    }

    #[test]
    fn large_text_preview_truncated_to_512_chars() {
        let big = "x".repeat(1000);
        let msg = Message::Text(big);
        let (_, len, text, _) = map_tungstenite_message_to_record(&msg).unwrap();
        assert_eq!(len, 1000);
        assert_eq!(text.unwrap().len(), 512);
    }

    #[test]
    fn websocket_response_context_records_completed_upgrade() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            "sec-websocket-key",
            hyper::header::HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
        );

        let response = ws_response_context(
            "ws://echo.test/socket",
            &headers,
            "echo.test",
            Some("conn-1".to_string()),
            Some(7),
            "ws",
            Some("graphql-ws"),
        );
        let metrics = ws_upgrade_metrics();

        assert_eq!(response.status, 101);
        assert_eq!(response.request_uri, "ws://echo.test/socket");
        assert_eq!(response.request_method, "WS");
        assert_eq!(response.protocol.as_deref(), Some("WebSocket"));
        assert_eq!(
            response.headers.get("upgrade"),
            Some(&"websocket".to_string())
        );
        assert!(response.headers.contains_key("sec-websocket-accept"));
        assert_eq!(
            response.headers.get("sec-websocket-protocol"),
            Some(&"graphql-ws".to_string()),
            "negotiated subprotocol must be recorded on the upgrade response"
        );
        assert_eq!(metrics.status_code, 101);
        assert_eq!(metrics.protocol.as_deref(), Some("WebSocket"));

        let protocol = response.protocol_context.expect("protocol context");
        assert!(matches!(
            protocol.downstream,
            crate::core::forward::WireProtocol::WebSocket
        ));
        assert!(matches!(
            protocol.body_mode,
            crate::core::forward::BodyMode::Frames
        ));
        assert_eq!(protocol.connection_id.as_deref(), Some("conn-1"));
        assert_eq!(protocol.stream_id, Some(7));
    }

    fn ws_rule(id: &str, host: &str, frames: Vec<WsFrameAction>) -> MockRule {
        MockRule {
            id: id.to_string(),
            name: id.to_string(),
            enabled: true,
            location: Location {
                host: Some(host.to_string()),
                mode: MatchMode::Glob,
                wire_protocol: Some("websocket".to_string()),
                body_mode: Some("frames".to_string()),
                ..Default::default()
            },
            behavior: Some(MockBehavior::WebSocketScript { frames }),
            responses: Vec::new(),
            call_count: 0,
        }
    }

    #[tokio::test]
    async fn websocket_script_selector_matches_typed_ws_rule_and_increments_count() {
        let rules = Arc::new(RwLock::new(vec![ws_rule(
            "ws-script",
            "echo.test",
            vec![WsFrameAction {
                opcode: 0x1,
                payload: "hello".to_string(),
                delay_ms: 0,
            }],
        )]));
        let headers = hyper::HeaderMap::new();
        let ctx = ws_request_context(
            "ws://echo.test:80/socket",
            &headers,
            "echo.test",
            None,
            None,
            Some("HTTP/1.1".to_string()),
            "ws",
        );

        let script = select_ws_mock_script(&rules, &ctx)
            .await
            .expect("matching ws script");

        assert_eq!(script.rule_id, "ws-script");
        assert_eq!(script.frames.len(), 1);
        assert_eq!(rules.read().await[0].call_count, 1);
    }

    #[tokio::test]
    async fn websocket_script_selector_ignores_legacy_http_mock() {
        let mut rule = ws_rule("legacy", "echo.test", Vec::new());
        rule.behavior = None;
        rule.responses = vec![crate::middleware::plugins::mock::MockResponse {
            status: 200,
            headers: std::collections::HashMap::new(),
            body: "not ws".to_string(),
            delay_ms: 0,
        }];
        let rules = Arc::new(RwLock::new(vec![rule]));
        let ctx = ws_request_context(
            "ws://echo.test:80/socket",
            &hyper::HeaderMap::new(),
            "echo.test",
            None,
            None,
            Some("HTTP/1.1".to_string()),
            "ws",
        );

        assert!(select_ws_mock_script(&rules, &ctx).await.is_none());
    }

    #[test]
    fn upgrade_response_relays_negotiated_subprotocol() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            "sec-websocket-key",
            hyper::header::HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
        );
        let response = websocket_upgrade_response(&headers, Some("graphql-ws"));
        assert_eq!(response.status(), 101);
        assert_eq!(
            response
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|v| v.to_str().ok()),
            Some("graphql-ws")
        );

        let without = websocket_upgrade_response(&headers, None);
        assert!(!without.headers().contains_key("sec-websocket-protocol"));
    }

    #[test]
    fn first_offered_subprotocol_picks_first_token() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            "sec-websocket-protocol",
            hyper::header::HeaderValue::from_static("graphql-ws, mqtt"),
        );
        assert_eq!(
            first_offered_subprotocol(&headers).as_deref(),
            Some("graphql-ws")
        );
        assert_eq!(first_offered_subprotocol(&hyper::HeaderMap::new()), None);
    }

    #[test]
    fn websocket_script_action_maps_to_message() {
        let text = ws_action_to_message(&WsFrameAction {
            opcode: 0x1,
            payload: "hello".to_string(),
            delay_ms: 0,
        });
        assert!(matches!(text, Message::Text(t) if t == "hello"));

        let binary = ws_action_to_message(&WsFrameAction {
            opcode: 0x2,
            payload: "bin".to_string(),
            delay_ms: 0,
        });
        assert!(matches!(binary, Message::Binary(bytes) if bytes == b"bin"));
    }
}
