use crate::middleware::{RequestContext, ResponseContext};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct GrpcInfo {
    pub service: Option<String>,
    pub method: Option<String>,
    pub messages: Vec<GrpcMessage>,
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
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspector_data: Option<InspectorData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionChangeKind {
    RequestCaptured,
    ResponseCaptured,
    SessionUpdated,
    SessionsImported,
    SessionsCleared,
    WsFrameCaptured,
}

#[derive(Debug, Clone)]
pub struct SessionChange {
    pub session_id: Option<String>,
    pub kind: SessionChangeKind,
}

pub struct SessionManager {
    exchanges: RwLock<IndexMap<String, Exchange>>,
    max_sessions: usize,
    max_retained_body_bytes: usize,
    // Fired whenever sessions change; SSE subscribers receive notifications.
    change_tx: broadcast::Sender<SessionChange>,
}

impl SessionManager {
    #[allow(dead_code)]
    pub fn new(max_sessions: usize) -> Self {
        Self::with_body_budget(max_sessions, usize::MAX)
    }

    pub fn with_body_budget(max_sessions: usize, max_retained_body_bytes: usize) -> Self {
        let (change_tx, _) = broadcast::channel(64);
        Self {
            exchanges: RwLock::new(IndexMap::new()),
            max_sessions,
            max_retained_body_bytes,
            change_tx,
        }
    }

    /// Returns a broadcast receiver that fires on every session change.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionChange> {
        self.change_tx.subscribe()
    }

    fn notify(&self, kind: SessionChangeKind, session_id: Option<String>) {
        let _ = self.change_tx.send(SessionChange { session_id, kind });
    }

    pub async fn save_to_file<P: AsRef<Path> + Send>(&self, path: P) -> Result<(), std::io::Error> {
        let json = {
            let guard = self.exchanges.read().unwrap();
            serde_json::to_string_pretty(&*guard)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
        };
        tokio::fs::write(path, json).await
    }

    pub async fn load_from_file<P: AsRef<Path> + Send>(
        &self,
        path: P,
    ) -> Result<(), std::io::Error> {
        let data = tokio::fs::read(path).await?;
        let exchanges: IndexMap<String, Exchange> = serde_json::from_slice(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        {
            let mut guard = self.exchanges.write().unwrap();
            *guard = exchanges;
            self.enforce_body_budget(&mut guard);
        }
        self.notify(SessionChangeKind::SessionsImported, None);
        Ok(())
    }

    pub fn record_request(&self, id: String, request: RequestContext) {
        self.record_request_with_source(id, request, SessionSource::Proxy);
    }

    pub fn record_request_with_source(
        &self,
        id: String,
        request: RequestContext,
        source: SessionSource,
    ) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            if exchanges.len() >= self.max_sessions && !exchanges.contains_key(&id) {
                // Evict the oldest entry (insertion order ≈ arrival order).
                exchanges.shift_remove_index(0);
            }
            exchanges.insert(
                id.clone(),
                Exchange {
                    id: id.clone(),
                    timestamp: Utc::now(),
                    updated_at: None,
                    request,
                    response: None,
                    metrics: None,
                    source,
                    ws_frames: Vec::new(),
                    note: None,
                    tags: Vec::new(),
                    inspector_data: None,
                },
            );
            self.enforce_body_budget(&mut exchanges);
        }
        self.notify(SessionChangeKind::RequestCaptured, Some(id));
    }

    pub fn record_response(&self, id: String, response: ResponseContext) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            if let Some(exchange) = exchanges.get_mut(&id) {
                exchange.response = Some(response);
                exchange.updated_at = Some(Utc::now());
            }
            self.enforce_body_budget(&mut exchanges);
        }
        self.notify(SessionChangeKind::ResponseCaptured, Some(id));
    }

    pub fn record_response_with_metrics(
        &self,
        id: String,
        response: ResponseContext,
        metrics: InspectionMetrics,
    ) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            if let Some(exchange) = exchanges.get_mut(&id) {
                exchange.response = Some(response);
                exchange.metrics = Some(metrics);
                exchange.updated_at = Some(Utc::now());
            }
            self.enforce_body_budget(&mut exchanges);
        }
        self.notify(SessionChangeKind::ResponseCaptured, Some(id));
    }

    pub fn import_sessions(&self, exchanges: Vec<Exchange>) {
        {
            let mut store = self.exchanges.write().unwrap();
            for e in exchanges {
                if store.len() >= self.max_sessions && !store.contains_key(&e.id) {
                    store.shift_remove_index(0);
                }
                store.insert(e.id.clone(), e);
            }
            self.enforce_body_budget(&mut store);
        }
        self.notify(SessionChangeKind::SessionsImported, None);
    }

    pub fn append_ws_frame(&self, id: &str, frame: WsFrame) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            if let Some(exchange) = exchanges.get_mut(id) {
                exchange.ws_frames.push(frame);
            }
            self.enforce_body_budget(&mut exchanges);
        }
        self.notify(SessionChangeKind::WsFrameCaptured, Some(id.to_string()));
    }

    /// Update the note and/or tags on an existing session.
    /// `note: Some(x)` replaces the note; `None` leaves it unchanged.
    /// `tags: Some(v)` replaces the tag list; `None` leaves it unchanged.
    /// Returns `false` if no session with `id` exists.
    pub fn annotate(&self, id: &str, note: Option<String>, tags: Option<Vec<String>>) -> bool {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            match exchanges.get_mut(id) {
                None => return false,
                Some(ex) => {
                    if let Some(n) = note {
                        ex.note = if n.is_empty() { None } else { Some(n) };
                    }
                    if let Some(t) = tags {
                        ex.tags = t;
                    }
                    ex.updated_at = Some(Utc::now());
                }
            }
        }
        self.notify(SessionChangeKind::SessionUpdated, Some(id.to_string()));
        true
    }

    pub fn get_all_sessions(&self) -> Vec<Exchange> {
        let exchanges = self.exchanges.read().unwrap();
        exchanges.values().cloned().collect()
    }

    /// Full-text search across URI, headers, body, notes, and tags.
    /// Supports `tag:x`, `host:x`, `method:GET`, `status:200` prefix syntax.
    /// Multiple terms use AND semantics. Empty query returns all sessions.
    pub fn search_sessions(&self, query: &str) -> Vec<Exchange> {
        if query.trim().is_empty() {
            return self.get_all_sessions();
        }
        let terms = parse_search_query(query);
        let exchanges = self.exchanges.read().unwrap();
        exchanges
            .values()
            .filter(|ex| terms.iter().all(|t| t.matches(ex)))
            .cloned()
            .collect()
    }

    pub fn get_session(&self, id: &str) -> Option<Exchange> {
        let exchanges = self.exchanges.read().unwrap();
        exchanges.get(id).cloned()
    }

    pub fn clear_sessions(&self) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            exchanges.clear();
        }
        self.notify(SessionChangeKind::SessionsCleared, None);
    }

    pub fn update_inspector_data(&self, id: &str, data: InspectorData) -> bool {
        let mut exchanges = self.exchanges.write().unwrap();
        if let Some(ex) = exchanges.get_mut(id) {
            ex.inspector_data = Some(data);
            true
        } else {
            false
        }
    }

    fn enforce_body_budget(&self, exchanges: &mut IndexMap<String, Exchange>) {
        if self.max_retained_body_bytes == usize::MAX {
            return;
        }

        let mut retained = exchanges
            .values()
            .map(Self::exchange_body_size)
            .sum::<usize>();
        if retained <= self.max_retained_body_bytes {
            return;
        }

        for exchange in exchanges.values_mut() {
            if retained <= self.max_retained_body_bytes {
                break;
            }
            let before = Self::exchange_body_size(exchange);
            if before == 0 {
                continue;
            }
            Self::clear_exchange_bodies(exchange);
            retained = retained.saturating_sub(before);
        }
    }

    fn exchange_body_size(exchange: &Exchange) -> usize {
        let request_bytes = exchange.request.body.len()
            + exchange
                .request
                .body_bytes
                .as_ref()
                .map_or(0, |bytes| bytes.len());
        let response_bytes = exchange.response.as_ref().map_or(0, |response| {
            response.body.len() + response.body_bytes.as_ref().map_or(0, |bytes| bytes.len())
        });
        let ws_bytes = exchange
            .ws_frames
            .iter()
            .map(|frame| {
                frame.payload_text.as_ref().map_or(0, String::len)
                    + frame.payload_hex.as_ref().map_or(0, String::len)
            })
            .sum::<usize>();

        request_bytes + response_bytes + ws_bytes
    }

    fn clear_exchange_bodies(exchange: &mut Exchange) {
        exchange.request.body.clear();
        exchange.request.body_bytes = None;
        if let Some(response) = &mut exchange.response {
            response.body.clear();
            response.body_bytes = None;
        }
        for frame in &mut exchange.ws_frames {
            frame.payload_text = None;
            frame.payload_hex = None;
        }
    }
}

pub type SharedSessionManager = Arc<SessionManager>;

// ── Search ────────────────────────────────────────────────────────────────────

pub enum SearchTerm {
    Tag(String),
    Host(String),
    Method(String),
    Status(u16),
    Text(String),
}

impl SearchTerm {
    pub fn matches(&self, ex: &Exchange) -> bool {
        match self {
            SearchTerm::Tag(t) => ex
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(t.as_str())),
            SearchTerm::Host(h) => ex.request.host.to_lowercase().contains(h.as_str()),
            SearchTerm::Method(m) => ex.request.method.to_lowercase() == m.as_str(),
            SearchTerm::Status(s) => ex
                .response
                .as_ref()
                .map(|r| r.status == *s)
                .unwrap_or(false),
            SearchTerm::Text(t) => {
                let t = t.as_str();
                ex.request.uri.to_lowercase().contains(t)
                    || ex.request.body.to_lowercase().contains(t)
                    || ex
                        .request
                        .headers
                        .iter()
                        .any(|(k, v)| k.to_lowercase().contains(t) || v.to_lowercase().contains(t))
                    || ex
                        .response
                        .as_ref()
                        .map(|r| {
                            r.body.to_lowercase().contains(t)
                                || r.headers.iter().any(|(k, v)| {
                                    k.to_lowercase().contains(t) || v.to_lowercase().contains(t)
                                })
                        })
                        .unwrap_or(false)
                    || ex
                        .note
                        .as_deref()
                        .map(|n| n.to_lowercase().contains(t))
                        .unwrap_or(false)
            }
        }
    }
}

pub fn parse_search_query(query: &str) -> Vec<SearchTerm> {
    query
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|token| {
            if let Some(t) = token.strip_prefix("tag:") {
                SearchTerm::Tag(t.to_lowercase())
            } else if let Some(h) = token.strip_prefix("host:") {
                SearchTerm::Host(h.to_lowercase())
            } else if let Some(m) = token.strip_prefix("method:") {
                SearchTerm::Method(m.to_lowercase())
            } else if let Some(s) = token.strip_prefix("status:") {
                s.parse::<u16>()
                    .map(SearchTerm::Status)
                    .unwrap_or_else(|_| SearchTerm::Text(s.to_lowercase()))
            } else {
                SearchTerm::Text(token.to_lowercase())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{RequestContext, ResponseContext};
    use std::collections::HashMap;

    fn req(uri: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: HashMap::new(),
            body: "body".to_string(),
            host: "localhost".to_string(),
            body_bytes: None,
        }
    }

    fn res(uri: &str, status: u16) -> ResponseContext {
        ResponseContext {
            status,
            headers: HashMap::new(),
            body: "response".to_string(),
            request_uri: uri.to_string(),
            session_id: None,
            ttfb_ms: 0,
            body_ms: 0,
            body_bytes: None,
        }
    }

    #[test]
    fn record_request_creates_exchange() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let all = sm.get_all_sessions();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "id1");
        assert_eq!(all[0].request.uri, "/test");
        assert!(all[0].response.is_none());
        assert!(all[0].metrics.is_none());
    }

    #[test]
    fn record_response_attaches_to_existing_exchange() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.record_response("id1".to_string(), res("/test", 200));
        let session = sm.get_session("id1").unwrap();
        assert_eq!(session.response.unwrap().status, 200);
    }

    #[test]
    fn record_response_for_unknown_id_is_noop() {
        let sm = SessionManager::new(10_000);
        sm.record_response("ghost".to_string(), res("/test", 200));
        assert!(sm.get_all_sessions().is_empty());
    }

    #[test]
    fn record_response_with_metrics_stores_all_fields() {
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
        let session = sm.get_session("id1").unwrap();
        let m = session.metrics.unwrap();
        assert_eq!(m.latency_ms, 42);
        assert_eq!(m.status_code, 404);
        assert_eq!(m.request_size_bytes, 10);
        assert_eq!(m.response_size_bytes, 20);
    }

    #[test]
    fn get_session_returns_none_for_missing_id() {
        let sm = SessionManager::new(10_000);
        assert!(sm.get_session("does-not-exist").is_none());
    }

    #[test]
    fn clear_sessions_empties_store() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/a"));
        sm.record_request("b".to_string(), req("/b"));
        assert_eq!(sm.get_all_sessions().len(), 2);
        sm.clear_sessions();
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
        let result = sm.load_from_file("/nonexistent/path/sessions.json").await;
        assert!(result.is_err());
    }

    #[test]
    fn duplicate_id_overwrites_previous_exchange() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/first"));
        sm.record_request("id1".to_string(), req("/second"));
        let all = sm.get_all_sessions();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].request.uri, "/second");
    }

    #[test]
    fn session_cap_evicts_oldest_when_full() {
        let cap = 5;
        let sm = SessionManager::new(cap);
        for i in 0..cap {
            sm.record_request(format!("id-{}", i), req(&format!("/{}", i)));
        }
        assert_eq!(sm.get_all_sessions().len(), cap);
        sm.record_request("id-new".to_string(), req("/new"));
        let all = sm.get_all_sessions();
        assert_eq!(all.len(), cap, "must not grow past cap");
        assert!(
            all.iter().any(|e| e.id == "id-new"),
            "new session must be present"
        );
    }

    #[test]
    fn body_budget_drops_oldest_bodies_but_keeps_metadata() {
        let sm = SessionManager::with_body_budget(10, 24);
        sm.record_request(
            "old".to_string(),
            RequestContext {
                body: "old-request-body".to_string(),
                body_bytes: Some(bytes::Bytes::from_static(b"old-request-body")),
                ..req("/old")
            },
        );
        sm.record_response(
            "old".to_string(),
            ResponseContext {
                body: "old-response-body".to_string(),
                body_bytes: Some(bytes::Bytes::from_static(b"old-response-body")),
                ..res("/old", 200)
            },
        );
        sm.record_request("new".to_string(), req("/new"));

        let old = sm.get_session("old").unwrap();
        let new = sm.get_session("new").unwrap();

        assert_eq!(old.request.uri, "/old");
        assert_eq!(old.response.as_ref().unwrap().status, 200);
        assert!(old.request.body.is_empty());
        assert!(old.request.body_bytes.is_none());
        assert!(old.response.as_ref().unwrap().body.is_empty());
        assert!(old.response.as_ref().unwrap().body_bytes.is_none());
        assert_eq!(new.request.body, "body");
    }

    #[test]
    fn subscribe_fires_on_record_request() {
        let sm = SessionManager::new(10_000);
        let mut rx = sm.subscribe();
        sm.record_request("id1".to_string(), req("/ping"));
        let change = rx
            .try_recv()
            .expect("subscriber should receive notification");
        assert_eq!(change.kind, SessionChangeKind::RequestCaptured);
        assert_eq!(change.session_id.as_deref(), Some("id1"));
    }

    #[test]
    fn get_all_sessions_returns_insertion_order() {
        let sm = SessionManager::new(10_000);
        for i in 0..5u32 {
            sm.record_request(format!("id-{}", i), req(&format!("/{}", i)));
        }
        let all = sm.get_all_sessions();
        for (i, e) in all.iter().enumerate() {
            assert_eq!(e.id, format!("id-{}", i));
        }
    }

    #[test]
    fn record_request_has_no_updated_at() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let session = sm.get_session("id1").unwrap();
        assert!(
            session.updated_at.is_none(),
            "updated_at must be None until a response arrives"
        );
    }

    #[test]
    fn record_response_sets_updated_at() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let before = Utc::now();
        sm.record_response("id1".to_string(), res("/test", 200));
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

    #[test]
    fn record_response_with_metrics_sets_updated_at() {
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
        let after = Utc::now();
        let session = sm.get_session("id1").unwrap();
        let updated_at = session
            .updated_at
            .expect("updated_at must be set after record_response_with_metrics");
        assert!(updated_at >= before && updated_at <= after);
    }

    // ── annotations ──────────────────────────────────────────────────────────

    #[test]
    fn annotate_stores_note_and_tags() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let ok = sm.annotate(
            "id1",
            Some("auth bug".to_string()),
            Some(vec!["auth".to_string(), "bug".to_string()]),
        );
        assert!(ok);
        let ex = sm.get_session("id1").unwrap();
        assert_eq!(ex.note.as_deref(), Some("auth bug"));
        assert_eq!(ex.tags, vec!["auth", "bug"]);
    }

    #[test]
    fn annotate_partial_note_only_leaves_tags_unchanged() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.annotate("id1", None, Some(vec!["existing".to_string()]));
        sm.annotate("id1", Some("new note".to_string()), None);
        let ex = sm.get_session("id1").unwrap();
        assert_eq!(ex.note.as_deref(), Some("new note"));
        assert_eq!(ex.tags, vec!["existing"]);
    }

    #[test]
    fn annotate_partial_tags_only_leaves_note_unchanged() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.annotate("id1", Some("original".to_string()), None);
        sm.annotate("id1", None, Some(vec!["new-tag".to_string()]));
        let ex = sm.get_session("id1").unwrap();
        assert_eq!(ex.note.as_deref(), Some("original"));
        assert_eq!(ex.tags, vec!["new-tag"]);
    }

    #[test]
    fn annotate_empty_string_clears_note() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.annotate("id1", Some("note".to_string()), None);
        sm.annotate("id1", Some(String::new()), None);
        let ex = sm.get_session("id1").unwrap();
        assert!(ex.note.is_none());
    }

    #[test]
    fn annotate_empty_tags_clears_tags() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        sm.annotate("id1", None, Some(vec!["tag".to_string()]));
        sm.annotate("id1", None, Some(vec![]));
        let ex = sm.get_session("id1").unwrap();
        assert!(ex.tags.is_empty());
    }

    #[test]
    fn annotate_missing_session_returns_false() {
        let sm = SessionManager::new(10_000);
        let ok = sm.annotate("nonexistent", Some("note".to_string()), None);
        assert!(!ok);
    }

    #[test]
    fn annotate_sets_updated_at() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let before = Utc::now();
        sm.annotate("id1", Some("note".to_string()), None);
        let after = Utc::now();
        let ex = sm.get_session("id1").unwrap();
        let ua = ex.updated_at.unwrap();
        assert!(ua >= before && ua <= after);
    }

    #[test]
    fn annotate_triggers_sse_notification() {
        let sm = SessionManager::new(10_000);
        sm.record_request("id1".to_string(), req("/test"));
        let mut rx = sm.subscribe();
        // Drain the record_request notification.
        let _ = rx.try_recv();
        sm.annotate("id1", Some("note".to_string()), None);
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
        );

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

    #[test]
    fn record_response_with_timing_metrics_stores_optional_fields() {
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
        };
        sm.record_response_with_metrics("id1".to_string(), res("/test", 200), metrics);
        let ex = sm.get_session("id1").unwrap();
        let m = ex.metrics.unwrap();
        assert_eq!(m.dns_ms, Some(5));
        assert_eq!(m.tcp_connect_ms, Some(10));
        assert_eq!(m.tls_ms, Some(20));
    }

    // ── search_sessions ───────────────────────────────────────────────────────

    fn req_with_host(uri: &str, host: &str) -> RequestContext {
        let mut h = HashMap::new();
        h.insert("host".to_string(), host.to_string());
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: h,
            body: String::new(),
            host: host.to_string(),
            body_bytes: None,
        }
    }

    #[test]
    fn search_empty_query_returns_all() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/a"));
        sm.record_request("b".to_string(), req("/b"));
        let results = sm.search_sessions("");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_text_matches_uri() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/api/users"));
        sm.record_request("b".to_string(), req("/health"));
        let results = sm.search_sessions("users");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn search_host_prefix() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req_with_host("/x", "api.example.com"));
        sm.record_request("b".to_string(), req_with_host("/y", "static.example.com"));
        let results = sm.search_sessions("host:api.example");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn search_tag_prefix() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/a"));
        sm.record_request("b".to_string(), req("/b"));
        sm.annotate("a", None, Some(vec!["auth".to_string()]));
        let results = sm.search_sessions("tag:auth");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn search_multiple_terms_and_semantics() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/api/auth/login"));
        sm.record_request("b".to_string(), req("/api/users"));
        // only "a" matches both "api" and "login"
        let results = sm.search_sessions("api login");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn search_status_prefix() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/a"));
        sm.record_request("b".to_string(), req("/b"));
        sm.record_response("a".to_string(), res("/a", 404));
        sm.record_response("b".to_string(), res("/b", 200));
        let results = sm.search_sessions("status:404");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn search_note_matches_text() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/a"));
        sm.record_request("b".to_string(), req("/b"));
        sm.annotate("a", Some("critical bug".to_string()), None);
        let results = sm.search_sessions("critical");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_no_match_returns_empty() {
        let sm = SessionManager::new(10_000);
        sm.record_request("a".to_string(), req("/api"));
        let results = sm.search_sessions("zzz_no_match");
        assert!(results.is_empty());
    }

    #[test]
    fn parse_search_query_tag_term() {
        let terms = parse_search_query("tag:auth");
        assert_eq!(terms.len(), 1);
        let mut ex = Exchange {
            id: "x".to_string(),
            timestamp: Utc::now(),
            updated_at: None,
            request: req("/x"),
            response: None,
            metrics: None,
            source: SessionSource::Proxy,
            ws_frames: vec![],
            note: None,
            tags: vec!["auth".to_string()],
            inspector_data: None,
        };
        assert!(terms[0].matches(&ex));
        ex.tags = vec![];
        assert!(!terms[0].matches(&ex));
    }

    #[test]
    fn import_sessions_preserves_existing_updated_at() {
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
            note: None,
            tags: vec![],
            inspector_data: None,
        };
        sm.import_sessions(vec![exchange]);
        let session = sm.get_session("imported").unwrap();
        assert_eq!(session.updated_at, Some(fixed_time));
    }
}
