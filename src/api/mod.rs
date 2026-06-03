use crate::core::playback::PlaybackEngine;
use crate::middleware::plugins::breakpoints::{
    BreakpointContext, BreakpointManager, BreakpointResolution, BreakpointRule, BreakpointType,
};
use crate::session::SharedSessionManager;
use crate::session::{Exchange, parse_search_query};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct SessionFileRequest {
    pub path: String,
}

#[derive(Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<Exchange>,
    pub total: usize,
    pub filtered_total: usize,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub facets: SessionListFacets,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SessionListFacets {
    pub methods: BTreeMap<String, usize>,
    pub status_buckets: BTreeMap<String, usize>,
    pub hosts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionListOptions {
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub include_bodies: bool,
    pub filter: SessionListFilter,
}

#[derive(Debug, Clone, Default)]
pub struct SessionListFilter {
    pub query: String,
    pub regex: bool,
    pub methods: Option<Vec<String>>,
    pub status_buckets: Option<Vec<String>>,
    pub host_focus: Vec<String>,
    pub host_filter: Option<String>,
    pub sort: SessionSort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSort {
    pub key: String,
    pub dir: SessionSortDirection,
}

impl Default for SessionSort {
    fn default() -> Self {
        Self {
            key: "ts".to_string(),
            dir: SessionSortDirection::Desc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSortDirection {
    Asc,
    Desc,
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
    breakpoint_manager: Arc<BreakpointManager>,
    playback_engine: PlaybackEngine,
}

impl ApiHandler {
    pub fn new(
        session_manager: SharedSessionManager,
        breakpoint_manager: Arc<BreakpointManager>,
        egress_policy: crate::security::AdminEgressPolicy,
    ) -> Self {
        let playback_engine = PlaybackEngine::new(session_manager.clone(), egress_policy);
        Self {
            session_manager,
            breakpoint_manager,
            playback_engine,
        }
    }

    pub async fn save_session(&self, path: String) -> Result<(), String> {
        self.session_manager
            .save_to_file(std::path::Path::new(&path))
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn load_session(&self, path: String) -> Result<(), String> {
        self.session_manager
            .load_from_file(std::path::Path::new(&path))
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn start_playback(&self) {
        let sessions = self.session_manager.get_all_sessions();
        self.playback_engine.replay(sessions).await;
    }

    /// List sessions with typed backend filtering, sorting, facets, and pagination.
    pub async fn list_sessions(&self, options: SessionListOptions) -> SessionListResponse {
        let all = self.session_manager.get_all_sessions();
        let total = all.len();
        let mut sessions: Vec<_> = all
            .into_iter()
            .filter(|session| matches_since(session, options.since))
            .filter(|session| matches_session_filter(session, &options.filter))
            .collect();
        let facets = build_facets(&sessions);
        let filtered_total = sessions.len();
        sort_sessions(&mut sessions, &options.filter.sort);

        let off = options.offset.unwrap_or(0);
        let mut paged: Vec<_> = if let Some(lim) = options.limit {
            sessions.into_iter().skip(off).take(lim).collect()
        } else {
            sessions.into_iter().skip(off).collect()
        };

        if !options.include_bodies {
            for exchange in &mut paged {
                exchange.request.body = bytes::Bytes::new();
                if let Some(response) = &mut exchange.response {
                    response.body = bytes::Bytes::new();
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
            filtered_total,
            limit: options.limit,
            offset: options.offset,
            facets,
        }
    }

    pub async fn get_session_details(&self, id: &str) -> Option<SessionDetailResponse> {
        self.session_manager
            .get_session(id)
            .map(|exchange| SessionDetailResponse { exchange })
    }

    pub async fn clear_sessions(&self) {
        self.session_manager.clear_sessions();
        self.session_manager.flush().await;
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

    pub async fn annotate_session(
        &self,
        id: &str,
        note: Option<String>,
        tags: Option<Vec<String>>,
    ) -> bool {
        self.session_manager.annotate(id, note, tags).await
    }
}

fn matches_since(session: &Exchange, since: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    match since {
        Some(since_dt) => {
            session.timestamp > since_dt
                || session.response.is_none()
                || session.updated_at.is_some_and(|t| t > since_dt)
        }
        None => true,
    }
}

fn matches_session_filter(session: &Exchange, filter: &SessionListFilter) -> bool {
    if let Some(methods) = &filter.methods
        && !methods
            .iter()
            .any(|method| session.request.method.eq_ignore_ascii_case(method))
    {
        return false;
    }

    if let Some(status_buckets) = &filter.status_buckets
        && !status_buckets
            .iter()
            .any(|bucket| status_bucket(session) == bucket.as_str())
    {
        return false;
    }

    if let Some(host_filter) = filter
        .host_filter
        .as_deref()
        .filter(|host| !host.is_empty())
        && session.request.host != host_filter
    {
        return false;
    }

    if !filter.host_focus.is_empty()
        && !filter.host_focus.iter().any(|host| {
            session.request.host == *host || session.request.host.ends_with(&format!(".{host}"))
        })
    {
        return false;
    }

    let query = filter.query.trim();
    if !query.is_empty() {
        if filter.regex {
            let Ok(re) = regex::Regex::new(query) else {
                return false;
            };
            if !re.is_match(&session_filter_haystack(session)) {
                return false;
            }
        } else {
            let terms = parse_search_query(query);
            if !terms.iter().all(|term| term.matches(session)) {
                return false;
            }
        }
    }

    true
}

fn sort_sessions(sessions: &mut [Exchange], sort: &SessionSort) {
    match sort.key.as_str() {
        "idx" | "ts" => sessions.sort_by_key(|session| session.timestamp),
        "method" => sessions.sort_by_key(|session| session.request.method.to_ascii_uppercase()),
        "status" => sessions.sort_by_key(response_status),
        "host" => sessions.sort_by_key(|session| session.request.host.to_ascii_lowercase()),
        "path" => sessions.sort_by_key(session_path),
        "type" => sessions.sort_by_key(session_kind),
        "reqSize" => sessions.sort_by_key(request_size),
        "total" => sessions.sort_by_key(session_latency),
        _ => sessions.sort_by_key(|session| session.timestamp),
    }

    if sort.dir == SessionSortDirection::Desc
        || (sort.key == "idx" && sort.dir == SessionSortDirection::Asc)
    {
        sessions.reverse();
    }
}

fn build_facets(sessions: &[Exchange]) -> SessionListFacets {
    let mut facets = SessionListFacets::default();
    for session in sessions {
        *facets
            .methods
            .entry(session.request.method.to_ascii_uppercase())
            .or_default() += 1;
        *facets
            .status_buckets
            .entry(status_bucket(session).to_string())
            .or_default() += 1;
        *facets
            .hosts
            .entry(session.request.host.clone())
            .or_default() += 1;
    }
    facets
}

fn status_bucket(session: &Exchange) -> &'static str {
    match response_status(session) {
        200..=299 => "2",
        300..=399 => "3",
        400..=499 => "4",
        500..=599 => "5",
        _ => "-",
    }
}

fn response_status(session: &Exchange) -> u16 {
    session
        .response
        .as_ref()
        .map(|response| response.status)
        .unwrap_or(0)
}

fn session_path(session: &Exchange) -> String {
    let uri = session.request.uri.as_str();
    let path = uri
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|idx| &rest[idx..]))
        .unwrap_or(uri);
    path.split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(path)
        .to_string()
}

fn session_kind(session: &Exchange) -> String {
    if session
        .inspector_data
        .as_ref()
        .and_then(|data| data.graphql.as_ref())
        .is_some()
    {
        "graphql".to_string()
    } else if session
        .inspector_data
        .as_ref()
        .and_then(|data| data.grpc.as_ref())
        .is_some()
    {
        "grpc".to_string()
    } else if session.response.is_some() {
        "http".to_string()
    } else {
        "pending".to_string()
    }
}

fn request_size(session: &Exchange) -> usize {
    session
        .metrics
        .as_ref()
        .map(|metrics| metrics.request_size_bytes)
        .unwrap_or_else(|| session.request.body.len())
}

fn session_latency(session: &Exchange) -> u64 {
    session
        .metrics
        .as_ref()
        .map(|metrics| metrics.latency_ms)
        .unwrap_or(0)
}

fn session_filter_haystack(session: &Exchange) -> String {
    format!(
        "{} {} {} {} {}",
        session.request.uri,
        session.request.method,
        session.request.host,
        session_kind(session),
        session.tags.join(" ")
    )
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
    use crate::middleware::{RequestContext, ResponseContext};
    use crate::session::SessionManager;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_handler() -> ApiHandler {
        let sm = Arc::new(SessionManager::new(10_000));
        ApiHandler::new(
            sm,
            Arc::new(BreakpointManager::new()),
            crate::security::AdminEgressPolicy::default(),
        )
    }

    fn req(uri: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: HashMap::new(),
            body: bytes::Bytes::new(),
            host: "localhost".to_string(),
            ..Default::default()
        }
    }

    fn req_with(method: &str, host: &str, uri: &str) -> RequestContext {
        RequestContext {
            method: method.to_string(),
            uri: uri.to_string(),
            headers: HashMap::new(),
            body: bytes::Bytes::new(),
            host: host.to_string(),
            ..Default::default()
        }
    }

    async fn list_test_sessions(
        h: &ApiHandler,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<usize>,
        offset: Option<usize>,
        q: Option<&str>,
        include_bodies: bool,
    ) -> SessionListResponse {
        h.list_sessions(SessionListOptions {
            since,
            limit,
            offset,
            include_bodies,
            filter: SessionListFilter {
                query: q.unwrap_or_default().to_string(),
                ..SessionListFilter::default()
            },
        })
        .await
    }

    // ── list_sessions: since filter ─────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_no_filter_returns_all() {
        let h = make_handler();
        h.session_manager.record_request("a".to_string(), req("/a"));
        h.session_manager.record_request("b".to_string(), req("/b"));
        h.session_manager.flush().await;
        let r = list_test_sessions(&h, None, None, None, None, true).await;
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
                body: bytes::Bytes::new(),
                request_uri: "/a".to_string(),
                ..Default::default()
            },
        );
        h.session_manager.flush().await;
        let future = chrono::Utc::now() + chrono::Duration::hours(1);
        let r = list_test_sessions(&h, Some(future), None, None, None, true).await;
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
        h.session_manager.flush().await;
        let past = chrono::Utc::now() - chrono::Duration::hours(1);
        let r = list_test_sessions(&h, Some(past), None, None, None, true).await;
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
        h.session_manager.flush().await;
        let r = list_test_sessions(&h, None, Some(2), None, None, true).await;
        assert_eq!(r.total, 5);
        assert_eq!(r.sessions.len(), 2);
        assert_eq!(r.limit, Some(2));
    }

    #[tokio::test]
    async fn list_sessions_can_return_bodyless_summaries() {
        let h = make_handler();
        let mut request = req("/large");
        request.body = bytes::Bytes::from_static(b"request-body");
        h.session_manager.record_request("id1".to_string(), request);
        h.session_manager.record_response(
            "id1".to_string(),
            ResponseContext {
                status: 200,
                headers: HashMap::new(),
                body: bytes::Bytes::from_static(b"response-body"),
                request_uri: "/large".to_string(),
                ..Default::default()
            },
        );
        h.session_manager.flush().await;

        let summary = list_test_sessions(&h, None, None, None, None, false).await;
        assert!(summary.sessions[0].request.body.is_empty());
        assert!(
            summary.sessions[0]
                .response
                .as_ref()
                .unwrap()
                .body
                .is_empty(),
            "list summaries must not ship full bodies"
        );

        let detail = h.get_session_details("id1").await.unwrap();
        assert_eq!(detail.exchange.request.body_text(), "request-body");
        assert_eq!(
            detail.exchange.response.unwrap().body_text(),
            "response-body"
        );
    }

    #[tokio::test]
    async fn list_sessions_offset_skips_entries() {
        let h = make_handler();
        for i in 0..5u32 {
            h.session_manager
                .record_request(format!("id-{i}"), req(&format!("/{i}")));
        }
        h.session_manager.flush().await;
        let r = list_test_sessions(&h, None, None, Some(3), None, true).await;
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
        h.session_manager.flush().await;
        let r = list_test_sessions(&h, None, Some(3), Some(4), None, true).await;
        assert_eq!(r.total, 10);
        assert_eq!(r.sessions.len(), 3);
    }

    #[tokio::test]
    async fn list_sessions_offset_beyond_end_returns_empty() {
        let h = make_handler();
        h.session_manager
            .record_request("id1".to_string(), req("/a"));
        h.session_manager.flush().await;
        let r = list_test_sessions(&h, None, None, Some(100), None, true).await;
        assert_eq!(r.total, 1);
        assert_eq!(r.sessions.len(), 0);
    }

    #[tokio::test]
    async fn list_sessions_applies_structured_filters_and_facets() {
        let h = make_handler();
        h.session_manager
            .record_request("ok".to_string(), req_with("GET", "api.test.com", "/ok"));
        h.session_manager.record_response(
            "ok".to_string(),
            ResponseContext {
                status: 200,
                headers: HashMap::new(),
                body: bytes::Bytes::new(),
                request_uri: "/ok".to_string(),
                ..Default::default()
            },
        );
        h.session_manager
            .record_request("bad".to_string(), req_with("POST", "api.test.com", "/bad"));
        h.session_manager.record_response(
            "bad".to_string(),
            ResponseContext {
                status: 500,
                headers: HashMap::new(),
                body: bytes::Bytes::new(),
                request_uri: "/bad".to_string(),
                ..Default::default()
            },
        );
        h.session_manager.record_request(
            "other".to_string(),
            req_with("POST", "other.test.com", "/bad"),
        );
        h.session_manager.flush().await;

        let r = h
            .list_sessions(SessionListOptions {
                include_bodies: true,
                filter: SessionListFilter {
                    methods: Some(vec!["POST".to_string()]),
                    status_buckets: Some(vec!["5".to_string()]),
                    host_focus: vec!["api.test.com".to_string()],
                    ..SessionListFilter::default()
                },
                ..SessionListOptions::default()
            })
            .await;

        assert_eq!(r.total, 3);
        assert_eq!(r.filtered_total, 1);
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].id, "bad");
        assert_eq!(r.facets.methods.get("POST"), Some(&1));
        assert_eq!(r.facets.status_buckets.get("5"), Some(&1));
        assert_eq!(r.facets.hosts.get("api.test.com"), Some(&1));
    }

    #[tokio::test]
    async fn list_sessions_sorts_on_backend() {
        let h = make_handler();
        h.session_manager
            .record_request("slow".to_string(), req_with("GET", "b.test.com", "/b"));
        h.session_manager.record_response_with_metrics(
            "slow".to_string(),
            ResponseContext {
                status: 200,
                headers: HashMap::new(),
                body: bytes::Bytes::new(),
                request_uri: "/b".to_string(),
                ..Default::default()
            },
            crate::session::InspectionMetrics {
                latency_ms: 100,
                ..Default::default()
            },
        );
        h.session_manager
            .record_request("fast".to_string(), req_with("GET", "a.test.com", "/a"));
        h.session_manager.record_response_with_metrics(
            "fast".to_string(),
            ResponseContext {
                status: 200,
                headers: HashMap::new(),
                body: bytes::Bytes::new(),
                request_uri: "/a".to_string(),
                ..Default::default()
            },
            crate::session::InspectionMetrics {
                latency_ms: 5,
                ..Default::default()
            },
        );
        h.session_manager.flush().await;

        let r = h
            .list_sessions(SessionListOptions {
                include_bodies: true,
                filter: SessionListFilter {
                    sort: SessionSort {
                        key: "total".to_string(),
                        dir: SessionSortDirection::Asc,
                    },
                    ..SessionListFilter::default()
                },
                ..SessionListOptions::default()
            })
            .await;

        assert_eq!(r.sessions[0].id, "fast");
        assert_eq!(r.sessions[1].id, "slow");
    }

    #[tokio::test]
    async fn list_sessions_distinguishes_missing_filter_from_empty_selection() {
        let h = make_handler();
        h.session_manager
            .record_request("id1".to_string(), req_with("GET", "api.test.com", "/a"));
        h.session_manager.flush().await;

        let unconstrained = h
            .list_sessions(SessionListOptions {
                include_bodies: true,
                filter: SessionListFilter {
                    methods: None,
                    ..SessionListFilter::default()
                },
                ..SessionListOptions::default()
            })
            .await;
        assert_eq!(unconstrained.filtered_total, 1);

        let empty_selection = h
            .list_sessions(SessionListOptions {
                include_bodies: true,
                filter: SessionListFilter {
                    methods: Some(vec![]),
                    ..SessionListFilter::default()
                },
                ..SessionListOptions::default()
            })
            .await;
        assert_eq!(empty_selection.filtered_total, 0);
    }

    // ── get_session_details ──────────────────────────────────────────────────

    #[tokio::test]
    async fn get_session_details_returns_some_for_known_id() {
        let h = make_handler();
        h.session_manager
            .record_request("x".to_string(), req("/detail"));
        h.session_manager.flush().await;
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
        h.clear_sessions().await; // already flushes internally
        let r = list_test_sessions(&h, None, None, None, None, true).await;
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
