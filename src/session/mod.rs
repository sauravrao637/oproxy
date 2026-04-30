use std::path::Path;
use indexmap::IndexMap;
use std::sync::{Arc, RwLock};
use chrono::{DateTime, Utc};
use tokio::sync::broadcast;
use crate::middleware::{RequestContext, ResponseContext};
use serde::{Serialize, Deserialize};

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
    pub ws_frames: Vec<WsFrame>,
}

pub struct SessionManager {
    exchanges: RwLock<IndexMap<String, Exchange>>,
    max_sessions: usize,
    // Fired whenever sessions change; SSE subscribers receive notifications.
    change_tx: broadcast::Sender<()>,
}

impl SessionManager {
    pub fn new(max_sessions: usize) -> Self {
        let (change_tx, _) = broadcast::channel(64);
        Self {
            exchanges: RwLock::new(IndexMap::new()),
            max_sessions,
            change_tx,
        }
    }

    /// Returns a broadcast receiver that fires on every session change.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }

    fn notify(&self) {
        let _ = self.change_tx.send(());
    }

    pub async fn save_to_file<P: AsRef<Path> + Send>(&self, path: P) -> Result<(), std::io::Error> {
        let json = {
            let guard = self.exchanges.read().unwrap();
            serde_json::to_string_pretty(&*guard)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
        };
        tokio::fs::write(path, json).await
    }

    pub async fn load_from_file<P: AsRef<Path> + Send>(&self, path: P) -> Result<(), std::io::Error> {
        let data = tokio::fs::read(path).await?;
        let exchanges: IndexMap<String, Exchange> = serde_json::from_slice(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        {
            let mut guard = self.exchanges.write().unwrap();
            *guard = exchanges;
        }
        self.notify();
        Ok(())
    }

    pub fn record_request(&self, id: String, request: RequestContext) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            if exchanges.len() >= self.max_sessions && !exchanges.contains_key(&id) {
                // Evict the oldest entry (insertion order ≈ arrival order).
                exchanges.shift_remove_index(0);
            }
            exchanges.insert(id.clone(), Exchange {
                id,
                timestamp: Utc::now(),
                updated_at: None,
                request,
                response: None,
                metrics: None,
                ws_frames: Vec::new(),
            });
        }
        self.notify();
    }

    pub fn record_response(&self, id: String, response: ResponseContext) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            if let Some(exchange) = exchanges.get_mut(&id) {
                exchange.response = Some(response);
                exchange.updated_at = Some(Utc::now());
            }
        }
        self.notify();
    }

    pub fn record_response_with_metrics(&self, id: String, response: ResponseContext, metrics: InspectionMetrics) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            if let Some(exchange) = exchanges.get_mut(&id) {
                exchange.response = Some(response);
                exchange.metrics = Some(metrics);
                exchange.updated_at = Some(Utc::now());
            }
        }
        self.notify();
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
        }
        self.notify();
    }

    pub fn append_ws_frame(&self, id: &str, frame: WsFrame) {
        {
            let mut exchanges = self.exchanges.write().unwrap();
            if let Some(exchange) = exchanges.get_mut(id) {
                exchange.ws_frames.push(frame);
            }
        }
        self.notify();
    }

    pub fn get_all_sessions(&self) -> Vec<Exchange> {
        let exchanges = self.exchanges.read().unwrap();
        exchanges.values().cloned().collect()
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
        self.notify();
    }
}

pub type SharedSessionManager = Arc<SessionManager>;

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
        assert!(all.iter().any(|e| e.id == "id-new"), "new session must be present");
    }

    #[test]
    fn subscribe_fires_on_record_request() {
        let sm = SessionManager::new(10_000);
        let mut rx = sm.subscribe();
        sm.record_request("id1".to_string(), req("/ping"));
        assert!(rx.try_recv().is_ok(), "subscriber should receive notification");
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
}
