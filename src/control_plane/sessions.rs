use axum::{
    extract::State,
    http::header,
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
};
use std::sync::Arc;
use std::time::Instant;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

use crate::AppState;
use crate::api::{
    SessionFileRequest, SessionListFilter, SessionListOptions, SessionSort, SessionSortDirection,
};
use crate::diff::diff_exchanges;

use super::metrics::record_endpoint_timing;
use super::storage_paths::{resolve_storage_file_for_read, resolve_storage_file_for_write};
use super::workspace::{SessionsViewState, SortDirection as WorkspaceSortDirection};

#[derive(serde::Deserialize, Default)]
pub(super) struct SessionQuery {
    since: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    q: Option<String>,
    regex: Option<bool>,
    methods: Option<String>,
    status_buckets: Option<String>,
    host_focus: Option<String>,
    /// Exact host filter. `host` is accepted as an alias for ergonomics.
    host_filter: Option<String>,
    host: Option<String>,
    sort_key: Option<String>,
    sort_dir: Option<String>,
    workspace_view: Option<String>,
    include_bodies: Option<bool>,
}

/// One member exchange within a connection, trimmed to what the stream tree needs.
#[derive(serde::Serialize)]
pub(super) struct ConnectionStream {
    id: String,
    stream_id: Option<u64>,
    method: String,
    host: String,
    path: String,
    status: u16,
    ts: String,
    /// Milliseconds from `connection.first_seen` to this stream's start — the
    /// x-offset for the concurrency timeline.
    start_offset_ms: i64,
    /// Stream duration in milliseconds (latency); the timeline bar width.
    duration_ms: u64,
}

/// Aggregated view of one downstream connection and the streams multiplexed on it.
#[derive(serde::Serialize)]
pub(super) struct ConnectionSummary {
    connection_id: String,
    downstream_protocol: Option<String>,
    /// Distinct origin hosts seen on this connection (usually one).
    hosts: Vec<String>,
    exchange_count: usize,
    /// Number of distinct stream ids observed (h1 ≈ exchange_count; h2/h3 may
    /// reuse the connection for many concurrent streams).
    stream_count: usize,
    first_seen: String,
    last_seen: String,
    /// Total wall-clock span of the connection in ms (timeline width).
    span_ms: i64,
    /// Peak number of streams in flight at the same instant — 1 for serial
    /// HTTP/1.1, higher when h2/h3 streams genuinely overlap.
    max_concurrency: usize,
    streams: Vec<ConnectionStream>,
}

/// `GET /api/connections` — groups recorded exchanges by `connection_id` so the
/// UI can render the connection → stream tree and h2/h3 multiplexing.
/// Exchanges without a captured connection id are omitted.
pub(super) async fn list_connections(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let started = Instant::now();
    let sessions = state.session_manager.get_all_sessions();
    let connections = aggregate_connections(sessions);

    record_endpoint_timing(
        &state.endpoint_metrics,
        "/api/connections",
        started,
        connections.len(),
    );
    axum::Json(serde_json::json!({
        "connections": connections,
        "total": connections.len(),
    }))
}

/// Pure aggregation of exchanges into per-connection summaries (most-recently
/// active first). Extracted from the handler so it is unit-testable.
pub(super) fn aggregate_connections(
    sessions: Vec<crate::session::Exchange>,
) -> Vec<ConnectionSummary> {
    // Preserve first-seen order of connections.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<crate::session::Exchange>> =
        std::collections::HashMap::new();
    for ex in sessions {
        let Some(cid) = ex.connection_id.clone() else {
            continue;
        };
        groups.entry(cid.clone()).or_default().push(ex);
        if !order.contains(&cid) {
            order.push(cid);
        }
    }

    let mut connections: Vec<ConnectionSummary> = Vec::with_capacity(order.len());
    for cid in order {
        let mut members = groups.remove(&cid).unwrap_or_default();
        members.sort_by_key(|e| e.stream_id.unwrap_or(0));

        let mut hosts: Vec<String> = Vec::new();
        let mut stream_ids: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut streams = Vec::with_capacity(members.len());
        let downstream_protocol = members.iter().find_map(|e| e.downstream_protocol.clone());
        let first_seen = members
            .iter()
            .map(|e| e.timestamp)
            .min()
            .unwrap_or_else(chrono::Utc::now);
        let last_seen = members
            .iter()
            .map(|e| e.updated_at.unwrap_or(e.timestamp))
            .max()
            .unwrap_or_else(chrono::Utc::now);

        // [start_ms, end_ms) intervals (relative to first_seen) for concurrency.
        let mut intervals: Vec<(i64, i64)> = Vec::with_capacity(members.len());
        for e in &members {
            if let Some(sid) = e.stream_id {
                stream_ids.insert(sid);
            }
            let host = e.request.host.clone();
            if !host.is_empty() && !hosts.contains(&host) {
                hosts.push(host.clone());
            }
            let (path, _) = e
                .request
                .uri
                .split_once('?')
                .unwrap_or((e.request.uri.as_str(), ""));
            let start_offset_ms = (e.timestamp - first_seen).num_milliseconds();
            let duration_ms = e.metrics.as_ref().map(|m| m.latency_ms).unwrap_or(0);
            intervals.push((start_offset_ms, start_offset_ms + duration_ms as i64));
            streams.push(ConnectionStream {
                id: e.id.clone(),
                stream_id: e.stream_id,
                method: e.request.method.clone(),
                host,
                path: path.to_string(),
                status: e
                    .metrics
                    .as_ref()
                    .map(|m| m.status_code)
                    .or_else(|| e.response.as_ref().map(|r| r.status))
                    .unwrap_or(0),
                ts: e.timestamp.to_rfc3339(),
                start_offset_ms,
                duration_ms,
            });
        }

        let span_ms = (last_seen - first_seen)
            .num_milliseconds()
            .max(intervals.iter().map(|(_, end)| *end).max().unwrap_or(0));
        let max_concurrency = peak_concurrency(&intervals);

        connections.push(ConnectionSummary {
            connection_id: cid,
            downstream_protocol,
            hosts,
            exchange_count: members.len(),
            stream_count: stream_ids.len(),
            first_seen: first_seen.to_rfc3339(),
            last_seen: last_seen.to_rfc3339(),
            span_ms,
            max_concurrency,
            streams,
        });
    }

    // Most recently active connections first.
    connections.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
    connections
}

/// A labelled count, serialised as `{ "label": .., "count": .. }`.
#[derive(serde::Serialize)]
pub(super) struct LabelCount {
    label: String,
    count: usize,
}

/// Aggregate protocol metrics over the whole session store.
#[derive(serde::Serialize, Default)]
pub(super) struct ProtocolMetrics {
    total_exchanges: usize,
    connections: usize,
    websockets: usize,
    grpc_calls: usize,
    /// Upstream (proxy→origin) protocol mix.
    protocol_mix: Vec<LabelCount>,
    /// Downstream (client→proxy) protocol mix.
    downstream_mix: Vec<LabelCount>,
    /// Application-level traffic family (HTTP / WebSocket / gRPC / tunnel).
    application_mix: Vec<LabelCount>,
    /// Capture source (live proxy, Compose/admin forward, replay, import).
    source_mix: Vec<LabelCount>,
    /// Status-class distribution (2xx/3xx/4xx/5xx/pending).
    status_classes: Vec<LabelCount>,
    /// gRPC status-code distribution (from captured `grpc-status`).
    grpc_status: Vec<LabelCount>,
    total_bytes: u64,
    latency_p50_ms: u64,
    latency_p95_ms: u64,
    latency_max_ms: u64,
}

/// `GET /api/metrics/protocol` — live protocol dashboard aggregates over the
/// recorded sessions. Cheap enough to recompute on demand; the UI polls / reacts
/// to the session SSE broadcast.
pub(super) async fn protocol_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let started = Instant::now();
    let sessions = state.session_manager.get_all_sessions();
    let metrics = aggregate_protocol_metrics(sessions);
    record_endpoint_timing(
        &state.endpoint_metrics,
        "/api/metrics/protocol",
        started,
        metrics.total_exchanges,
    );
    axum::Json(metrics)
}

/// Pure aggregation for [`protocol_metrics`], extracted for unit testing.
pub(super) fn aggregate_protocol_metrics(
    sessions: Vec<crate::session::Exchange>,
) -> ProtocolMetrics {
    use std::collections::{BTreeMap, HashSet};

    let mut m = ProtocolMetrics {
        total_exchanges: sessions.len(),
        ..Default::default()
    };
    let mut upstream: BTreeMap<String, usize> = BTreeMap::new();
    let mut downstream: BTreeMap<String, usize> = BTreeMap::new();
    let mut application: BTreeMap<String, usize> = BTreeMap::new();
    let mut source: BTreeMap<String, usize> = BTreeMap::new();
    let mut status: BTreeMap<String, usize> = BTreeMap::new();
    let mut grpc: BTreeMap<String, usize> = BTreeMap::new();
    let mut conns: HashSet<String> = HashSet::new();
    let mut latencies: Vec<u64> = Vec::with_capacity(sessions.len());

    for ex in &sessions {
        if let Some(cid) = &ex.connection_id {
            conns.insert(cid.clone());
        }
        if ex.request.method.eq_ignore_ascii_case("WS") {
            m.websockets += 1;
        }
        if let Some(g) = ex.inspector_data.as_ref().and_then(|i| i.grpc.as_ref()) {
            m.grpc_calls += 1;
            if let Some(s) = &g.grpc_status {
                *grpc.entry(s.clone()).or_default() += 1;
            }
        }
        if let Some(p) = ex.metrics.as_ref().and_then(|x| x.protocol.clone()) {
            *upstream.entry(p).or_default() += 1;
        }
        *downstream.entry(downstream_protocol_label(ex)).or_default() += 1;
        *application
            .entry(application_protocol_label(ex))
            .or_default() += 1;
        *source.entry(source_label(ex.source)).or_default() += 1;
        let code = ex
            .metrics
            .as_ref()
            .map(|x| x.status_code)
            .or_else(|| ex.response.as_ref().map(|r| r.status))
            .unwrap_or(0);
        let class = match code {
            0 => "pending",
            200..=299 => "2xx",
            300..=399 => "3xx",
            400..=499 => "4xx",
            500..=599 => "5xx",
            _ => "Others",
        };
        *status.entry(class.to_string()).or_default() += 1;
        if let Some(mx) = &ex.metrics {
            m.total_bytes += mx.request_size_bytes as u64 + mx.response_size_bytes as u64;
            latencies.push(mx.latency_ms);
        }
    }

    let to_sorted = |map: BTreeMap<String, usize>| -> Vec<LabelCount> {
        let mut v: Vec<LabelCount> = map
            .into_iter()
            .map(|(label, count)| LabelCount { label, count })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
        v
    };

    m.connections = conns.len();
    m.protocol_mix = to_sorted(upstream);
    m.downstream_mix = to_sorted(downstream);
    m.application_mix = to_sorted(application);
    m.source_mix = to_sorted(source);
    m.status_classes = to_sorted(status);
    m.grpc_status = to_sorted(grpc);

    latencies.sort_unstable();
    let pct = |p: f64| -> u64 {
        if latencies.is_empty() {
            return 0;
        }
        let idx = ((p * (latencies.len() as f64 - 1.0)).round() as usize).min(latencies.len() - 1);
        latencies[idx]
    };
    m.latency_p50_ms = pct(0.50);
    m.latency_p95_ms = pct(0.95);
    m.latency_max_ms = latencies.last().copied().unwrap_or(0);
    m
}

fn downstream_protocol_label(ex: &crate::session::Exchange) -> String {
    if let Some(ctx) = &ex.protocol_context {
        return ctx.downstream.label().to_string();
    }
    match ex.downstream_protocol.as_deref() {
        Some("admin") | None => ex
            .metrics
            .as_ref()
            .and_then(|m| m.protocol.clone())
            .unwrap_or_else(|| "HTTP/1.1".to_string()),
        Some(label) => label.to_string(),
    }
}

fn application_protocol_label(ex: &crate::session::Exchange) -> String {
    if let Some(ctx) = &ex.protocol_context {
        return match ctx.application {
            crate::core::forward::ApplicationProtocol::Grpc => "gRPC".to_string(),
            crate::core::forward::ApplicationProtocol::Http
                if matches!(
                    ctx.downstream,
                    crate::core::forward::WireProtocol::WebSocket
                ) =>
            {
                "WebSocket".to_string()
            }
            crate::core::forward::ApplicationProtocol::Http
                if matches!(ctx.body_mode, crate::core::forward::BodyMode::Tunnel) =>
            {
                "Tunnel".to_string()
            }
            _ => "HTTP".to_string(),
        };
    }
    if ex.request.method.eq_ignore_ascii_case("WS") {
        "WebSocket".to_string()
    } else if ex
        .inspector_data
        .as_ref()
        .and_then(|i| i.grpc.as_ref())
        .is_some()
    {
        "gRPC".to_string()
    } else if matches!(ex.downstream_protocol.as_deref(), Some("SOCKS5")) {
        "Tunnel".to_string()
    } else {
        "HTTP".to_string()
    }
}

fn source_label(source: crate::session::SessionSource) -> String {
    match source {
        crate::session::SessionSource::Proxy => "Proxy".to_string(),
        crate::session::SessionSource::AdminForward => "Compose".to_string(),
        crate::session::SessionSource::Playback => "Replay".to_string(),
        crate::session::SessionSource::Imported => "Imported".to_string(),
    }
}

/// Peak number of `[start, end)` intervals overlapping at any instant — the max
/// in-flight stream concurrency on a connection. A classic endpoint sweep:
/// +1 at each start, -1 at each end, tracking the running maximum. Equal
/// start/end ties resolve ends before starts so a zero-duration stream and the
/// next one don't count as overlapping.
fn peak_concurrency(intervals: &[(i64, i64)]) -> usize {
    let mut events: Vec<(i64, i8)> = Vec::with_capacity(intervals.len() * 2);
    for &(start, end) in intervals {
        events.push((start, 1));
        events.push((end.max(start), -1));
    }
    // Sort by time; at the same time, process ends (-1) before starts (+1).
    events.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut cur = 0i64;
    let mut peak = 0i64;
    for (_, delta) in events {
        cur += delta as i64;
        peak = peak.max(cur);
    }
    peak.max(0) as usize
}

pub(super) async fn list_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<SessionQuery>,
) -> impl IntoResponse {
    let started = Instant::now();
    let since = q
        .since
        .as_deref()
        .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());
    let mut options = SessionListOptions {
        since,
        limit: q.limit,
        offset: q.offset,
        include_bodies: q.include_bodies.unwrap_or(false),
        filter: SessionListFilter {
            query: q.q.unwrap_or_default(),
            regex: q.regex.unwrap_or(false),
            methods: q.methods.as_deref().map(split_csv),
            status_buckets: q.status_buckets.as_deref().map(split_csv),
            host_focus: q.host_focus.as_deref().map(split_csv).unwrap_or_default(),
            host_filter: q.host_filter.or(q.host).filter(|h| !h.trim().is_empty()),
            sort: SessionSort {
                key: q.sort_key.unwrap_or_else(|| "ts".to_string()),
                dir: parse_sort_dir(q.sort_dir.as_deref()),
            },
        },
    };
    if q.workspace_view.as_deref() == Some("current") {
        let workspace = state.workspace.read().await;
        options.filter = session_filter_from_workspace(&workspace.sessions_view);
    }
    let sessions = state.api_handler.list_sessions(options).await;
    record_endpoint_timing(
        &state.endpoint_metrics,
        "/api/sessions",
        started,
        sessions.total,
    );
    axum::Json(sessions)
}

fn session_filter_from_workspace(view: &SessionsViewState) -> SessionListFilter {
    let mut methods = view.methods.clone();
    if methods.iter().any(|m| m.eq_ignore_ascii_case("GET"))
        && !methods.iter().any(|m| m.eq_ignore_ascii_case("WS"))
    {
        methods.push("WS".to_string());
    }
    SessionListFilter {
        query: view.query.clone(),
        regex: view.regex,
        methods: Some(methods),
        status_buckets: Some(view.status_buckets.clone()),
        host_focus: view.host_focus.clone(),
        host_filter: view.host_filter.clone(),
        sort: SessionSort {
            key: view.sort.key.clone(),
            dir: match view.sort.dir {
                WorkspaceSortDirection::Asc => SessionSortDirection::Asc,
                WorkspaceSortDirection::Desc => SessionSortDirection::Desc,
            },
        },
    }
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_sort_dir(value: Option<&str>) -> SessionSortDirection {
    match value {
        Some("asc") => SessionSortDirection::Asc,
        _ => SessionSortDirection::Desc,
    }
}

/// Server-Sent Events stream: fires a `{"type":"update"}` event whenever
/// any session changes (new request, new response, clear). Clients subscribe
/// once and re-fetch sessions on each event rather than polling every 2 s.
pub(super) async fn sessions_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.session_manager.subscribe();
    let stream = BroadcastStream::new(rx).map(|result| {
        let data = match result {
            Ok(ref change) => {
                serde_json::to_string(change).unwrap_or_else(|_| r#"{"kind":"reload"}"#.to_string())
            }
            // Receiver lagged (broadcast buffer overflowed) — tell the client to reload.
            Err(_) => r#"{"kind":"reload"}"#.to_string(),
        };
        Ok::<_, std::convert::Infallible>(Event::default().data(data))
    });
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

pub(super) async fn get_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.api_handler.get_session_details(&id).await {
        Some(detail) => axum::Json(detail).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn get_ws_frames(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.session_manager.get_session(&id) {
        Some(exchange) => axum::Json(exchange.ws_frames).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(serde::Deserialize, Default)]
pub(super) struct ExportQuery {
    format: Option<String>,
    raw: Option<bool>,
}

pub(super) async fn export_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<ExportQuery>,
) -> impl IntoResponse {
    let exchange = match state.session_manager.get_session(&id) {
        Some(ex) => ex,
        None => return (axum::http::StatusCode::NOT_FOUND, "session not found").into_response(),
    };
    let format = q.format.as_deref().unwrap_or("curl");
    let raw = q.raw.unwrap_or(false);
    let (content_type, body) = match format {
        "fetch" if raw => (
            "application/javascript",
            crate::export::export_as_fetch_raw(&exchange),
        ),
        "fetch" => (
            "application/javascript",
            crate::export::export_as_fetch(&exchange),
        ),
        "python" if raw => (
            "text/x-python",
            crate::export::export_as_python_raw(&exchange),
        ),
        "python" => ("text/x-python", crate::export::export_as_python(&exchange)),
        _ if raw => ("text/plain", crate::export::export_as_curl_raw(&exchange)),
        _ => ("text/plain", crate::export::export_as_curl(&exchange)),
    };
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

pub(super) async fn get_session_timing(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let exchange = match state.session_manager.get_session(&id) {
        Some(ex) => ex,
        None => return (axum::http::StatusCode::NOT_FOUND, "session not found").into_response(),
    };

    let metrics = match &exchange.metrics {
        Some(m) => m.clone(),
        None => return axum::Json(serde_json::json!({ "available": false })).into_response(),
    };

    // Build waterfall phases in sequential order.
    // Each phase has: name, start_ms (offset from t=0), duration_ms.
    let mut phases = Vec::new();
    let mut cursor = 0u64;

    if let Some(dns) = metrics.dns_ms {
        phases.push(serde_json::json!({ "phase": "dns", "start": cursor, "duration": dns }));
        cursor += dns;
    }
    if let Some(tcp) = metrics.tcp_connect_ms {
        phases.push(serde_json::json!({ "phase": "tcp", "start": cursor, "duration": tcp }));
        cursor += tcp;
    }
    if let Some(tls) = metrics.tls_ms {
        phases.push(serde_json::json!({ "phase": "tls", "start": cursor, "duration": tls }));
        cursor += tls;
    }
    // ttfb covers wait time from after connection to first byte.
    let known_before_ttfb = cursor;
    let ttfb_wait = metrics.ttfb_ms.saturating_sub(known_before_ttfb);
    if ttfb_wait > 0 {
        phases.push(serde_json::json!({ "phase": "wait", "start": cursor, "duration": ttfb_wait }));
        cursor += ttfb_wait;
    }
    if metrics.body_ms > 0 {
        phases.push(
            serde_json::json!({ "phase": "body", "start": cursor, "duration": metrics.body_ms }),
        );
    }

    axum::Json(serde_json::json!({
        "available": true,
        "total_ms": metrics.latency_ms,
        "ttfb_ms": metrics.ttfb_ms,
        "body_ms": metrics.body_ms,
        "dns_ms": metrics.dns_ms,
        "tcp_connect_ms": metrics.tcp_connect_ms,
        "tls_ms": metrics.tls_ms,
        "status_code": metrics.status_code,
        "request_size_bytes": metrics.request_size_bytes,
        "response_size_bytes": metrics.response_size_bytes,
        "phases": phases,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub(super) struct DiffQuery {
    a: String,
    b: String,
}

pub(super) async fn diff_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<DiffQuery>,
) -> impl IntoResponse {
    let a = match state.session_manager.get_session(&q.a) {
        Some(ex) => ex,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                format!("session {} not found", q.a),
            )
                .into_response();
        }
    };
    let b = match state.session_manager.get_session(&q.b) {
        Some(ex) => ex,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                format!("session {} not found", q.b),
            )
                .into_response();
        }
    };
    axum::Json(diff_exchanges(&a, &b)).into_response()
}

#[derive(serde::Deserialize)]
pub(super) struct CurlImportBody {
    curl: String,
}

pub(super) async fn import_curl(
    axum::extract::Json(body): axum::extract::Json<CurlImportBody>,
) -> impl IntoResponse {
    match crate::export::parse_curl(&body.curl) {
        Ok(parsed) => axum::Json(serde_json::json!({
            "method": parsed.method,
            "url": parsed.url,
            "headers": parsed.headers,
            "body": parsed.body,
        }))
        .into_response(),
        Err(e) => (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize, Default)]
pub(super) struct AnnotationPatch {
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

pub(super) async fn annotate_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(patch): axum::extract::Json<AnnotationPatch>,
) -> impl IntoResponse {
    if state
        .api_handler
        .annotate_session(&id, patch.note, patch.tags)
        .await
    {
        axum::Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response()
    }
}

pub(super) async fn clear_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.api_handler.clear_sessions().await;
    axum::http::StatusCode::OK
}

pub(super) async fn save_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(req): axum::extract::Json<SessionFileRequest>,
) -> impl IntoResponse {
    let path = match resolve_storage_file_for_write(&state.storage_path, &req.path) {
        Ok(path) => path,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };
    match state
        .api_handler
        .save_session(path.to_string_lossy().to_string())
        .await
    {
        Ok(_) => axum::http::StatusCode::OK.into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

pub(super) async fn load_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(req): axum::extract::Json<SessionFileRequest>,
) -> impl IntoResponse {
    let path = match resolve_storage_file_for_read(&state.storage_path, &req.path) {
        Ok(path) => path,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };
    match state
        .api_handler
        .load_session(path.to_string_lossy().to_string())
        .await
    {
        Ok(_) => axum::http::StatusCode::OK.into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct ImportRequest {
    sessions: Vec<crate::session::Exchange>,
    #[serde(default = "bool_true")]
    merge: bool,
}

fn bool_true() -> bool {
    true
}

pub(super) async fn import_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(req): axum::extract::Json<ImportRequest>,
) -> impl IntoResponse {
    if !req.merge {
        state.session_manager.clear_sessions();
    }
    let count = req.sessions.len();
    state.session_manager.import_sessions(req.sessions);
    axum::Json(serde_json::json!({ "imported": count }))
}

#[derive(serde::Deserialize, Default)]
pub(super) struct HarExportQuery {
    raw: Option<bool>,
    ids: Option<String>,
}

pub(super) async fn export_har(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<HarExportQuery>,
) -> impl IntoResponse {
    let ids = q.ids.as_ref().map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(ToOwned::to_owned)
            .collect::<std::collections::HashSet<_>>()
    });
    let exchanges = {
        let guard = state.session_manager.get_all_sessions();
        let mut map = indexmap::IndexMap::new();
        for ex in guard {
            if ids.as_ref().is_some_and(|wanted| !wanted.contains(&ex.id)) {
                continue;
            }
            map.insert(ex.id.clone(), ex);
        }
        map
    };
    let har = if q.raw.unwrap_or(false) {
        crate::har::exchanges_to_har(&exchanges)
    } else {
        crate::har::exchanges_to_har_redacted(&exchanges)
    };
    match serde_json::to_string_pretty(&har) {
        Ok(json) => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"capture.har\"",
                ),
            ],
            json,
        )
            .into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct HarImportQuery {
    #[serde(default = "bool_true")]
    merge: bool,
}

pub(super) async fn import_har(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<HarImportQuery>,
    axum::extract::Json(har): axum::extract::Json<crate::har::Har>,
) -> impl IntoResponse {
    if !q.merge {
        state.session_manager.clear_sessions();
    }
    let exchanges: Vec<_> = crate::har::har_to_exchanges(&har);
    let count = exchanges.len();
    state.session_manager.import_sessions(exchanges);
    axum::Json(serde_json::json!({ "imported": count }))
}

#[cfg(test)]
mod tests {
    use super::{aggregate_connections, peak_concurrency};
    use crate::middleware::RequestContext;
    use crate::session::Exchange;

    #[test]
    fn peak_concurrency_serial_intervals_is_one() {
        // Back-to-back, non-overlapping streams (h1-style).
        assert_eq!(peak_concurrency(&[(0, 10), (10, 20), (20, 30)]), 1);
    }

    #[test]
    fn peak_concurrency_counts_overlap() {
        // Three streams all in flight 5..8 → peak 3.
        assert_eq!(peak_concurrency(&[(0, 10), (5, 15), (6, 8)]), 3);
    }

    #[test]
    fn peak_concurrency_empty_is_zero() {
        assert_eq!(peak_concurrency(&[]), 0);
    }

    #[test]
    fn protocol_metrics_counts_mix_status_and_latency() {
        use super::aggregate_protocol_metrics;
        use crate::session::InspectionMetrics;

        let mk = |id: &str, proto: &str, status: u16, latency: u64| {
            let mut e = ex(id, Some("c1"), Some(0), "api.test", "/p");
            e.metrics = Some(InspectionMetrics {
                status_code: status,
                latency_ms: latency,
                protocol: Some(proto.to_string()),
                response_size_bytes: 100,
                ..Default::default()
            });
            e
        };
        let m = aggregate_protocol_metrics(vec![
            mk("a", "HTTP/2", 200, 10),
            mk("b", "HTTP/2", 404, 20),
            mk("c", "HTTP/1.1", 200, 30),
        ]);

        assert_eq!(m.total_exchanges, 3);
        assert_eq!(m.connections, 1);
        // HTTP/2 is the most common upstream protocol → first.
        assert_eq!(m.protocol_mix[0].label, "HTTP/2");
        assert_eq!(m.protocol_mix[0].count, 2);
        let two_xx = m.status_classes.iter().find(|c| c.label == "2xx").unwrap();
        assert_eq!(two_xx.count, 2);
        let http_app = m
            .application_mix
            .iter()
            .find(|c| c.label == "HTTP")
            .unwrap();
        assert_eq!(http_app.count, 3);
        let proxy_source = m.source_mix.iter().find(|c| c.label == "Proxy").unwrap();
        assert_eq!(proxy_source.count, 3);
        assert_eq!(m.total_bytes, 300);
        assert_eq!(m.latency_max_ms, 30);
        assert_eq!(m.latency_p50_ms, 20);
    }

    #[test]
    fn protocol_metrics_keeps_source_out_of_downstream_protocol() {
        use super::aggregate_protocol_metrics;
        use crate::session::{InspectionMetrics, SessionSource};

        let mut e = ex("admin", Some("c1"), Some(0), "api.test", "/p");
        e.source = SessionSource::AdminForward;
        e.downstream_protocol = Some("admin".to_string());
        e.metrics = Some(InspectionMetrics {
            status_code: 200,
            protocol: Some("HTTP/2".to_string()),
            ..Default::default()
        });

        let m = aggregate_protocol_metrics(vec![e]);

        assert!(m.downstream_mix.iter().all(|c| c.label != "admin"));
        assert_eq!(m.downstream_mix[0].label, "HTTP/2");
        assert_eq!(m.source_mix[0].label, "Compose");
    }

    #[test]
    fn protocol_metrics_counts_websocket_exchanges() {
        use super::aggregate_protocol_metrics;

        let mut ws = ex("ws", Some("c1"), Some(0), "ws.test", "/socket");
        ws.request.method = "WS".to_string();

        let m = aggregate_protocol_metrics(vec![ws]);

        assert_eq!(m.websockets, 1);
    }

    fn ex(id: &str, conn: Option<&str>, stream: Option<u64>, host: &str, path: &str) -> Exchange {
        Exchange {
            id: id.to_string(),
            timestamp: chrono::Utc::now(),
            updated_at: None,
            request: RequestContext {
                method: "GET".to_string(),
                uri: path.to_string(),
                host: host.to_string(),
                ..Default::default()
            },
            response: None,
            metrics: None,
            source: Default::default(),
            ws_frames: vec![],
            events: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
            paused_at: None,
            connection_id: conn.map(str::to_string),
            stream_id: stream,
            downstream_protocol: Some("HTTP/2".to_string()),
            protocol_context: None,
        }
    }

    #[test]
    fn aggregate_groups_by_connection_and_omits_unidentified() {
        let connections = aggregate_connections(vec![
            ex("a", Some("c1"), Some(0), "api.test", "/a"),
            ex("b", Some("c1"), Some(1), "api.test", "/b"),
            ex("c", Some("c2"), Some(0), "cdn.test", "/c"),
            ex("d", None, None, "x.test", "/d"), // no connection id → omitted
        ]);

        assert_eq!(connections.len(), 2, "two identified connections");
        let c1 = connections
            .iter()
            .find(|c| c.connection_id == "c1")
            .expect("c1 present");
        assert_eq!(c1.exchange_count, 2);
        assert_eq!(c1.stream_count, 2);
        assert_eq!(c1.streams.len(), 2);
        assert_eq!(c1.downstream_protocol.as_deref(), Some("HTTP/2"));
        // Streams ordered by stream_id.
        assert_eq!(c1.streams[0].stream_id, Some(0));
        assert_eq!(c1.streams[1].stream_id, Some(1));
    }

    #[test]
    fn aggregate_dedups_hosts_and_counts_distinct_streams() {
        let connections = aggregate_connections(vec![
            ex("a", Some("c1"), Some(0), "api.test", "/a"),
            ex("b", Some("c1"), Some(0), "api.test", "/b"), // same stream id reused
        ]);
        let c1 = &connections[0];
        assert_eq!(c1.exchange_count, 2);
        assert_eq!(c1.stream_count, 1, "distinct stream ids");
        assert_eq!(c1.hosts, vec!["api.test".to_string()]);
    }
}
