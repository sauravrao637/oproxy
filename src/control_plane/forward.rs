use axum::{extract::State, response::IntoResponse};
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        protocol::{CloseFrame, Message},
    },
};

use crate::AppState;
use crate::core::engine::is_binary_content_type;
use crate::core::forward::{
    ApplicationProtocol, BodyMode, ProtocolContext, WireProtocol, encode_grpc_frame,
};
use crate::middleware::{RequestContext, ResponseContext};
use crate::security::{AdminEgressPolicy, enforce_admin_egress_policy};
use crate::session::{
    GrpcMessage, InspectionMetrics, SessionEvent, SessionSource, WsDirection, WsFrame,
};

use super::admin_egress_policy_response;

/// How long the Compose WebSocket forwarder waits for the next server frame
/// before considering the exchange complete.
const WS_FORWARD_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
/// Cap on server frames collected by the Compose WebSocket forwarder; keeps a
/// chatty upstream from holding the admin request open indefinitely.
const WS_FORWARD_MAX_SERVER_FRAMES: usize = 10;

#[derive(serde::Deserialize)]
pub(super) struct ForwardReq {
    #[serde(default)]
    pub(super) kind: ForwardKind,
    pub(super) method: String,
    pub(super) url: String,
    #[serde(default)]
    pub(super) headers: HashMap<String, String>,
    #[serde(default)]
    pub(super) body: Option<String>,
    #[serde(default)]
    pub(super) note: Option<String>,
    #[serde(default)]
    pub(super) tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Default, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ForwardKind {
    #[default]
    Http,
    Http2,
    Http3,
    Grpc,
}

#[derive(serde::Serialize)]
pub(super) struct ForwardResp {
    status: u16,
    status_text: String,
    headers: HashMap<String, String>,
    body: String,
    is_binary: bool,
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol: Option<ForwardProtocolResp>,
}

#[derive(Debug)]
pub(super) enum ForwardFailure {
    BadRequest(String),
    EgressBlocked(String),
    Upstream(String),
}

#[derive(serde::Serialize)]
struct ForwardProtocolResp {
    downstream: String,
    upstream: Option<String>,
    application: String,
    body_mode: String,
}

#[derive(serde::Deserialize)]
pub(super) struct ForwardWsReq {
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    frames: Vec<ForwardWsFrameReq>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
struct ForwardWsFrameReq {
    #[serde(default = "default_ws_opcode")]
    opcode: String,
    #[serde(default)]
    payload: String,
}

fn default_ws_opcode() -> String {
    "text".to_string()
}

#[derive(serde::Serialize)]
pub(super) struct ForwardWsResp {
    status: u16,
    status_text: String,
    session_id: String,
    frames: Vec<ForwardWsFrameResp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol: Option<ForwardProtocolResp>,
}

/// Why a WebSocket forward failed, so each caller (axum handler, assistant
/// executor) can map it to its own response shape.
#[derive(Debug)]
pub(super) enum ForwardWsFailure {
    /// Malformed request (bad URL/scheme). No session was recorded.
    BadRequest(String),
    /// Blocked by the admin egress policy. No session was recorded.
    EgressBlocked(String),
    /// Connection/upstream failure; a 502 response has already been recorded
    /// onto the session.
    Upstream { session_id: String, error: String },
}

#[derive(serde::Serialize)]
struct ForwardWsFrameResp {
    direction: String,
    opcode: String,
    payload: Option<String>,
    payload_len: usize,
}

struct ForwardFrameCollector<'a> {
    state: &'a Arc<AppState>,
    session_id: &'a str,
    frames: Vec<ForwardWsFrameResp>,
    server_frames: usize,
}

struct PreparedWebSocketForward {
    request: tokio_tungstenite::tungstenite::http::Request<()>,
    session_id: String,
    url: String,
    frames: Vec<ForwardWsFrameReq>,
    protocol: ProtocolContext,
}

impl<'a> ForwardFrameCollector<'a> {
    fn new(state: &'a Arc<AppState>, session_id: &'a str) -> Self {
        Self {
            state,
            session_id,
            frames: Vec::new(),
            server_frames: 0,
        }
    }

    fn record(&mut self, direction: WsDirection, message: &Message) {
        if direction == WsDirection::ServerToClient {
            self.server_frames += 1;
        }
        record_ws_message(
            self.state,
            self.session_id,
            direction,
            message,
            &mut self.frames,
        );
    }

    fn reached_server_limit(&self) -> bool {
        self.server_frames >= WS_FORWARD_MAX_SERVER_FRAMES
    }
}

pub(super) async fn forward_request(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ForwardReq>,
) -> impl IntoResponse {
    match forward_http_exchange(&state, req).await {
        Ok(response) => axum::Json(response).into_response(),
        Err(ForwardFailure::BadRequest(error)) => (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
        Err(ForwardFailure::EgressBlocked(error)) => admin_egress_policy_response(error),
        Err(ForwardFailure::Upstream(error)) => {
            (axum::http::StatusCode::BAD_GATEWAY, error).into_response()
        }
    }
}

pub(super) async fn forward_http_exchange(
    state: &Arc<AppState>,
    req: ForwardReq,
) -> Result<ForwardResp, ForwardFailure> {
    let method = match reqwest::Method::from_bytes(req.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return Err(ForwardFailure::BadRequest(format!(
                "Invalid HTTP method: {}",
                req.method
            )));
        }
    };
    let url_parsed = match reqwest::Url::parse(&req.url) {
        Ok(u) => u,
        Err(e) => {
            return Err(ForwardFailure::BadRequest(format!("Invalid URL: {e}")));
        }
    };
    if !matches!(url_parsed.scheme(), "http" | "https") {
        return Err(ForwardFailure::BadRequest(format!(
            "Unsupported URL scheme: {}",
            url_parsed.scheme()
        )));
    }
    if let Err(e) =
        enforce_admin_egress_policy(&url_parsed, AdminEgressPolicy::from_config(&state.config))
            .await
    {
        return Err(ForwardFailure::EgressBlocked(e));
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let is_grpc = req.kind == ForwardKind::Grpc;
    let request_body = req.body.clone().unwrap_or_default();
    let body_bytes = if is_grpc {
        encode_grpc_frame(false, request_body.as_bytes())
    } else {
        request_body.into_bytes()
    };
    let host = match (url_parsed.host_str(), url_parsed.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => String::new(),
    };
    let display_uri = req.url.clone();

    // Record request in session manager
    let protocol_context = protocol_context_for_kind(req.kind, url_parsed.scheme(), None);
    let req_ctx = RequestContext {
        method: if is_grpc {
            "POST".to_string()
        } else {
            req.method.clone()
        },
        uri: display_uri.clone(),
        host: host.clone(),
        headers: req.headers.clone().into(),
        body: bytes::Bytes::from(body_bytes.clone()),
        downstream_protocol: Some(protocol_context.downstream.label().to_string()),
        protocol_context: Some(protocol_context),
        ..Default::default()
    };
    let request_size_bytes = req_ctx.body.len();
    state
        .api_handler
        .session_manager
        .record_request_with_source(session_id.clone(), req_ctx, SessionSource::AdminForward);
    if req.note.is_some() || req.tags.is_some() {
        state
            .api_handler
            .session_manager
            .annotate(&session_id, req.note.clone(), req.tags.clone())
            .await;
    }
    if is_grpc {
        state.api_handler.session_manager.append_event(
            &session_id,
            SessionEvent::GrpcMessage {
                direction: "request".to_string(),
                message: grpc_message("request", false, body_bytes.len().saturating_sub(5) as u32),
            },
        );
    }

    // Build and send request using the proxy engine's http client
    let method = if is_grpc {
        reqwest::Method::POST
    } else {
        method
    };
    let mut builder = state
        .proxy_engine
        .http_client()
        .await
        .request(method, &req.url);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    if is_grpc
        && !req
            .headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("content-type"))
    {
        builder = builder.header("content-type", "application/grpc+proto");
    }
    if is_grpc || !body_bytes.is_empty() {
        builder = builder.body(body_bytes.clone());
    }

    let t0 = std::time::Instant::now();
    match builder.send().await {
        Ok(res) => {
            let ttfb_ms = t0.elapsed().as_millis() as u64;
            let status = res.status().as_u16();
            let upstream_protocol = crate::core::engine::protocol_label(res.version()).to_string();
            let mut res_headers: HashMap<String, String> = HashMap::new();
            for (k, v) in res.headers() {
                res_headers.insert(k.to_string(), v.to_str().unwrap_or("").to_string());
            }
            let content_type = res_headers.get("content-type").cloned().unwrap_or_default();
            let bytes = res.bytes().await.unwrap_or_default();
            if is_grpc {
                for (compressed, payload_len) in decode_grpc_message_lengths(&bytes) {
                    state.api_handler.session_manager.append_event(
                        &session_id,
                        SessionEvent::GrpcMessage {
                            direction: "response".to_string(),
                            message: grpc_message("response", compressed, payload_len),
                        },
                    );
                }
            }
            let body_ms = t0.elapsed().as_millis() as u64 - ttfb_ms;
            let (body, is_binary) = if is_binary_content_type(&content_type) {
                (
                    base64::engine::general_purpose::STANDARD.encode(&bytes),
                    true,
                )
            } else {
                (String::from_utf8_lossy(&bytes).to_string(), false)
            };

            // Record response (one protocol context, shared by the recording and
            // the API response below).
            let final_protocol = protocol_context_for_kind(
                req.kind,
                url_parsed.scheme(),
                Some(protocol_from_label(&upstream_protocol)),
            );
            let res_ctx = ResponseContext {
                status,
                headers: res_headers.clone().into(),
                body: bytes.clone(),
                request_uri: display_uri,
                session_id: Some(session_id.clone()),
                ttfb_ms,
                body_ms,
                protocol: Some(upstream_protocol.clone()),
                protocol_context: Some(final_protocol.clone()),
                ..Default::default()
            };
            let metrics = crate::session::InspectionMetrics {
                latency_ms: t0.elapsed().as_millis() as u64,
                request_size_bytes,
                response_size_bytes: bytes.len(),
                status_code: status,
                ttfb_ms,
                body_ms,
                protocol: Some(upstream_protocol.clone()),
                ..Default::default()
            };
            state
                .api_handler
                .session_manager
                .record_response_with_metrics(session_id.clone(), res_ctx, metrics);

            let status_text = reqwest::StatusCode::from_u16(status)
                .ok()
                .and_then(|s| s.canonical_reason())
                .unwrap_or("")
                .to_string();
            Ok(ForwardResp {
                status,
                status_text,
                headers: res_headers,
                body,
                is_binary,
                session_id,
                protocol: Some(protocol_response(final_protocol)),
            })
        }
        Err(e) => {
            let res_ctx = ResponseContext {
                status: 502,
                body: bytes::Bytes::from(e.to_string()),
                request_uri: display_uri,
                session_id: Some(session_id.clone()),
                protocol_context: Some(protocol_context_for_kind(
                    req.kind,
                    url_parsed.scheme(),
                    None,
                )),
                ..Default::default()
            };
            let metrics = crate::session::InspectionMetrics {
                latency_ms: t0.elapsed().as_millis() as u64,
                request_size_bytes,
                response_size_bytes: e.to_string().len(),
                status_code: 502,
                ttfb_ms: t0.elapsed().as_millis() as u64,
                body_ms: 0,
                ..Default::default()
            };
            state
                .api_handler
                .session_manager
                .record_response_with_metrics(session_id.clone(), res_ctx, metrics);
            Err(ForwardFailure::Upstream(e.to_string()))
        }
    }
}

pub(super) async fn forward_websocket(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ForwardWsReq>,
) -> impl IntoResponse {
    match forward_websocket_exchange(&state, req).await {
        Ok(resp) => axum::Json(resp).into_response(),
        Err(ForwardWsFailure::BadRequest(error)) => (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
        Err(ForwardWsFailure::EgressBlocked(error)) => admin_egress_policy_response(error),
        Err(ForwardWsFailure::Upstream { session_id, error }) => (
            axum::http::StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({ "error": error, "session_id": session_id })),
        )
            .into_response(),
    }
}

/// Connects to an upstream WebSocket, sends the scripted frames, collects up to
/// [`WS_FORWARD_MAX_SERVER_FRAMES`] replies, and records everything as an
/// admin-forward session. Shared by the `/admin/forward/websocket` handler and
/// the assistant action executor so both paths behave identically.
pub(super) async fn forward_websocket_exchange(
    state: &Arc<AppState>,
    req: ForwardWsReq,
) -> Result<ForwardWsResp, ForwardWsFailure> {
    let prepared = prepare_websocket_forward(state, req).await?;
    let PreparedWebSocketForward {
        request,
        session_id,
        url,
        frames,
        protocol,
    } = prepared;
    let started = std::time::Instant::now();
    let mut ws = connect_async(request)
        .await
        .map_err(|error| {
            record_ws_failure(state, session_id.clone(), &url, started, error.to_string())
        })?
        .0;

    let mut collector = ForwardFrameCollector::new(state, &session_id);
    for frame in frames {
        let message = websocket_message(frame);
        collector.record(WsDirection::ClientToServer, &message);
        if let Err(error) = ws.send(message).await {
            return Err(record_ws_failure(
                state,
                session_id,
                &url,
                started,
                error.to_string(),
            ));
        }
    }

    collect_websocket_replies(state, &session_id, &url, started, &mut ws, &mut collector).await?;
    let _ = ws.close(None).await;
    record_websocket_success(state, &session_id, &url, started, &protocol);
    let collected_frames = std::mem::take(&mut collector.frames);
    drop(collector);

    Ok(ForwardWsResp {
        status: 101,
        status_text: "Switching Protocols".to_string(),
        session_id,
        frames: collected_frames,
        protocol: Some(protocol_response(protocol)),
    })
}

async fn prepare_websocket_forward(
    state: &Arc<AppState>,
    req: ForwardWsReq,
) -> Result<PreparedWebSocketForward, ForwardWsFailure> {
    let url = reqwest::Url::parse(&req.url)
        .map_err(|e| ForwardWsFailure::BadRequest(format!("Invalid URL: {e}")))?;
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err(ForwardWsFailure::BadRequest(format!(
            "Unsupported WebSocket URL scheme: {}",
            url.scheme()
        )));
    }
    let policy_url = ws_url_to_http(&url).map_err(ForwardWsFailure::BadRequest)?;
    enforce_admin_egress_policy(&policy_url, AdminEgressPolicy::from_config(&state.config))
        .await
        .map_err(ForwardWsFailure::EgressBlocked)?;

    let session_id = uuid::Uuid::new_v4().to_string();
    let host = match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => String::new(),
    };
    let mut headers = req.headers.clone();
    headers.insert("upgrade".to_string(), "websocket".to_string());
    let proto = ProtocolContext {
        upstream: Some(WireProtocol::WebSocket),
        ..ProtocolContext::websocket(url.scheme()).with_identity(Some(session_id.clone()), Some(1))
    };
    let req_ctx = RequestContext {
        method: "WS".to_string(),
        uri: req.url.clone(),
        host,
        headers: headers.clone().into(),
        body: bytes::Bytes::new(),
        connection_id: Some(session_id.clone()),
        stream_id: Some(1),
        downstream_protocol: Some("WebSocket".to_string()),
        protocol_context: Some(proto.clone()),
        ..Default::default()
    };
    state
        .api_handler
        .session_manager
        .record_request_with_source(session_id.clone(), req_ctx, SessionSource::AdminForward);
    if req.note.is_some() || req.tags.is_some() {
        state
            .api_handler
            .session_manager
            .annotate(&session_id, req.note.clone(), req.tags.clone())
            .await;
    }

    let mut request = match req.url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => {
            return Err(ForwardWsFailure::BadRequest(e.to_string()));
        }
    };
    for (name, value) in req.headers {
        if is_ws_forward_header(&name)
            && let (Ok(name), Ok(value)) = (
                axum::http::HeaderName::from_bytes(name.as_bytes()),
                axum::http::HeaderValue::from_str(&value),
            )
        {
            request.headers_mut().insert(name, value);
        }
    }

    Ok(PreparedWebSocketForward {
        request,
        session_id,
        url: req.url,
        frames: req.frames,
        protocol: proto,
    })
}

fn websocket_message(frame: ForwardWsFrameReq) -> Message {
    match frame.opcode.as_str() {
        "binary" => Message::Binary(
            base64::engine::general_purpose::STANDARD
                .decode(frame.payload.as_bytes())
                .unwrap_or_else(|_| frame.payload.into_bytes()),
        ),
        "ping" => Message::Ping(frame.payload.into_bytes()),
        "pong" => Message::Pong(frame.payload.into_bytes()),
        "close" => Message::Close(Some(CloseFrame {
            code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
            reason: frame.payload.into(),
        })),
        _ => Message::Text(frame.payload),
    }
}

async fn collect_websocket_replies(
    state: &Arc<AppState>,
    session_id: &str,
    url: &str,
    started: std::time::Instant,
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    collector: &mut ForwardFrameCollector<'_>,
) -> Result<(), ForwardWsFailure> {
    while let Ok(Some(next)) = tokio::time::timeout(WS_FORWARD_REPLY_TIMEOUT, ws.next()).await {
        match next {
            Ok(message) => {
                collector.record(WsDirection::ServerToClient, &message);
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
            Err(e) => {
                return Err(record_ws_failure(
                    state,
                    session_id.to_string(),
                    url,
                    started,
                    e.to_string(),
                ));
            }
        }
        if collector.reached_server_limit() {
            break;
        }
    }
    Ok(())
}

fn record_websocket_success(
    state: &Arc<AppState>,
    session_id: &str,
    url: &str,
    started: std::time::Instant,
    protocol: &ProtocolContext,
) {
    let response = ResponseContext {
        status: 101,
        headers: HashMap::from([("upgrade".to_string(), "websocket".to_string())]).into(),
        request_uri: url.to_string(),
        session_id: Some(session_id.to_string()),
        protocol: Some("WebSocket".to_string()),
        protocol_context: Some(protocol.clone()),
        ..Default::default()
    };
    state
        .api_handler
        .session_manager
        .record_response_with_metrics(
            session_id.to_string(),
            response,
            InspectionMetrics {
                latency_ms: started.elapsed().as_millis() as u64,
                status_code: 101,
                protocol: Some("WebSocket".to_string()),
                ..Default::default()
            },
        );
}

fn record_ws_failure(
    state: &Arc<AppState>,
    session_id: String,
    request_uri: &str,
    started: std::time::Instant,
    error: String,
) -> ForwardWsFailure {
    let response = ResponseContext {
        status: 502,
        body: bytes::Bytes::from(error.clone()),
        request_uri: request_uri.to_string(),
        session_id: Some(session_id.clone()),
        ..Default::default()
    };
    state
        .api_handler
        .session_manager
        .record_response_with_metrics(
            session_id.clone(),
            response,
            InspectionMetrics {
                latency_ms: started.elapsed().as_millis() as u64,
                status_code: 502,
                response_size_bytes: error.len(),
                ..Default::default()
            },
        );
    ForwardWsFailure::Upstream { session_id, error }
}

fn protocol_context_for_kind(
    kind: ForwardKind,
    scheme: &str,
    upstream: Option<WireProtocol>,
) -> ProtocolContext {
    match kind {
        ForwardKind::Http => ProtocolContext {
            downstream: WireProtocol::Http1,
            upstream,
            application: ApplicationProtocol::Http,
            body_mode: BodyMode::Full,
            scheme: scheme.to_string(),
            connection_id: None,
            stream_id: None,
        },
        ForwardKind::Http2 => ProtocolContext {
            downstream: WireProtocol::Http2,
            upstream,
            application: ApplicationProtocol::Http,
            body_mode: BodyMode::Full,
            scheme: scheme.to_string(),
            connection_id: None,
            stream_id: None,
        },
        ForwardKind::Http3 => ProtocolContext {
            downstream: WireProtocol::Http3,
            upstream,
            application: ApplicationProtocol::Http,
            body_mode: BodyMode::Full,
            scheme: scheme.to_string(),
            connection_id: None,
            stream_id: None,
        },
        ForwardKind::Grpc => ProtocolContext {
            downstream: WireProtocol::Http2,
            upstream,
            application: ApplicationProtocol::Grpc,
            body_mode: BodyMode::StreamMessages,
            scheme: scheme.to_string(),
            connection_id: None,
            stream_id: None,
        },
    }
}

fn protocol_from_label(label: &str) -> WireProtocol {
    match label {
        "HTTP/2" => WireProtocol::Http2,
        "HTTP/3" => WireProtocol::Http3,
        _ => WireProtocol::Http1,
    }
}

fn protocol_response(ctx: ProtocolContext) -> ForwardProtocolResp {
    ForwardProtocolResp {
        downstream: ctx.downstream.label().to_string(),
        upstream: ctx.upstream.map(|p| p.label().to_string()),
        application: ctx.application.match_value().to_string(),
        body_mode: ctx.body_mode.match_value().to_string(),
    }
}

fn decode_grpc_message_lengths(bytes: &[u8]) -> Vec<(bool, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 5 <= bytes.len() {
        let compressed = bytes[i] != 0;
        let len = u32::from_be_bytes([bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]]);
        i += 5;
        let end = i.saturating_add(len as usize);
        if end > bytes.len() {
            break;
        }
        out.push((compressed, len));
        i = end;
    }
    out
}

fn grpc_message(direction: &str, compressed: bool, length: u32) -> GrpcMessage {
    GrpcMessage {
        direction: direction.to_string(),
        compressed,
        length,
        fields: Vec::new(),
    }
}

fn ws_url_to_http(url: &reqwest::Url) -> Result<reqwest::Url, String> {
    let mut out = url.clone();
    let scheme = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        other => return Err(format!("Unsupported WebSocket URL scheme: {other}")),
    };
    out.set_scheme(scheme)
        .map_err(|_| format!("Unable to translate {} URL for egress policy", url.scheme()))?;
    Ok(out)
}

fn is_ws_forward_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "connection"
            | "upgrade"
            | "sec-websocket-key"
            | "sec-websocket-version"
            | "sec-websocket-accept"
            | "content-length"
    )
}

fn record_ws_message(
    state: &Arc<AppState>,
    session_id: &str,
    direction: WsDirection,
    message: &Message,
    frames: &mut Vec<ForwardWsFrameResp>,
) {
    let (opcode, payload_len, payload_text, payload_hex, opcode_label) = match message {
        Message::Text(text) => (
            0x1,
            text.len(),
            Some(text.chars().take(512).collect::<String>()),
            None,
            "text",
        ),
        Message::Binary(data) => {
            let hex = data
                .iter()
                .take(64)
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            (
                0x2,
                data.len(),
                None,
                if hex.is_empty() { None } else { Some(hex) },
                "binary",
            )
        }
        Message::Ping(data) => (0x9, data.len(), None, None, "ping"),
        Message::Pong(data) => (0xA, data.len(), None, None, "pong"),
        Message::Close(_) => (0x8, 0, None, None, "close"),
        Message::Frame(_) => return,
    };
    let frame = WsFrame {
        timestamp: chrono::Utc::now(),
        direction: direction.clone(),
        opcode,
        payload_len,
        payload_text: payload_text.clone(),
        payload_hex: payload_hex.clone(),
    };
    state
        .api_handler
        .session_manager
        .append_ws_frame(session_id, frame);
    frames.push(ForwardWsFrameResp {
        direction: match direction {
            WsDirection::ClientToServer => "client".to_string(),
            WsDirection::ServerToClient => "server".to_string(),
        },
        opcode: opcode_label.to_string(),
        payload: payload_text.or(payload_hex),
        payload_len,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_frame_round_trips_message_lengths() {
        let mut body = encode_grpc_frame(false, b"hello");
        body.extend_from_slice(&encode_grpc_frame(true, b"world!"));

        assert_eq!(
            decode_grpc_message_lengths(&body),
            vec![(false, 5), (true, 6)]
        );
    }

    #[test]
    fn grpc_frame_decoder_ignores_truncated_tail() {
        let mut body = encode_grpc_frame(false, b"ok");
        body.extend_from_slice(&[0, 0, 0, 0, 8, b'p']);

        assert_eq!(decode_grpc_message_lengths(&body), vec![(false, 2)]);
    }

    #[test]
    fn grpc_message_carries_direction_in_event_payload() {
        let message = grpc_message("response", true, 42);

        assert_eq!(message.direction, "response");
        assert!(message.compressed);
        assert_eq!(message.length, 42);
    }

    #[test]
    fn websocket_policy_url_translates_ws_schemes() {
        let ws = reqwest::Url::parse("ws://example.test/socket").unwrap();
        let wss = reqwest::Url::parse("wss://example.test/socket").unwrap();

        assert_eq!(
            ws_url_to_http(&ws).unwrap().as_str(),
            "http://example.test/socket"
        );
        assert_eq!(
            ws_url_to_http(&wss).unwrap().as_str(),
            "https://example.test/socket"
        );
    }

    #[test]
    fn websocket_forward_header_filter_blocks_handshake_headers() {
        assert!(!is_ws_forward_header("Connection"));
        assert!(!is_ws_forward_header("sec-websocket-key"));
        assert!(!is_ws_forward_header("content-length"));
        assert!(is_ws_forward_header("authorization"));
        assert!(is_ws_forward_header("x-trace-id"));
    }

    #[test]
    fn grpc_protocol_response_uses_stream_message_body_mode() {
        let response = protocol_response(protocol_context_for_kind(
            ForwardKind::Grpc,
            "https",
            Some(WireProtocol::Http2),
        ));

        assert_eq!(response.downstream, "HTTP/2");
        assert_eq!(response.upstream.as_deref(), Some("HTTP/2"));
        assert_eq!(response.application, "grpc");
        assert_eq!(response.body_mode, "stream_messages");
    }
}
