use crate::middleware::{RequestContext, ResponseContext};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tokio::sync::{broadcast, mpsc, oneshot};

/// Bound on the in-flight session write queue. Generous enough that normal
/// operation never blocks or drops; only pathological overload (writer task
/// starved) hits the limit, where dropping a record is preferable to unbounded
/// memory growth.
const WRITE_QUEUE_CAPACITY: usize = 8192;

fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| {
        tracing::error!("session store read lock was poisoned; recovering protected state");
        poisoned.into_inner()
    })
}

fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poisoned| {
        tracing::error!("session store write lock was poisoned; recovering protected state");
        poisoned.into_inner()
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WsDirection {
    ClientToServer,
    ServerToClient,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsFrame {
    pub timestamp: DateTime<Utc>,
    pub direction: WsDirection,
    pub opcode: u8,
    pub payload_len: usize,
    pub payload_text: Option<String>,
    pub payload_hex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InspectionMetrics {
    pub latency_ms: u64,
    pub request_size_bytes: usize,
    pub response_size_bytes: usize,
    pub status_code: u16,
    #[serde(default)]
    pub ttfb_ms: u64,
    #[serde(default)]
    pub body_ms: u64,
    /// DNS resolution time in milliseconds (None when already resolved / not measured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_ms: Option<u64>,
    /// TCP connect handshake time in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_connect_ms: Option<u64>,
    /// TLS handshake time in milliseconds (None for plain HTTP connections).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_ms: Option<u64>,
    /// Negotiated upstream HTTP protocol (e.g. "HTTP/1.1", "HTTP/2", "HTTP/3").
    /// None when not captured (e.g. CONNECT tunnels, synthetic error responses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    #[default]
    Proxy,
    AdminForward,
    Playback,
    Imported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLInfo {
    pub operation_type: String,
    pub operation_name: Option<String>,
    pub variables: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtInfo {
    pub header: serde_json::Value,
    pub claims: serde_json::Value,
    pub expired: bool,
    pub alg_none_warning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcField {
    pub field_number: u32,
    pub wire_type: u8,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcMessage {
    pub direction: String,
    pub compressed: bool,
    pub length: u32,
    pub fields: Vec<GrpcField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    RequestBodyChunk {
        bytes: usize,
        at_ms: u64,
    },
    ResponseBodyChunk {
        bytes: usize,
        at_ms: u64,
    },
    GrpcMessage {
        direction: String,
        message: GrpcMessage,
    },
    WsFrame {
        frame: WsFrame,
    },
    Trailers {
        headers: crate::middleware::HeaderMap,
    },
    BreakpointPaused {
        breakpoint_id: String,
    },
    MockServed {
        rule_id: String,
        behavior: String,
    },
    TunnelOpened,
    TunnelClosed {
        bytes_up: u64,
        bytes_down: u64,
    },
}

impl SessionEvent {
    fn retained_body_size(&self) -> usize {
        match self {
            SessionEvent::WsFrame { frame } => {
                frame.payload_text.as_ref().map_or(0, String::len)
                    + frame.payload_hex.as_ref().map_or(0, String::len)
            }
            _ => 0,
        }
    }

    fn clear_retained_body(&mut self) {
        if let SessionEvent::WsFrame { frame } = self {
            frame.payload_text = None;
            frame.payload_hex = None;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcInfo {
    pub service: Option<String>,
    pub method: Option<String>,
    pub messages: Vec<GrpcMessage>,
    /// gRPC status code string from `grpc-status` (best-effort; present when the
    /// server sends it in response headers or the streaming path captures it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc_status: Option<String>,
    /// Human-readable status detail from `grpc-message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc_status_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InspectorData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql: Option<GraphQLInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwt: Option<JwtInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc: Option<GrpcInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exchange {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    pub request: RequestContext,
    pub response: Option<ResponseContext>,
    pub metrics: Option<InspectionMetrics>,
    #[serde(default)]
    pub source: SessionSource,
    #[serde(default)]
    pub ws_frames: Vec<WsFrame>,
    /// Append-only protocol event stream. Compatibility fields above remain as
    /// read models while UI/API consumers migrate to this unified path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<SessionEvent>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspector_data: Option<InspectorData>,
    /// Timestamp when the request was paused at a breakpoint; None if not paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_at: Option<DateTime<Utc>>,
    /// Identity of the downstream connection this exchange arrived on. All
    /// exchanges multiplexed over one HTTP/2 or HTTP/3 connection share this id;
    /// for HTTP/1.1 it is one connection per (reused) socket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    /// Monotonic stream index within `connection_id`. For HTTP/1.1 this counts
    /// sequential requests on the socket; for h2/h3 it orders the multiplexed
    /// streams. None when not captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<u64>,
    /// Protocol negotiated on the downstream (client→proxy) side, e.g. "HTTP/2".
    /// Distinct from `metrics.protocol`, which is the upstream (proxy→origin) leg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downstream_protocol: Option<String>,
    /// Typed protocol identity for protocol-aware rules and UI read models. Kept
    /// optional so old saved sessions/imports remain valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_context: Option<crate::core::forward::ProtocolContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionChangeKind {
    RequestCaptured,
    ResponseCaptured,
    SessionUpdated,
    SessionsImported,
    SessionsCleared,
    WsFrameCaptured,
    SessionPaused,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub kind: SessionChangeKind,
}

// ── Write operations routed through the single writer task ────────────────────

enum WriteOp {
    RecordRequest {
        id: String,
        request: Box<RequestContext>,
        source: SessionSource,
    },
    RecordResponse {
        id: String,
        response: ResponseContext,
    },
    RecordResponseWithMetrics {
        id: String,
        response: ResponseContext,
        metrics: InspectionMetrics,
    },
    AppendWsFrame {
        id: String,
        frame: WsFrame,
    },
    AppendEvent {
        id: String,
        event: SessionEvent,
    },
    Annotate {
        id: String,
        note: Option<String>,
        tags: Option<Vec<String>>,
        reply: oneshot::Sender<bool>,
    },
    ImportSessions {
        exchanges: Vec<Exchange>,
    },
    ClearSessions,
    UpdateInspectorData {
        id: String,
        data: InspectorData,
    },
    MarkPaused {
        id: String,
    },
    ClearPaused {
        id: String,
    },
    /// Replace the entire store with a pre-parsed map (used by load_from_file).
    LoadData {
        map: IndexMap<String, Exchange>,
        reply: oneshot::Sender<()>,
    },
    /// Drain all preceding ops; reply signals completion.
    Flush(oneshot::Sender<()>),
}

// ── SessionManager ────────────────────────────────────────────────────────────

pub struct SessionManager {
    /// Shared with the writer task; only the writer task acquires write locks.
    exchanges: Arc<RwLock<IndexMap<String, Exchange>>>,
    change_tx: broadcast::Sender<SessionChange>,
    write_tx: mpsc::Sender<WriteOp>,
    dropped_writes: Arc<AtomicU64>,
}

impl SessionManager {
    #[cfg(test)]
    pub fn new(max_sessions: usize) -> Self {
        Self::with_body_budget(max_sessions, usize::MAX)
    }

    pub fn with_body_budget(max_sessions: usize, max_retained_body_bytes: usize) -> Self {
        let (change_tx, _) = broadcast::channel(64);
        let exchanges = Arc::new(RwLock::new(IndexMap::new()));
        let (write_tx, write_rx) = mpsc::channel(WRITE_QUEUE_CAPACITY);

        tokio::spawn(writer_task(
            write_rx,
            Arc::clone(&exchanges),
            max_sessions,
            max_retained_body_bytes,
            change_tx.clone(),
        ));

        Self {
            exchanges,
            change_tx,
            write_tx,
            dropped_writes: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Fire-and-forget enqueue. Drops (and counts) the op if the writer queue is
    /// saturated, trading record loss for bounded memory under overload.
    fn enqueue(&self, op: WriteOp) {
        if let Err(e) = self.write_tx.try_send(op) {
            let kind = match e {
                mpsc::error::TrySendError::Full(_) => "queue full",
                mpsc::error::TrySendError::Closed(_) => "writer closed",
            };
            let total = self.dropped_writes.fetch_add(1, Ordering::Relaxed) + 1;
            if total.is_power_of_two() {
                tracing::warn!(
                    reason = kind,
                    dropped_total = total,
                    "dropping session write op; writer task is not keeping up"
                );
            }
        }
    }

    /// Returns a broadcast receiver that fires on every session change.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionChange> {
        self.change_tx.subscribe()
    }

    /// Wait until all previously sent write ops have been processed.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        let _ = self.write_tx.send(WriteOp::Flush(tx)).await;
        let _ = rx.await;
    }

    // ── Fire-and-forget write operations ──────────────────────────────────────

    pub fn record_request(&self, id: String, request: RequestContext) {
        self.record_request_with_source(id, request, SessionSource::Proxy);
    }

    pub fn record_request_with_source(
        &self,
        id: String,
        request: RequestContext,
        source: SessionSource,
    ) {
        self.enqueue(WriteOp::RecordRequest {
            id,
            request: Box::new(request),
            source,
        });
    }

    pub fn record_response(&self, id: String, response: ResponseContext) {
        self.enqueue(WriteOp::RecordResponse { id, response });
    }

    pub fn record_response_with_metrics(
        &self,
        id: String,
        response: ResponseContext,
        metrics: InspectionMetrics,
    ) {
        self.enqueue(WriteOp::RecordResponseWithMetrics {
            id,
            response,
            metrics,
        });
    }

    pub fn import_sessions(&self, exchanges: Vec<Exchange>) {
        self.enqueue(WriteOp::ImportSessions { exchanges });
    }

    pub fn append_ws_frame(&self, id: &str, frame: WsFrame) {
        self.enqueue(WriteOp::AppendWsFrame {
            id: id.to_string(),
            frame,
        });
    }

    pub fn append_event(&self, id: &str, event: SessionEvent) {
        self.enqueue(WriteOp::AppendEvent {
            id: id.to_string(),
            event,
        });
    }

    pub fn clear_sessions(&self) {
        self.enqueue(WriteOp::ClearSessions);
    }

    pub fn update_inspector_data(&self, id: &str, data: InspectorData) {
        self.enqueue(WriteOp::UpdateInspectorData {
            id: id.to_string(),
            data,
        });
    }

    /// Mark a session as paused at a breakpoint. Records the pause timestamp.
    pub fn mark_paused(&self, id: &str) {
        self.enqueue(WriteOp::MarkPaused { id: id.to_string() });
    }

    /// Clear the paused state from a session (called when breakpoint is resolved).
    pub fn clear_paused(&self, id: &str) {
        self.enqueue(WriteOp::ClearPaused { id: id.to_string() });
    }

    // ── Write operations that need a reply ────────────────────────────────────

    /// Update the note and/or tags on an existing session.
    /// `note: Some(x)` replaces the note; `None` leaves it unchanged.
    /// `tags: Some(v)` replaces the tag list; `None` leaves it unchanged.
    /// Returns `false` if no session with `id` exists.
    pub async fn annotate(
        &self,
        id: &str,
        note: Option<String>,
        tags: Option<Vec<String>>,
    ) -> bool {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .write_tx
            .send(WriteOp::Annotate {
                id: id.to_string(),
                note,
                tags,
                reply: tx,
            })
            .await;
        rx.await.unwrap_or(false)
    }

    // ── Async file I/O ────────────────────────────────────────────────────────

    pub async fn save_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        // Flush pending writes before taking the read snapshot.
        self.flush().await;
        let json = {
            let guard = read_lock(&self.exchanges);
            serde_json::to_string_pretty(&*guard)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
        };
        tokio::fs::write(path, json).await
    }

    pub async fn load_from_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let data = tokio::fs::read(path).await?;
        let map: IndexMap<String, Exchange> = serde_json::from_slice(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let (tx, rx) = oneshot::channel();
        let _ = self
            .write_tx
            .send(WriteOp::LoadData { map, reply: tx })
            .await;
        rx.await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "writer task closed"))
    }

    // ── Read operations (acquire read lock directly) ───────────────────────────

    pub fn get_all_sessions(&self) -> Vec<Exchange> {
        let exchanges = read_lock(&self.exchanges);
        exchanges.values().cloned().collect()
    }

    pub fn get_session(&self, id: &str) -> Option<Exchange> {
        let exchanges = read_lock(&self.exchanges);
        exchanges.get(id).cloned()
    }

    fn exchange_body_size(exchange: &Exchange) -> usize {
        let request_bytes = exchange.request.body.len();
        let response_bytes = exchange.response.as_ref().map_or(0, |r| r.body.len());
        let ws_bytes = exchange
            .ws_frames
            .iter()
            .map(|f| {
                f.payload_text.as_ref().map_or(0, String::len)
                    + f.payload_hex.as_ref().map_or(0, String::len)
            })
            .sum::<usize>();
        let event_bytes = exchange
            .events
            .iter()
            .map(SessionEvent::retained_body_size)
            .sum::<usize>();
        request_bytes + response_bytes + ws_bytes + event_bytes
    }

    fn clear_exchange_bodies(exchange: &mut Exchange) {
        exchange.request.body = bytes::Bytes::new();
        if let Some(response) = &mut exchange.response {
            response.body = bytes::Bytes::new();
        }
        for frame in &mut exchange.ws_frames {
            frame.payload_text = None;
            frame.payload_hex = None;
        }
        for event in &mut exchange.events {
            event.clear_retained_body();
        }
    }
}

#[async_trait]
pub trait ExchangeRecorder: Send + Sync {
    fn subscribe(&self) -> broadcast::Receiver<SessionChange>;
    async fn flush(&self);
    fn record_request(&self, id: String, request: RequestContext);
    fn record_request_with_source(
        &self,
        id: String,
        request: RequestContext,
        source: SessionSource,
    );
    fn record_response(&self, id: String, response: ResponseContext);
    fn record_response_with_metrics(
        &self,
        id: String,
        response: ResponseContext,
        metrics: InspectionMetrics,
    );
    fn import_sessions(&self, exchanges: Vec<Exchange>);
    fn append_ws_frame(&self, id: &str, frame: WsFrame);
    fn append_event(&self, id: &str, event: SessionEvent);
    fn clear_sessions(&self);
    fn update_inspector_data(&self, id: &str, data: InspectorData);
    fn mark_paused(&self, id: &str);
    fn clear_paused(&self, id: &str);
    async fn annotate(&self, id: &str, note: Option<String>, tags: Option<Vec<String>>) -> bool;
    async fn save_to_file(&self, path: &Path) -> Result<(), std::io::Error>;
    async fn load_from_file(&self, path: &Path) -> Result<(), std::io::Error>;
    fn get_all_sessions(&self) -> Vec<Exchange>;
    fn get_session(&self, id: &str) -> Option<Exchange>;
}

#[async_trait]
impl ExchangeRecorder for SessionManager {
    fn subscribe(&self) -> broadcast::Receiver<SessionChange> {
        self.subscribe()
    }
    async fn flush(&self) {
        self.flush().await
    }
    fn record_request(&self, id: String, request: RequestContext) {
        self.record_request(id, request)
    }
    fn record_request_with_source(
        &self,
        id: String,
        request: RequestContext,
        source: SessionSource,
    ) {
        self.record_request_with_source(id, request, source)
    }
    fn record_response(&self, id: String, response: ResponseContext) {
        self.record_response(id, response)
    }
    fn record_response_with_metrics(
        &self,
        id: String,
        response: ResponseContext,
        metrics: InspectionMetrics,
    ) {
        self.record_response_with_metrics(id, response, metrics)
    }
    fn import_sessions(&self, exchanges: Vec<Exchange>) {
        self.import_sessions(exchanges)
    }
    fn append_ws_frame(&self, id: &str, frame: WsFrame) {
        self.append_ws_frame(id, frame)
    }
    fn append_event(&self, id: &str, event: SessionEvent) {
        self.append_event(id, event)
    }
    fn clear_sessions(&self) {
        self.clear_sessions()
    }
    fn update_inspector_data(&self, id: &str, data: InspectorData) {
        self.update_inspector_data(id, data)
    }
    fn mark_paused(&self, id: &str) {
        self.mark_paused(id)
    }
    fn clear_paused(&self, id: &str) {
        self.clear_paused(id)
    }
    async fn annotate(&self, id: &str, note: Option<String>, tags: Option<Vec<String>>) -> bool {
        self.annotate(id, note, tags).await
    }
    async fn save_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        self.save_to_file(path).await
    }
    async fn load_from_file(&self, path: &Path) -> Result<(), std::io::Error> {
        self.load_from_file(path).await
    }
    fn get_all_sessions(&self) -> Vec<Exchange> {
        self.get_all_sessions()
    }
    fn get_session(&self, id: &str) -> Option<Exchange> {
        self.get_session(id)
    }
}

pub type SharedSessionManager = Arc<dyn ExchangeRecorder>;

// ── Writer task ───────────────────────────────────────────────────────────────

async fn writer_task(
    mut rx: mpsc::Receiver<WriteOp>,
    exchanges: Arc<RwLock<IndexMap<String, Exchange>>>,
    max_sessions: usize,
    max_retained_body_bytes: usize,
    change_tx: broadcast::Sender<SessionChange>,
) {
    let mut store = SessionStore {
        exchanges: &exchanges,
        max_sessions,
        max_retained_body_bytes,
        body_bytes: 0,
    };
    while let Some(op) = rx.recv().await {
        process_write_op(op, &mut store, &change_tx);
    }
}

struct SessionStore<'a> {
    exchanges: &'a RwLock<IndexMap<String, Exchange>>,
    max_sessions: usize,
    max_retained_body_bytes: usize,
    body_bytes: usize,
}

impl SessionStore<'_> {
    fn record_request(&mut self, id: String, request: Box<RequestContext>, source: SessionSource) {
        let mut exchanges = write_lock(self.exchanges);
        self.evict_session_if_full(&mut exchanges, &id);
        self.body_bytes += request.body.len();
        exchanges.insert(
            id.clone(),
            Exchange {
                id,
                timestamp: Utc::now(),
                updated_at: None,
                connection_id: request.connection_id.clone(),
                stream_id: request.stream_id,
                downstream_protocol: request.downstream_protocol.clone(),
                protocol_context: request.protocol_context.clone(),
                request: *request,
                response: None,
                metrics: None,
                source,
                ws_frames: Vec::new(),
                events: Vec::new(),
                note: None,
                tags: Vec::new(),
                inspector_data: None,
                paused_at: None,
            },
        );
        self.enforce_body_budget(&mut exchanges);
    }

    fn record_response(
        &mut self,
        id: &str,
        response: ResponseContext,
        metrics: Option<InspectionMetrics>,
    ) {
        let added = response.body.len();
        let mut exchanges = write_lock(self.exchanges);
        if let Some(exchange) = exchanges.get_mut(id) {
            exchange.response = Some(response);
            if let Some(metrics) = metrics {
                exchange.metrics = Some(metrics);
            }
            exchange.updated_at = Some(Utc::now());
            self.body_bytes += added;
        }
        self.enforce_body_budget(&mut exchanges);
    }

    fn append_ws_frame(&mut self, id: &str, frame: WsFrame) {
        let added = (frame.payload_text.as_ref().map_or(0, String::len)
            + frame.payload_hex.as_ref().map_or(0, String::len))
        .saturating_mul(2);
        let mut exchanges = write_lock(self.exchanges);
        if let Some(exchange) = exchanges.get_mut(id) {
            exchange.events.push(SessionEvent::WsFrame {
                frame: frame.clone(),
            });
            exchange.ws_frames.push(frame);
            self.body_bytes += added;
        }
        self.enforce_body_budget(&mut exchanges);
    }

    fn append_event(&mut self, id: &str, event: SessionEvent) {
        let added = event.retained_body_size();
        let mut exchanges = write_lock(self.exchanges);
        if let Some(exchange) = exchanges.get_mut(id) {
            exchange.events.push(event);
            exchange.updated_at = Some(Utc::now());
            self.body_bytes += added;
        }
        self.enforce_body_budget(&mut exchanges);
    }

    fn annotate(&self, id: &str, note: Option<String>, tags: Option<Vec<String>>) -> bool {
        let mut exchanges = write_lock(self.exchanges);
        let Some(exchange) = exchanges.get_mut(id) else {
            return false;
        };
        if let Some(note) = note {
            exchange.note = (!note.is_empty()).then_some(note);
        }
        if let Some(tags) = tags {
            exchange.tags = tags;
        }
        exchange.updated_at = Some(Utc::now());
        true
    }

    fn import(&mut self, new_exchanges: Vec<Exchange>) {
        let mut exchanges = write_lock(self.exchanges);
        for exchange in new_exchanges {
            self.evict_session_if_full(&mut exchanges, &exchange.id);
            self.body_bytes += SessionManager::exchange_body_size(&exchange);
            exchanges.insert(exchange.id.clone(), exchange);
        }
        self.enforce_body_budget(&mut exchanges);
    }

    fn clear(&mut self) {
        write_lock(self.exchanges).clear();
        self.body_bytes = 0;
    }

    fn update_inspector_data(&self, id: &str, data: InspectorData) {
        if let Some(exchange) = write_lock(self.exchanges).get_mut(id) {
            exchange.inspector_data = Some(data);
        }
    }

    fn set_paused(&self, id: &str, paused: bool) {
        if let Some(exchange) = write_lock(self.exchanges).get_mut(id) {
            exchange.paused_at = paused.then(Utc::now);
            exchange.updated_at = Some(Utc::now());
        }
    }

    fn replace(&mut self, map: IndexMap<String, Exchange>) {
        self.body_bytes = map.values().map(SessionManager::exchange_body_size).sum();
        let mut exchanges = write_lock(self.exchanges);
        *exchanges = map;
        self.enforce_body_budget(&mut exchanges);
    }

    fn evict_session_if_full(
        &mut self,
        exchanges: &mut IndexMap<String, Exchange>,
        incoming_id: &str,
    ) {
        if exchanges.len() >= self.max_sessions
            && !exchanges.contains_key(incoming_id)
            && let Some((_, evicted)) = exchanges.shift_remove_index(0)
        {
            self.body_bytes = self
                .body_bytes
                .saturating_sub(SessionManager::exchange_body_size(&evicted));
        }
    }

    fn enforce_body_budget(&mut self, exchanges: &mut IndexMap<String, Exchange>) {
        if self.max_retained_body_bytes != usize::MAX
            && self.body_bytes > self.max_retained_body_bytes
        {
            enforce_budget(
                exchanges,
                self.max_retained_body_bytes,
                &mut self.body_bytes,
            );
        }
    }
}

fn process_write_op(
    op: WriteOp,
    store: &mut SessionStore<'_>,
    change_tx: &broadcast::Sender<SessionChange>,
) {
    match op {
        WriteOp::RecordRequest {
            id,
            request,
            source,
        } => {
            store.record_request(id.clone(), request, source);
            publish_change(change_tx, Some(id), SessionChangeKind::RequestCaptured);
        }

        WriteOp::RecordResponse { id, response } => {
            store.record_response(&id, response, None);
            publish_change(change_tx, Some(id), SessionChangeKind::ResponseCaptured);
        }

        WriteOp::RecordResponseWithMetrics {
            id,
            response,
            metrics,
        } => {
            store.record_response(&id, response, Some(metrics));
            publish_change(change_tx, Some(id), SessionChangeKind::ResponseCaptured);
        }

        WriteOp::AppendWsFrame { id, frame } => {
            store.append_ws_frame(&id, frame);
            publish_change(change_tx, Some(id), SessionChangeKind::WsFrameCaptured);
        }

        WriteOp::AppendEvent { id, event } => {
            store.append_event(&id, event);
            publish_change(change_tx, Some(id), SessionChangeKind::SessionUpdated);
        }

        WriteOp::Annotate {
            id,
            note,
            tags,
            reply,
        } => {
            let found = store.annotate(&id, note, tags);
            if found {
                publish_change(change_tx, Some(id), SessionChangeKind::SessionUpdated);
            }
            let _ = reply.send(found);
        }

        WriteOp::ImportSessions {
            exchanges: new_exchanges,
        } => {
            store.import(new_exchanges);
            publish_change(change_tx, None, SessionChangeKind::SessionsImported);
        }

        WriteOp::ClearSessions => {
            store.clear();
            publish_change(change_tx, None, SessionChangeKind::SessionsCleared);
        }

        WriteOp::UpdateInspectorData { id, data } => {
            store.update_inspector_data(&id, data);
        }

        WriteOp::MarkPaused { id } => {
            store.set_paused(&id, true);
            publish_change(change_tx, Some(id), SessionChangeKind::SessionPaused);
        }

        WriteOp::ClearPaused { id } => {
            store.set_paused(&id, false);
            publish_change(change_tx, Some(id), SessionChangeKind::SessionUpdated);
        }

        WriteOp::LoadData { map, reply } => {
            store.replace(map);
            publish_change(change_tx, None, SessionChangeKind::SessionsImported);
            let _ = reply.send(());
        }

        WriteOp::Flush(reply) => {
            let _ = reply.send(());
        }
    }
}

fn publish_change(
    change_tx: &broadcast::Sender<SessionChange>,
    session_id: Option<String>,
    kind: SessionChangeKind,
) {
    let _ = change_tx.send(SessionChange { session_id, kind });
}

/// Evict body content from the oldest exchanges until the budget is satisfied.
/// Called with the write lock already held by the writer task.
fn enforce_budget(
    store: &mut IndexMap<String, Exchange>,
    max_retained_body_bytes: usize,
    body_bytes: &mut usize,
) {
    for ex in store.values_mut() {
        if *body_bytes <= max_retained_body_bytes {
            break;
        }
        let freed = SessionManager::exchange_body_size(ex);
        if freed == 0 {
            continue;
        }
        SessionManager::clear_exchange_bodies(ex);
        *body_bytes = body_bytes.saturating_sub(freed);
    }
}

// ── Search ────────────────────────────────────────────────────────────────────
pub mod search;
pub use search::parse_search_query;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{RequestContext, ResponseContext};

    fn req(uri: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: crate::middleware::HeaderMap::new(),
            body: bytes::Bytes::from_static(b"body"),
            host: "localhost".to_string(),
            ..Default::default()
        }
    }

    fn res(uri: &str, status: u16) -> ResponseContext {
        ResponseContext {
            status,
            headers: crate::middleware::HeaderMap::new(),
            body: bytes::Bytes::from_static(b"response"),
            request_uri: uri.to_string(),
            session_id: None,
            ttfb_ms: 0,
            body_ms: 0,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn record_request_creates_exchange() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.flush().await;
        let all = sm.get_all_sessions();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "id1");
        assert_eq!(all[0].request.uri, "/test");
        assert!(all[0].response.is_none());
        assert!(all[0].metrics.is_none());
    }

    #[tokio::test]
    async fn record_response_attaches_to_existing_exchange() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.record_response("id1".to_string(), res("/test", 200));
        sm.flush().await;
        let session = sm.get_session("id1").unwrap();
        assert_eq!(session.response.unwrap().status, 200);
    }

    #[tokio::test]
    async fn record_response_for_unknown_id_is_noop() {
        let sm = SessionManager::new(10_000);
        sm.record_response("ghost".to_string(), res("/test", 200));
        sm.flush().await;
        assert!(sm.get_all_sessions().is_empty());
    }

    #[tokio::test]
    async fn append_ws_frame_populates_compat_and_event_paths() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/ws"));
        sm.append_ws_frame(
            "id1",
            WsFrame {
                timestamp: Utc::now(),
                direction: WsDirection::ClientToServer,
                opcode: 1,
                payload_len: 5,
                payload_text: Some("hello".to_string()),
                payload_hex: None,
            },
        );
        sm.flush().await;

        let session = sm.get_session("id1").unwrap();
        assert_eq!(session.ws_frames.len(), 1);
        assert_eq!(session.events.len(), 1);
        assert!(matches!(
            &session.events[0],
            SessionEvent::WsFrame { frame } if frame.payload_text.as_deref() == Some("hello")
        ));
    }

    #[tokio::test]
    async fn append_event_records_protocol_event() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/break"));
        sm.append_event(
            "id1",
            SessionEvent::BreakpointPaused {
                breakpoint_id: "bp-1".to_string(),
            },
        );
        sm.flush().await;

        let session = sm.get_session("id1").unwrap();
        assert!(matches!(
            &session.events[0],
            SessionEvent::BreakpointPaused { breakpoint_id } if breakpoint_id == "bp-1"
        ));
    }

    #[tokio::test]
    async fn record_response_with_metrics_stores_all_fields() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/x"));
        let metrics = InspectionMetrics {
            latency_ms: 42,
            request_size_bytes: 10,
            response_size_bytes: 20,
            status_code: 404,
            ttfb_ms: 0,
            body_ms: 0,
            ..Default::default()
        };
        sm.record_response_with_metrics("id1".to_string(), res("/x", 404), metrics);
        sm.flush().await;
        let session = sm.get_session("id1").unwrap();
        let m = session.metrics.unwrap();
        assert_eq!(m.latency_ms, 42);
        assert_eq!(m.status_code, 404);
        assert_eq!(m.request_size_bytes, 10);
        assert_eq!(m.response_size_bytes, 20);
    }

    #[tokio::test]
    async fn get_session_returns_none_for_missing_id() {
        let sm = SessionManager::new(10_000);
        assert!(sm.get_session("does-not-exist").is_none());
    }

    #[tokio::test]
    async fn clear_sessions_empties_store() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/a"));
        sm.record_request("b".to_string(), req("/b"));
        sm.flush().await;
        assert_eq!(sm.get_all_sessions().len(), 2);
        sm.clear_sessions();
        sm.flush().await;
        assert!(sm.get_all_sessions().is_empty());
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/save-test"));

        let path = std::env::temp_dir().join("oproxy_session_roundtrip_test.json");
        sm.save_to_file(&path).await.expect("save failed");

        let sm2 = SessionManager::new(10_000);
        sm2.load_from_file(&path).await.expect("load failed");
        let sessions = sm2.get_all_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "id1");
        assert_eq!(sessions[0].request.uri, "/save-test");

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn load_from_nonexistent_file_returns_error() {
        let sm = SessionManager::new(10_000);
        let result = sm
            .load_from_file(Path::new("/nonexistent/path/sessions.json"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn duplicate_id_overwrites_previous_exchange() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/first"));
        sm.record_request("id1".to_string(), req("/second"));
        sm.flush().await;
        let all = sm.get_all_sessions();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].request.uri, "/second");
    }

    #[tokio::test]
    async fn session_cap_evicts_oldest_when_full() {
        let cap = 5;
        let sm = SessionManager::new(cap);
        for i in 0..cap {
            sm.record_request(format!("id-{}", i), req(&format!("/{}", i)));
        }
        sm.flush().await;
        assert_eq!(sm.get_all_sessions().len(), cap);
        sm.record_request("id-new".to_string(), req("/new"));
        sm.flush().await;
        let all = sm.get_all_sessions();
        assert_eq!(all.len(), cap, "must not grow past cap");
        assert!(
            all.iter().any(|e| e.id == "id-new"),
            "new session must be present"
        );
    }

    #[tokio::test]
    async fn body_budget_drops_oldest_bodies_but_keeps_metadata() {
        let sm = SessionManager::with_body_budget(10, 24);
        sm.record_request(
            "old".to_string(),
            RequestContext {
                body: bytes::Bytes::from_static(b"old-request-body"),
                ..req("/old")
            },
        );
        sm.record_response(
            "old".to_string(),
            ResponseContext {
                body: bytes::Bytes::from_static(b"old-response-body"),
                ..res("/old", 200)
            },
        );
        sm.record_request("new".to_string(), req("/new"));
        sm.flush().await;

        let old = sm.get_session("old").unwrap();
        let new = sm.get_session("new").unwrap();

        assert_eq!(old.request.uri, "/old");
        assert_eq!(old.response.as_ref().unwrap().status, 200);
        assert!(old.request.body.is_empty());
        assert!(old.response.as_ref().unwrap().body.is_empty());
        assert_eq!(new.request.body_text(), "body");
    }

    #[tokio::test]
    async fn subscribe_fires_on_record_request() {
        let sm = SessionManager::new(10_000);
        let mut rx = sm.subscribe();
        sm.record_request("id1".to_string(), req("/ping"));
        sm.flush().await;
        let change = rx
            .try_recv()
            .expect("subscriber should receive notification");
        assert_eq!(change.kind, SessionChangeKind::RequestCaptured);
        assert_eq!(change.session_id.as_deref(), Some("id1"));
    }

    #[tokio::test]
    async fn get_all_sessions_returns_insertion_order() {
        let sm = SessionManager::new(10_000);
        for i in 0..5u32 {
            sm.record_request(format!("id-{}", i), req(&format!("/{}", i)));
        }
        sm.flush().await;
        let all = sm.get_all_sessions();
        for (i, e) in all.iter().enumerate() {
            assert_eq!(e.id, format!("id-{}", i));
        }
    }

    #[tokio::test]
    async fn record_request_has_no_updated_at() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.flush().await;
        let session = sm.get_session("id1").unwrap();
        assert!(
            session.updated_at.is_none(),
            "updated_at must be None until a response arrives"
        );
    }

    #[tokio::test]
    async fn record_response_sets_updated_at() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let before = Utc::now();
        sm.record_response("id1".to_string(), res("/test", 200));
        sm.flush().await;
        let after = Utc::now();
        let session = sm.get_session("id1").unwrap();
        let updated_at = session
            .updated_at
            .expect("updated_at must be set after record_response");
        assert!(
            updated_at >= before && updated_at <= after,
            "updated_at must be recent"
        );
    }

    #[tokio::test]
    async fn record_response_with_metrics_sets_updated_at() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let metrics = InspectionMetrics {
            latency_ms: 10,
            request_size_bytes: 0,
            response_size_bytes: 0,
            status_code: 200,
            ttfb_ms: 0,
            body_ms: 0,
            ..Default::default()
        };
        let before = Utc::now();
        sm.record_response_with_metrics("id1".to_string(), res("/test", 200), metrics);
        sm.flush().await;
        let after = Utc::now();
        let session = sm.get_session("id1").unwrap();
        let updated_at = session
            .updated_at
            .expect("updated_at must be set after record_response_with_metrics");
        assert!(updated_at >= before && updated_at <= after);
    }

    // ── annotations ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn annotate_stores_note_and_tags() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let ok = sm
            .annotate(
                "id1",
                Some("auth bug".to_string()),
                Some(vec!["auth".to_string(), "bug".to_string()]),
            )
            .await;
        assert!(ok);
        let ex = sm.get_session("id1").unwrap();
        assert_eq!(ex.note.as_deref(), Some("auth bug"));
        assert_eq!(ex.tags, vec!["auth", "bug"]);
    }

    #[tokio::test]
    async fn annotate_partial_note_only_leaves_tags_unchanged() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.annotate("id1", None, Some(vec!["existing".to_string()]))
            .await;
        sm.annotate("id1", Some("new note".to_string()), None).await;
        let ex = sm.get_session("id1").unwrap();
        assert_eq!(ex.note.as_deref(), Some("new note"));
        assert_eq!(ex.tags, vec!["existing"]);
    }

    #[tokio::test]
    async fn annotate_partial_tags_only_leaves_note_unchanged() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.annotate("id1", Some("original".to_string()), None).await;
        sm.annotate("id1", None, Some(vec!["new-tag".to_string()]))
            .await;
        let ex = sm.get_session("id1").unwrap();
        assert_eq!(ex.note.as_deref(), Some("original"));
        assert_eq!(ex.tags, vec!["new-tag"]);
    }

    #[tokio::test]
    async fn annotate_empty_string_clears_note() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.annotate("id1", Some("note".to_string()), None).await;
        sm.annotate("id1", Some(String::new()), None).await;
        let ex = sm.get_session("id1").unwrap();
        assert!(ex.note.is_none());
    }

    #[tokio::test]
    async fn annotate_empty_tags_clears_tags() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.annotate("id1", None, Some(vec!["tag".to_string()]))
            .await;
        sm.annotate("id1", None, Some(vec![])).await;
        let ex = sm.get_session("id1").unwrap();
        assert!(ex.tags.is_empty());
    }

    #[tokio::test]
    async fn annotate_missing_session_returns_false() {
        let sm = SessionManager::new(10_000);
        let ok = sm
            .annotate("nonexistent", Some("note".to_string()), None)
            .await;
        assert!(!ok);
    }

    #[tokio::test]
    async fn annotate_sets_updated_at() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let before = Utc::now();
        sm.annotate("id1", Some("note".to_string()), None).await;
        let after = Utc::now();
        let ex = sm.get_session("id1").unwrap();
        let ua = ex.updated_at.unwrap();
        assert!(ua >= before && ua <= after);
    }

    #[tokio::test]
    async fn annotate_triggers_sse_notification() {
        let sm = SessionManager::new(10_000);
        let mut rx = sm.subscribe();
        sm.record_request("id1".to_string(), req("/test"));
        // Flush so the record_request notification is in the broadcast buffer.
        sm.flush().await;
        let _ = rx.try_recv(); // drain record_request notification
        sm.annotate("id1", Some("note".to_string()), None).await;
        let change = rx.try_recv().expect("annotate must fire SSE notification");
        assert_eq!(change.kind, SessionChangeKind::SessionUpdated);
        assert_eq!(change.session_id.as_deref(), Some("id1"));
    }

    #[tokio::test]
    async fn annotation_roundtrip_through_save_load() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/save-annot-test"));
        sm.annotate(
            "id1",
            Some("important".to_string()),
            Some(vec!["prod".to_string()]),
        )
        .await;

        let path = std::env::temp_dir().join("oproxy_annot_roundtrip_test.json");
        sm.save_to_file(&path).await.expect("save failed");

        let sm2 = SessionManager::new(10_000);
        sm2.load_from_file(&path).await.expect("load failed");
        let ex = sm2.get_session("id1").unwrap();
        assert_eq!(ex.note.as_deref(), Some("important"));
        assert_eq!(ex.tags, vec!["prod"]);

        let _ = tokio::fs::remove_file(&path).await;
    }

    // ── InspectionMetrics waterfall fields ───────────────────────────────────

    #[test]
    fn inspection_metrics_optional_timing_fields_default_to_none() {
        let m: InspectionMetrics = Default::default();
        assert!(m.dns_ms.is_none());
        assert!(m.tcp_connect_ms.is_none());
        assert!(m.tls_ms.is_none());
    }

    #[test]
    fn inspection_metrics_timing_fields_roundtrip_via_serde() {
        let m = InspectionMetrics {
            latency_ms: 120,
            request_size_bytes: 256,
            response_size_bytes: 1024,
            status_code: 200,
            ttfb_ms: 80,
            body_ms: 40,
            dns_ms: Some(10),
            tcp_connect_ms: Some(15),
            tls_ms: Some(25),
            protocol: None,
        };
        let json = serde_json::to_string(&m).unwrap();
        let m2: InspectionMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(m2.dns_ms, Some(10));
        assert_eq!(m2.tcp_connect_ms, Some(15));
        assert_eq!(m2.tls_ms, Some(25));
    }

    #[test]
    fn inspection_metrics_absent_timing_fields_omitted_from_json() {
        let m = InspectionMetrics {
            latency_ms: 10,
            ..Default::default()
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            !json.contains("dns_ms"),
            "absent optional fields must not appear in JSON"
        );
        assert!(!json.contains("tcp_connect_ms"));
        assert!(!json.contains("tls_ms"));
    }

    #[tokio::test]
    async fn record_response_with_timing_metrics_stores_optional_fields() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let metrics = InspectionMetrics {
            latency_ms: 120,
            request_size_bytes: 0,
            response_size_bytes: 0,
            status_code: 200,
            ttfb_ms: 80,
            body_ms: 40,
            dns_ms: Some(5),
            tcp_connect_ms: Some(10),
            tls_ms: Some(20),
            protocol: None,
        };
        sm.record_response_with_metrics("id1".to_string(), res("/test", 200), metrics);
        sm.flush().await;
        let ex = sm.get_session("id1").unwrap();
        let m = ex.metrics.unwrap();
        assert_eq!(m.dns_ms, Some(5));
        assert_eq!(m.tcp_connect_ms, Some(10));
        assert_eq!(m.tls_ms, Some(20));
    }

    #[test]
    fn parse_search_query_tag_term() {
        let terms = parse_search_query("tag:auth");
        assert_eq!(terms.len(), 1);
        let ex = Exchange {
            id: "x".to_string(),
            timestamp: Utc::now(),
            updated_at: None,
            request: RequestContext {
                method: "GET".to_string(),
                uri: "/x".to_string(),
                headers: crate::middleware::HeaderMap::new(),
                body: bytes::Bytes::new(),
                host: "localhost".to_string(),
                ..Default::default()
            },
            response: None,
            metrics: None,
            source: SessionSource::Proxy,
            ws_frames: vec![],
            events: vec![],
            note: None,
            tags: vec!["auth".to_string()],
            inspector_data: None,
            paused_at: None,
            connection_id: None,
            stream_id: None,
            downstream_protocol: None,
            protocol_context: None,
        };
        assert!(terms[0].matches(&ex));
        let ex2 = Exchange { tags: vec![], ..ex };
        assert!(!terms[0].matches(&ex2));
    }

    #[tokio::test]
    async fn import_sessions_preserves_existing_updated_at() {
        let sm = SessionManager::new(10_000);
        let fixed_time = Utc::now() - chrono::Duration::hours(2);
        let exchange = Exchange {
            id: "imported".to_string(),
            timestamp: fixed_time,
            updated_at: Some(fixed_time),
            request: req("/imported"),
            response: None,
            metrics: None,
            source: SessionSource::Proxy,
            ws_frames: vec![],
            events: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
            paused_at: None,
            connection_id: None,
            stream_id: None,
            downstream_protocol: None,
            protocol_context: None,
        };
        sm.import_sessions(vec![exchange]);
        sm.flush().await;
        let session = sm.get_session("imported").unwrap();
        assert_eq!(session.updated_at, Some(fixed_time));
    }
}
