use crate::core::playback::PlaybackEngine;
use crate::middleware::plugins::breakpoints::{
    BreakpointContext, BreakpointManager, BreakpointResolution, BreakpointRule, BreakpointType,
};
use crate::middleware::plugins::header_map::{HeaderMapMiddleware, HeaderMapRule};
use crate::middleware::plugins::modification::{ModificationMiddleware, ModificationRule};
use crate::middleware::plugins::rewrite::{RewriteMiddleware, RewriteRule};
use crate::session::Exchange;
use crate::session::SharedSessionManager;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct SessionFileRequest {
    pub path: String,
}

#[derive(Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<Exchange>,
    pub total: usize,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Serialize)]
pub struct SessionDetailResponse {
    pub exchange: Exchange,
}

#[derive(Serialize)]
pub struct PendingBreakpointInfo {
    pub id: String,
    pub bp_type: BreakpointType,
    pub context: BreakpointContext,
}

pub struct ApiHandler {
    pub session_manager: SharedSessionManager,
    rewrite_middleware: Arc<RewriteMiddleware>,
    breakpoint_manager: Arc<BreakpointManager>,
    header_map_middleware: Arc<HeaderMapMiddleware>,
    modification_middleware: Arc<ModificationMiddleware>,
    playback_engine: PlaybackEngine,
}

impl ApiHandler {
    pub fn new(
        session_manager: SharedSessionManager,
        rewrite_middleware: Arc<RewriteMiddleware>,
        breakpoint_manager: Arc<BreakpointManager>,
        header_map_middleware: Arc<HeaderMapMiddleware>,
        modification_middleware: Arc<ModificationMiddleware>,
        egress_policy: crate::security::AdminEgressPolicy,
    ) -> Self {
        let playback_engine = PlaybackEngine::new(session_manager.clone(), egress_policy);
        Self {
            session_manager,
            rewrite_middleware,
            breakpoint_manager,
            header_map_middleware,
            modification_middleware,
            playback_engine,
        }
    }

    pub async fn save_session(&self, path: String) -> Result<(), String> {
        self.session_manager
            .save_to_file(path)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn load_session(&self, path: String) -> Result<(), String> {
        self.session_manager
            .load_from_file(path)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn start_playback(&self) {
        let sessions = self.session_manager.get_all_sessions();
        self.playback_engine.replay(sessions).await;
    }

    /// List sessions with optional full-text search, timestamp filter, and pagination.
    /// `q` — full-text search (supports `tag:x`, `host:x`, `method:GET`, `status:200`).
    /// `since` — return only sessions newer than this timestamp.
    /// `limit` / `offset` — pagination (applied after filtering).
    pub async fn list_sessions(
        &self,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<usize>,
        offset: Option<usize>,
        q: Option<&str>,
        include_bodies: bool,
    ) -> SessionListResponse {
        let all = match q {
            Some(query) if !query.trim().is_empty() => self.session_manager.search_sessions(query),
            _ => self.session_manager.get_all_sessions(),
        };
        let total = all.len();
        let mut sessions: Vec<_> = if let Some(since_dt) = since {
            all.into_iter()
                .filter(|e| {
                    e.timestamp > since_dt
                        || e.response.is_none()
                        || e.updated_at.is_some_and(|t| t > since_dt)
                })
                .collect()
        } else {
            all
        };
        sessions.sort_unstable_by_key(|session| std::cmp::Reverse(session.timestamp));

        let off = offset.unwrap_or(0);
        let mut paged: Vec<_> = if let Some(lim) = limit {
            sessions.into_iter().skip(off).take(lim).collect()
        } else {
            sessions.into_iter().skip(off).collect()
        };

        if !include_bodies {
            for exchange in &mut paged {
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

        SessionListResponse {
            sessions: paged,
            total,
            limit,
            offset,
        }
    }

    pub async fn get_session_details(&self, id: &str) -> Option<SessionDetailResponse> {
        self.session_manager
            .get_session(id)
            .map(|exchange| SessionDetailResponse { exchange })
    }

    pub async fn clear_sessions(&self) {
        self.session_manager.clear_sessions();
    }

    pub async fn add_rewrite_rule(&self, rule: RewriteRule) {
        let mut rules = self.rewrite_middleware.rules.write().await;
        rules.push(rule);
    }

    pub async fn list_rewrite_rules(&self) -> Vec<RewriteRule> {
        self.rewrite_middleware.rules.read().await.clone()
    }

    pub async fn delete_rewrite_rule(&self, index: usize) {
        let mut rules = self.rewrite_middleware.rules.write().await;
        if index < rules.len() {
            rules.remove(index);
        }
    }

    pub async fn update_rewrite_rule(&self, index: usize, rule: RewriteRule) {
        let mut rules = self.rewrite_middleware.rules.write().await;
        if index < rules.len() {
            rules[index] = rule;
        }
    }

    pub async fn replace_all_rewrite_rules(&self, new_rules: Vec<RewriteRule>) {
        let mut rules = self.rewrite_middleware.rules.write().await;
        *rules = new_rules;
    }

    pub async fn list_header_maps(&self) -> Vec<HeaderMapRule> {
        self.header_map_middleware.rules.read().await.clone()
    }

    pub async fn add_header_map(&self, rule: HeaderMapRule) {
        let mut rules = self.header_map_middleware.rules.write().await;
        rules.push(rule);
    }

    pub async fn update_header_map(&self, id: &str, updated: HeaderMapRule) {
        let mut rules = self.header_map_middleware.rules.write().await;
        if let Some(r) = rules.iter_mut().find(|r| r.id == id) {
            *r = updated;
        }
    }

    pub async fn delete_header_map(&self, id: &str) {
        let mut rules = self.header_map_middleware.rules.write().await;
        rules.retain(|r| r.id != id);
    }

    pub async fn resolve_breakpoint(
        &self,
        id: String,
        resolution: BreakpointResolution,
    ) -> Result<(), String> {
        self.breakpoint_manager
            .resolve_breakpoint(&id, resolution)
            .await
    }

    pub async fn list_breakpoint_rules(&self) -> Vec<BreakpointRule> {
        self.breakpoint_manager.list_rules().await
    }

    pub async fn add_breakpoint_rule(&self, rule: BreakpointRule) {
        self.breakpoint_manager.add_rule(rule).await;
    }

    pub async fn delete_breakpoint_rule(&self, id: &str) {
        self.breakpoint_manager.delete_rule(id).await;
    }

    pub async fn update_breakpoint_rule(&self, id: &str, rule: BreakpointRule) -> bool {
        self.breakpoint_manager.update_rule(id, rule).await
    }

    pub async fn list_pending(&self) -> Vec<PendingBreakpointInfo> {
        let pending = self.breakpoint_manager.pending.read().await;
        pending
            .values()
            .map(|bp| PendingBreakpointInfo {
                id: bp.id.clone(),
                bp_type: bp.bp_type.clone(),
                context: bp.context.clone(),
            })
            .collect()
    }

    pub async fn list_modifications(&self) -> Vec<ModificationRule> {
        self.modification_middleware.rules.read().await.clone()
    }

    pub async fn add_modification(&self, rule: ModificationRule) {
        self.modification_middleware.rules.write().await.push(rule);
    }

    pub async fn delete_modification(&self, index: usize) {
        let mut rules = self.modification_middleware.rules.write().await;
        if index < rules.len() {
            rules.remove(index);
        }
    }

    pub async fn annotate_session(
        &self,
        id: &str,
        note: Option<String>,
        tags: Option<Vec<String>>,
    ) -> bool {
        self.session_manager.annotate(id, note, tags)
    }
}

/// Pretty-print a body string based on its content-type.
/// Returns the original string unchanged if it cannot be pretty-printed.
#[cfg_attr(not(test), allow(dead_code))]
pub fn pretty_body(body: &str, content_type: &str) -> String {
    if (content_type.contains("application/json") || content_type.contains("/json"))
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
        && let Ok(s) = serde_json::to_string_pretty(&v)
    {
        return s;
    }
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::plugins::breakpoints::BreakpointManager;
    use crate::middleware::plugins::header_map::HeaderMapMiddleware;
    use crate::middleware::plugins::modification::ModificationMiddleware;
    use crate::middleware::plugins::rewrite::RewriteMiddleware;
    use crate::middleware::{RequestContext, ResponseContext};
    use crate::session::SessionManager;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_handler() -> ApiHandler {
        let sm = Arc::new(SessionManager::new(10_000));
        ApiHandler::new(
            sm,
            Arc::new(RewriteMiddleware::new(vec![])),
            Arc::new(BreakpointManager::new()),
            Arc::new(HeaderMapMiddleware::new(vec![])),
            Arc::new(ModificationMiddleware::new(vec![])),
            crate::security::AdminEgressPolicy::default(),
        )
    }

    fn req(uri: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: HashMap::new(),
            body: String::new(),
            host: "localhost".to_string(),
            body_bytes: None,
        }
    }

    // ── list_sessions: since filter ─────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_no_filter_returns_all() {
        let h = make_handler();
        h.session_manager.record_request("a".to_string(), req("/a"));
        h.session_manager.record_request("b".to_string(), req("/b"));
        let r = h.list_sessions(None, None, None, None, true).await;
        assert_eq!(r.total, 2);
        assert_eq!(r.sessions.len(), 2);
    }

    #[tokio::test]
    async fn list_sessions_since_future_excludes_completed_sessions() {
        let h = make_handler();
        h.session_manager.record_request("a".to_string(), req("/a"));
        // Attach a response so the session is "completed" — pending sessions always pass since filter.
        h.session_manager.record_response(
            "a".to_string(),
            ResponseContext {
                status: 200,
                headers: HashMap::new(),
                body: String::new(),
                request_uri: "/a".to_string(),
                session_id: None,
                ttfb_ms: 0,
                body_ms: 0,
                body_bytes: None,
            },
        );
        let future = chrono::Utc::now() + chrono::Duration::hours(1);
        let r = h.list_sessions(Some(future), None, None, None, true).await;
        assert_eq!(r.total, 1);
        assert_eq!(
            r.sessions.len(),
            0,
            "completed session older than since must be excluded"
        );
    }

    #[tokio::test]
    async fn list_sessions_since_past_returns_all() {
        let h = make_handler();
        h.session_manager.record_request("a".to_string(), req("/a"));
        let past = chrono::Utc::now() - chrono::Duration::hours(1);
        let r = h.list_sessions(Some(past), None, None, None, true).await;
        assert_eq!(r.sessions.len(), 1);
    }

    // ── list_sessions: pagination ───────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_limit_caps_results() {
        let h = make_handler();
        for i in 0..5u32 {
            h.session_manager
                .record_request(format!("id-{i}"), req(&format!("/{i}")));
        }
        let r = h.list_sessions(None, Some(2), None, None, true).await;
        assert_eq!(r.total, 5);
        assert_eq!(r.sessions.len(), 2);
        assert_eq!(r.limit, Some(2));
    }

    #[tokio::test]
    async fn list_sessions_can_return_bodyless_summaries() {
        let h = make_handler();
        let mut request = req("/large");
        request.body = "request-body".to_string();
        h.session_manager.record_request("id1".to_string(), request);
        h.session_manager.record_response(
            "id1".to_string(),
            ResponseContext {
                status: 200,
                headers: HashMap::new(),
                body: "response-body".to_string(),
                request_uri: "/large".to_string(),
                session_id: None,
                ttfb_ms: 0,
                body_ms: 0,
                body_bytes: None,
            },
        );

        let summary = h.list_sessions(None, None, None, None, false).await;
        assert_eq!(summary.sessions[0].request.body, "");
        assert_eq!(
            summary.sessions[0].response.as_ref().unwrap().body,
            "",
            "list summaries must not ship full bodies"
        );

        let detail = h.get_session_details("id1").await.unwrap();
        assert_eq!(detail.exchange.request.body, "request-body");
        assert_eq!(detail.exchange.response.unwrap().body, "response-body");
    }

    #[tokio::test]
    async fn list_sessions_offset_skips_entries() {
        let h = make_handler();
        for i in 0..5u32 {
            h.session_manager
                .record_request(format!("id-{i}"), req(&format!("/{i}")));
        }
        let r = h.list_sessions(None, None, Some(3), None, true).await;
        assert_eq!(r.total, 5);
        assert_eq!(r.sessions.len(), 2); // 5 - skip 3
        assert_eq!(r.offset, Some(3));
    }

    #[tokio::test]
    async fn list_sessions_limit_and_offset() {
        let h = make_handler();
        for i in 0..10u32 {
            h.session_manager
                .record_request(format!("id-{i}"), req(&format!("/{i}")));
        }
        let r = h.list_sessions(None, Some(3), Some(4), None, true).await;
        assert_eq!(r.total, 10);
        assert_eq!(r.sessions.len(), 3);
    }

    #[tokio::test]
    async fn list_sessions_offset_beyond_end_returns_empty() {
        let h = make_handler();
        h.session_manager
            .record_request("id1".to_string(), req("/a"));
        let r = h.list_sessions(None, None, Some(100), None, true).await;
        assert_eq!(r.total, 1);
        assert_eq!(r.sessions.len(), 0);
    }

    // ── get_session_details ──────────────────────────────────────────────────

    #[tokio::test]
    async fn get_session_details_returns_some_for_known_id() {
        let h = make_handler();
        h.session_manager
            .record_request("x".to_string(), req("/detail"));
        let detail = h.get_session_details("x").await;
        assert!(detail.is_some());
        assert_eq!(detail.unwrap().exchange.request.uri, "/detail");
    }

    #[tokio::test]
    async fn get_session_details_returns_none_for_unknown_id() {
        let h = make_handler();
        assert!(h.get_session_details("ghost").await.is_none());
    }

    // ── clear_sessions ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn clear_sessions_empties_all() {
        let h = make_handler();
        h.session_manager.record_request("a".to_string(), req("/a"));
        h.session_manager.record_request("b".to_string(), req("/b"));
        h.clear_sessions().await;
        let r = h.list_sessions(None, None, None, None, true).await;
        assert_eq!(r.total, 0);
    }

    // ── pretty_body ──────────────────────────────────────────────────────────

    #[test]
    fn pretty_body_formats_json() {
        let raw = r#"{"b":2,"a":1}"#;
        let out = pretty_body(raw, "application/json");
        assert!(out.contains('\n'), "pretty JSON must be multi-line");
        assert!(
            out.contains("\"a\": 1") || out.contains("\"b\": 2"),
            "keys must be present"
        );
    }

    #[test]
    fn pretty_body_json_content_type_with_charset() {
        let raw = r#"{"ok":true}"#;
        let out = pretty_body(raw, "application/json; charset=utf-8");
        assert!(out.contains('\n'));
    }

    #[test]
    fn pretty_body_non_json_content_type_returns_unchanged() {
        let raw = "plain text body";
        let out = pretty_body(raw, "text/plain");
        assert_eq!(out, raw);
    }

    #[test]
    fn pretty_body_malformed_json_returns_unchanged() {
        let raw = r#"{"incomplete:"#;
        let out = pretty_body(raw, "application/json");
        assert_eq!(out, raw);
    }

    #[test]
    fn pretty_body_empty_body_returns_empty() {
        let out = pretty_body("", "application/json");
        assert_eq!(out, "");
    }

    #[test]
    fn pretty_body_vendor_json_type_returned_unchanged() {
        // pretty_body only matches "application/json" and types containing "/json".
        // "application/vnd.api+json" contains "+json" not "/json", so it falls through.
        let raw = r#"{"x":1}"#;
        let out = pretty_body(raw, "application/vnd.api+json");
        assert_eq!(
            out, raw,
            "vendor +json types are not pretty-printed by current impl"
        );
    }
}
