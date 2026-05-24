use axum::response::sse::{Event, Sse};
use axum::{
    Router,
    extract::State,
    http::header,
    middleware::Next,
    response::{Html, IntoResponse},
    routing::get,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

#[derive(serde::Deserialize, Default)]
struct SessionQuery {
    since: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    q: Option<String>,
    include_bodies: Option<bool>,
}
use crate::AppState;
use crate::api::SessionFileRequest;
use crate::core::engine::is_binary_content_type;
use crate::diff::diff_exchanges;
use crate::middleware::plugins::breakpoints::{
    BreakpointContext, BreakpointResolution, BreakpointRule,
};
use crate::middleware::plugins::capture_filter::CaptureFilterConfig;
use crate::middleware::plugins::lua_engine::LuaScript;
use crate::middleware::plugins::mock::MockRule;
use crate::middleware::plugins::modification::ModificationRule;
use crate::middleware::plugins::rewrite::RewriteRule;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::middleware::{RequestContext, ResponseContext};
use crate::session::SessionSource;
use crate::storage;
use crate::webhooks::{WebhookConfig, sanitize_webhook_events};
use base64::Engine as _;

mod design_assets {
    include!(concat!(env!("OUT_DIR"), "/design_assets.rs"));
}

const ENDPOINT_TIMING_LIMIT: usize = 64;

pub(crate) type SharedEndpointMetrics = Arc<std::sync::Mutex<EndpointMetrics>>;

#[derive(Debug, Clone)]
struct EndpointTimingSample {
    endpoint: &'static str,
    duration_ms: u64,
    session_count: usize,
    timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Default)]
pub(crate) struct EndpointMetrics {
    samples: VecDeque<EndpointTimingSample>,
}

pub(crate) fn new_endpoint_metrics() -> SharedEndpointMetrics {
    Arc::new(std::sync::Mutex::new(EndpointMetrics::default()))
}

impl EndpointMetrics {
    fn record(&mut self, endpoint: &'static str, elapsed: Duration, session_count: usize) {
        if self.samples.len() >= ENDPOINT_TIMING_LIMIT {
            self.samples.pop_front();
        }
        self.samples.push_back(EndpointTimingSample {
            endpoint,
            duration_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
            session_count,
            timestamp: chrono::Utc::now(),
        });
    }

    fn payload(&self) -> serde_json::Value {
        let mut grouped: BTreeMap<&'static str, Vec<&EndpointTimingSample>> = BTreeMap::new();
        for sample in &self.samples {
            grouped.entry(sample.endpoint).or_default().push(sample);
        }

        let summaries: BTreeMap<_, _> = grouped
            .into_iter()
            .map(|(endpoint, samples)| {
                let total: u64 = samples.iter().map(|sample| sample.duration_ms).sum();
                let max = samples
                    .iter()
                    .map(|sample| sample.duration_ms)
                    .max()
                    .unwrap_or(0);
                let last = samples.last().copied();
                (
                    endpoint,
                    serde_json::json!({
                        "samples": samples.len(),
                        "last_ms": last.map(|sample| sample.duration_ms).unwrap_or(0),
                        "avg_ms": if samples.is_empty() { 0 } else { total / samples.len() as u64 },
                        "max_ms": max,
                        "last_session_count": last.map(|sample| sample.session_count).unwrap_or(0),
                    }),
                )
            })
            .collect();

        let recent: Vec<_> = self
            .samples
            .iter()
            .rev()
            .take(12)
            .map(|sample| {
                serde_json::json!({
                    "endpoint": sample.endpoint,
                    "duration_ms": sample.duration_ms,
                    "session_count": sample.session_count,
                    "timestamp": sample.timestamp.to_rfc3339(),
                })
            })
            .collect();

        serde_json::json!({
            "sample_limit": ENDPOINT_TIMING_LIMIT,
            "summaries": summaries,
            "recent": recent,
        })
    }
}

fn record_endpoint_timing(
    metrics: &SharedEndpointMetrics,
    endpoint: &'static str,
    started: Instant,
    session_count: usize,
) {
    if let Ok(mut guard) = metrics.lock() {
        guard.record(endpoint, started.elapsed(), session_count);
    }
}

fn endpoint_timing_payload(metrics: &SharedEndpointMetrics) -> serde_json::Value {
    metrics
        .lock()
        .map(|guard| guard.payload())
        .unwrap_or_else(|_| serde_json::json!({ "error": "endpoint metrics unavailable" }))
}

/// Builds the management router: UI, admin API, static assets, and proxy fallback.
/// The caller is responsible for applying the proxy-dispatch layer on top.
pub fn management_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/health", get(health))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/stream", get(sessions_stream))
        .route("/api/sessions/{id}", get(get_session))
        .route(
            "/api/sessions/{id}/annotation",
            axum::routing::patch(annotate_session),
        )
        .route("/api/sessions/{id}/export", get(export_session))
        .route("/api/sessions/{id}/timing", get(get_session_timing))
        .route("/api/sessions/diff", get(diff_sessions))
        .route("/api/sessions/{id}/ws-frames", get(get_ws_frames))
        .route("/api/import/curl", axum::routing::post(import_curl))
        .route("/admin/routes", get(list_routes).post(update_routes))
        .route("/admin/sessions", get(list_sessions).delete(clear_sessions))
        .route("/admin/sessions/save", axum::routing::post(save_sessions))
        .route("/admin/sessions/load", axum::routing::post(load_sessions))
        .route(
            "/admin/sessions/import",
            axum::routing::post(import_sessions),
        )
        .route("/admin/sessions/export/har", get(export_har))
        .route(
            "/admin/sessions/import/har",
            axum::routing::post(import_har),
        )
        .route(
            "/admin/throttling",
            get(get_throttling).post(update_throttling),
        )
        .route("/admin/rewrites", get(list_rewrites).post(add_rewrite))
        .route(
            "/admin/rewrites/replace-all",
            axum::routing::post(replace_all_rewrites),
        )
        .route(
            "/admin/rewrites/{index}",
            axum::routing::delete(delete_rewrite).put(update_rewrite),
        )
        .route(
            "/admin/header-maps",
            get(list_header_maps).post(add_header_map),
        )
        .route(
            "/admin/header-maps/{id}",
            axum::routing::put(update_header_map).delete(delete_header_map),
        )
        .route("/admin/ca", get(get_ca_cert))
        .route("/admin/metrics", get(get_metrics))
        .route("/admin/playback", axum::routing::post(start_playback))
        .route("/admin/breakpoints", get(list_bp_rules).post(add_bp_rule))
        .route("/admin/breakpoints/pending", get(list_pending_bp))
        .route(
            "/admin/breakpoints/pending/{id}/resolve",
            axum::routing::post(resolve_bp),
        )
        .route(
            "/admin/breakpoints/{id}",
            axum::routing::put(update_bp_rule).delete(delete_bp_rule),
        )
        .route("/admin/plugins", get(list_plugins))
        .route("/admin/config/reload", axum::routing::post(reload_config))
        .route("/admin/config", get(get_config))
        .route(
            "/admin/modifications",
            get(list_modifications).post(add_modification),
        )
        .route(
            "/admin/modifications/{index}",
            axum::routing::delete(delete_modification),
        )
        .route(
            "/admin/capture-filter",
            get(get_capture_filter).post(update_capture_filter),
        )
        .route("/admin/dns", get(list_dns).post(update_dns))
        .route("/admin/dns/{host}", axum::routing::delete(delete_dns))
        .route("/admin/map-local", get(list_map_local).post(set_map_local))
        .route(
            "/admin/map-local/{host}",
            axum::routing::delete(delete_map_local),
        )
        .route("/admin/forward", axum::routing::post(forward_request))
        .route(
            "/admin/upstream-proxy",
            get(get_upstream_proxy).post(set_upstream_proxy_handler),
        )
        .route("/admin/webhooks", get(list_webhooks).post(create_webhook))
        .route(
            "/admin/webhooks/{id}",
            axum::routing::put(update_webhook).delete(delete_webhook),
        )
        .route(
            "/admin/mock/rules",
            get(list_mock_rules).post(create_mock_rule),
        )
        .route(
            "/admin/mock/rules/{id}",
            axum::routing::put(update_mock_rule).delete(delete_mock_rule),
        )
        .route(
            "/admin/mock/rules/{id}/reset",
            axum::routing::post(reset_mock_rule),
        )
        .route("/admin/scripts", get(list_scripts).post(create_script))
        .route(
            "/admin/scripts/{id}",
            axum::routing::put(update_script).delete(delete_script),
        )
        .route("/admin/socks5/status", get(get_socks5_status))
        .route("/setup", get(serve_setup_wizard))
        .route("/setup/mobile", get(serve_setup_wizard))
        .route("/admin/setup/network-info", get(get_network_info))
        .route("/manifest.json", get(serve_manifest))
        .route("/sw.js", get(serve_sw))
        .route("/icons/icon.svg", get(serve_icon))
        .route("/data.js", get(not_found))
        .route("/composer.jsx", get(not_found))
        .route("/styles.css", get(not_found))
        .route("/tweaks-panel.jsx", get(not_found))
        .route("/icons.jsx", get(not_found))
        .route("/sessions-table.jsx", get(not_found))
        .route("/detail-panel.jsx", get(not_found))
        .route("/surfaces.jsx", get(not_found))
        .route("/surfaces-extra.jsx", get(not_found))
        .route("/compose.jsx", get(not_found))
        .route("/app.jsx", get(not_found))
        .route("/assets/app.css", get(serve_design_app_css))
        .route("/assets/app.js", get(serve_design_app_js))
        .route("/app.css", get(not_found))
        .route("/js/{*path}", get(not_found))
        // Silence browser probes that would otherwise reach the proxy fallback
        .route("/favicon.ico", get(serve_icon))
        .route("/.well-known/{*path}", get(not_found))
        .route("/robots.txt", get(robots_txt))
        .fallback(proxy_fallback)
        .layer(axum::middleware::from_fn(security_headers))
        .with_state(state)
}

// ── Security helpers ───────────────────────────────────────────────────────────

async fn security_headers(req: axum::extract::Request, next: Next) -> axum::response::Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", header::HeaderValue::from_static("nosniff"));
    headers.insert("x-frame-options", header::HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", header::HeaderValue::from_static("no-referrer"));
    headers.insert(
        "content-security-policy",
        header::HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'unsafe-inline'; \
             style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
             connect-src 'self'; font-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    response
}

async fn robots_txt() -> impl IntoResponse {
    (
        [("content-type", "text/plain")],
        "User-agent: *\nDisallow: /\n",
    )
}

// ── Proxy dispatch ─────────────────────────────────────────────────────────────

/// Tower layer applied before route matching. Requests whose Host is not a
/// configured admin host go straight to the proxy engine so management routes
/// (like GET /) are never accidentally served to proxied traffic.
pub async fn proxy_dispatch_layer(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let is_admin_host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .map(|h| is_management_host(h, &state.config.bind_host))
        .unwrap_or(true); // no Host header → direct connection, treat as local

    if is_admin_host {
        next.run(req).await
    } else {
        state.proxy_engine.clone().handle_request(req).await
    }
}

fn is_management_host(host_header: &str, bind_host: &str) -> bool {
    let host = host_without_port(host_header).to_ascii_lowercase();
    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return true;
    }

    let bind_host = bind_host.trim().to_ascii_lowercase();
    if !matches!(bind_host.as_str(), "0.0.0.0" | "::" | "[::]") {
        return host == host_without_port(&bind_host).to_ascii_lowercase();
    }

    if host == "0.0.0.0" {
        return true;
    }

    let lan_hosts = [
        crate::setup::public_lan_ip_for_setup(),
        crate::setup::detect_lan_ip(),
    ];
    lan_hosts
        .into_iter()
        .flatten()
        .any(|lan_host| host == lan_host.to_ascii_lowercase())
}

fn host_without_port(host_header: &str) -> &str {
    let host = host_header.trim();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(host);
    }
    host.rsplit_once(':')
        .filter(|(_, port)| {
            host.matches(':').count() == 1 && port.chars().all(|c| c.is_ascii_digit())
        })
        .map_or(host, |(host, _)| host)
}

fn storage_root(storage_path: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(storage_path)
        .map_err(|e| format!("failed to create storage directory: {e}"))?;
    storage_path
        .canonicalize()
        .map_err(|e| format!("failed to resolve storage directory: {e}"))
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn resolve_storage_file_for_write(storage_path: &Path, requested: &str) -> Result<PathBuf, String> {
    let root = storage_root(storage_path)?;
    let requested = Path::new(requested);
    if requested.as_os_str().is_empty() {
        return Err("path is required".to_string());
    }

    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        if !is_safe_relative_path(requested) {
            return Err("path must stay inside storage directory".to_string());
        }
        root.join(requested)
    };

    let Some(file_name) = candidate.file_name() else {
        return Err("path must include a file name".to_string());
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| "path must include a parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("failed to create parent directory: {e}"))?;
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("failed to resolve parent directory: {e}"))?;
    if !parent.starts_with(&root) {
        return Err("path must stay inside storage directory".to_string());
    }

    let file = parent.join(file_name);
    if file.exists() {
        let resolved = file
            .canonicalize()
            .map_err(|e| format!("failed to resolve file: {e}"))?;
        if !resolved.starts_with(&root) || !resolved.is_file() {
            return Err("path must stay inside storage directory".to_string());
        }
    }

    Ok(file)
}

fn resolve_storage_file_for_read(storage_path: &Path, requested: &str) -> Result<PathBuf, String> {
    let root = storage_root(storage_path)?;
    let requested = Path::new(requested);
    if requested.as_os_str().is_empty() {
        return Err("path is required".to_string());
    }
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        if !is_safe_relative_path(requested) {
            return Err("path must stay inside storage directory".to_string());
        }
        root.join(requested)
    };
    let file = candidate
        .canonicalize()
        .map_err(|e| format!("failed to resolve file: {e}"))?;
    if !file.starts_with(&root) {
        return Err("path must stay inside storage directory".to_string());
    }
    if !file.is_file() {
        return Err("path must reference a file".to_string());
    }
    Ok(file)
}

/// Fallback handler: proxies any request that didn't match a management route.
async fn proxy_fallback(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    state.proxy_engine.clone().handle_request(req).await
}

// ── Static assets ──────────────────────────────────────────────────────────────

async fn serve_index() -> impl IntoResponse {
    Html(design_assets::INDEX_HTML)
}

async fn serve_manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("manifest.json"),
    )
}

async fn serve_sw() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("sw.js"),
    )
}

async fn serve_icon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("icon.svg"),
    )
}
async fn serve_design_app_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        design_assets::APP_CSS,
    )
}
async fn serve_design_app_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        design_assets::APP_JS,
    )
}
async fn serve_setup_wizard() -> impl IntoResponse {
    Html(include_str!("setup_wizard.html"))
}

async fn get_network_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let detected_lan_ip = crate::setup::detect_lan_ip();
    let lan_setup_ip = crate::setup::public_lan_ip_for_setup();
    let running_in_container = crate::setup::running_in_container();
    let port = state.config.port;
    let socks5_port = state.config.socks5_port;
    // Best IP to advertise to remote clients (other devices on LAN).
    // In a container the bridge IP is not reachable from outside, so we fall back
    // to detected_lan_ip with a caveat flag; the client can show both options.
    let remote_ip = lan_setup_ip
        .clone()
        .or_else(|| detected_lan_ip.clone())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let lan_ip_available = lan_setup_ip.is_some();
    let ca_url = format!("http://{}:{}/admin/ca", remote_ip, port);
    axum::Json(serde_json::json!({
        "lan_ip": remote_ip,
        "detected_lan_ip": detected_lan_ip,
        "lan_setup_ip": lan_setup_ip,
        "port": port,
        "socks5_port": socks5_port,
        "localhost_proxy": format!("127.0.0.1:{port}"),
        "lan_proxy": format!("{remote_ip}:{port}"),
        "ca_url": ca_url,
        "ca_local_url": format!("http://127.0.0.1:{port}/admin/ca"),
        "running_in_container": running_in_container,
        "lan_ip_available": lan_ip_available,
        "mitm_enabled": state.proxy_engine.mitm_enabled,
    }))
}

async fn not_found() -> impl IntoResponse {
    axum::http::StatusCode::NOT_FOUND
}

// ── Health ─────────────────────────────────────────────────────────────────────

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok",
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "mitm_enabled": state.proxy_engine.mitm_enabled,
    }))
}

// ── Sessions ───────────────────────────────────────────────────────────────────

async fn list_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<SessionQuery>,
) -> impl IntoResponse {
    let started = Instant::now();
    let since = q
        .since
        .as_deref()
        .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());
    let sessions = state
        .api_handler
        .list_sessions(
            since,
            q.limit,
            q.offset,
            q.q.as_deref(),
            q.include_bodies.unwrap_or(false),
        )
        .await;
    record_endpoint_timing(
        &state.endpoint_metrics,
        "/api/sessions",
        started,
        sessions.total,
    );
    axum::Json(sessions)
}

/// Server-Sent Events stream: fires a `{"type":"update"}` event whenever
/// any session changes (new request, new response, clear). Clients subscribe
/// once and re-fetch sessions on each event rather than polling every 2 s.
async fn sessions_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.session_manager.subscribe();
    let stream = BroadcastStream::new(rx)
        .map(|_| Ok::<_, std::convert::Infallible>(Event::default().data("update")));
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

async fn get_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.api_handler.get_session_details(&id).await {
        Some(detail) => axum::Json(detail).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

async fn get_ws_frames(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.session_manager.get_session(&id) {
        Some(exchange) => axum::Json(exchange.ws_frames).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(serde::Deserialize, Default)]
struct ExportQuery {
    format: Option<String>,
    raw: Option<bool>,
}

async fn export_session(
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

async fn get_session_timing(
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
struct DiffQuery {
    a: String,
    b: String,
}

async fn diff_sessions(
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
struct CurlImportBody {
    curl: String,
}

async fn import_curl(
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
struct AnnotationPatch {
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

async fn annotate_session(
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

async fn clear_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.api_handler.clear_sessions().await;
    axum::http::StatusCode::OK
}

async fn save_sessions(
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

async fn load_sessions(
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
struct ImportRequest {
    sessions: Vec<crate::session::Exchange>,
    #[serde(default = "bool_true")]
    merge: bool,
}
fn bool_true() -> bool {
    true
}

fn storage_error_response(error: std::io::Error) -> axum::response::Response {
    tracing::warn!(error = %error, "Failed to persist management state");
    (
        axum::http::StatusCode::INSUFFICIENT_STORAGE,
        axum::Json(serde_json::json!({
            "error": format!("failed to persist state: {error}")
        })),
    )
        .into_response()
}

async fn import_sessions(
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
struct HarExportQuery {
    raw: Option<bool>,
    ids: Option<String>,
}

async fn export_har(
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
struct HarImportQuery {
    #[serde(default = "bool_true")]
    merge: bool,
}

async fn import_har(
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

// ── Routes ─────────────────────────────────────────────────────────────────────

async fn list_routes(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.routing_table.read().await.clone())
}

async fn update_routes(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_routes): axum::extract::Json<HashMap<String, String>>,
) -> impl IntoResponse {
    let mut routes = state.routing_table.write().await;
    *routes = new_routes;
    if let Err(e) = storage::save_routes(&state.storage_path, &routes) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Throttling ─────────────────────────────────────────────────────────────────

async fn get_throttling(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.throttling_config.read().await.clone())
}

async fn update_throttling(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_config): axum::extract::Json<ThrottlingConfig>,
) -> impl IntoResponse {
    let mut config = state.throttling_config.write().await;
    *config = new_config;
    if let Err(e) = storage::save_throttle(&state.storage_path, &config) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Rewrites ───────────────────────────────────────────────────────────────────

async fn list_rewrites(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_rewrite_rules().await)
}

async fn add_rewrite(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(rule): axum::extract::Json<RewriteRule>,
) -> impl IntoResponse {
    state.api_handler.add_rewrite_rule(rule).await;
    let rules = state.api_handler.list_rewrite_rules().await;
    if let Err(e) = storage::save_rewrites(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::CREATED.into_response()
}

async fn delete_rewrite(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(index): axum::extract::Path<usize>,
) -> impl IntoResponse {
    state.api_handler.delete_rewrite_rule(index).await;
    let rules = state.api_handler.list_rewrite_rules().await;
    if let Err(e) = storage::save_rewrites(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

async fn update_rewrite(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(index): axum::extract::Path<usize>,
    axum::extract::Json(rule): axum::extract::Json<RewriteRule>,
) -> impl IntoResponse {
    state.api_handler.update_rewrite_rule(index, rule).await;
    let rules = state.api_handler.list_rewrite_rules().await;
    if let Err(e) = storage::save_rewrites(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

async fn replace_all_rewrites(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(rules): axum::extract::Json<Vec<RewriteRule>>,
) -> impl IntoResponse {
    state
        .api_handler
        .replace_all_rewrite_rules(rules.clone())
        .await;
    if let Err(e) = storage::save_rewrites(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Header Maps ────────────────────────────────────────────────────────────────

async fn list_header_maps(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_header_maps().await)
}

async fn add_header_map(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<
        crate::middleware::plugins::header_map::HeaderMapRule,
    >,
) -> impl IntoResponse {
    rule.id = uuid::Uuid::new_v4().to_string();
    let saved = rule.clone();
    state.api_handler.add_header_map(rule).await;
    let rules = state.api_handler.list_header_maps().await;
    if let Err(e) = storage::save_header_maps(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::Json(saved).into_response()
}

async fn update_header_map(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(rule): axum::extract::Json<
        crate::middleware::plugins::header_map::HeaderMapRule,
    >,
) -> impl IntoResponse {
    state.api_handler.update_header_map(&id, rule).await;
    let rules = state.api_handler.list_header_maps().await;
    if let Err(e) = storage::save_header_maps(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

async fn delete_header_map(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    state.api_handler.delete_header_map(&id).await;
    let rules = state.api_handler.list_header_maps().await;
    if let Err(e) = storage::save_header_maps(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Metrics ────────────────────────────────────────────────────────────────────

async fn get_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let started = Instant::now();
    let sessions = state.session_manager.get_all_sessions();
    let session_count = sessions.len();
    let mut payload = build_metrics_payload(&sessions);
    record_endpoint_timing(
        &state.endpoint_metrics,
        "/admin/metrics",
        started,
        session_count,
    );
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "endpoint_timings".to_string(),
            endpoint_timing_payload(&state.endpoint_metrics),
        );
    }
    axum::Json(payload)
}

fn build_metrics_payload(sessions: &[crate::session::Exchange]) -> serde_json::Value {
    let raw: Vec<_> = sessions.iter().filter_map(|s| s.metrics.as_ref()).collect();
    let latency_samples: Vec<u64> = raw.iter().map(|m| m.latency_ms).collect();
    let captured_session_count = sessions.len();
    let active_requests = sessions.iter().filter(|s| s.response.is_none()).count();
    let completed_requests = captured_session_count.saturating_sub(active_requests);
    let proxied_requests = sessions
        .iter()
        .filter(|s| s.source == SessionSource::Proxy)
        .count();
    let admin_forward_requests = sessions
        .iter()
        .filter(|s| s.source == SessionSource::AdminForward)
        .count();
    let playback_requests = sessions
        .iter()
        .filter(|s| s.source == SessionSource::Playback)
        .count();
    let imported_sessions = sessions
        .iter()
        .filter(|s| s.source == SessionSource::Imported)
        .count();
    let inspected_requests = raw.len();
    let error_count = raw.iter().filter(|m| m.status_code >= 400).count();
    let total_request_bytes: u64 = raw.iter().map(|m| m.request_size_bytes as u64).sum();
    let total_response_bytes: u64 = raw.iter().map(|m| m.response_size_bytes as u64).sum();
    let avg_latency_ms = if inspected_requests > 0 {
        raw.iter().map(|m| m.latency_ms).sum::<u64>() / inspected_requests as u64
    } else {
        0
    };
    let avg_request_size_bytes = if inspected_requests > 0 {
        total_request_bytes / inspected_requests as u64
    } else {
        0
    };
    let avg_response_size_bytes = if inspected_requests > 0 {
        total_response_bytes / inspected_requests as u64
    } else {
        0
    };
    serde_json::json!({
        "sessions": {
            "captured": captured_session_count,
            "active_without_response": active_requests,
            "completed": completed_requests,
            "by_source": {
                "proxy": proxied_requests,
                "admin_forward": admin_forward_requests,
                "playback": playback_requests,
                "imported": imported_sessions,
            },
        },
        "requests": {
            "active": active_requests,
            "completed_with_metrics": inspected_requests,
            "errors": error_count,
            "proxied": proxied_requests,
            "admin_forward": admin_forward_requests,
            "playback": playback_requests,
        },
        "captured_session_count": captured_session_count,
        "active_requests": active_requests,
        "completed_requests": completed_requests,
        "proxied_requests": proxied_requests,
        "admin_forward_requests": admin_forward_requests,
        "playback_requests": playback_requests,
        "imported_sessions": imported_sessions,
        "inspected_requests": inspected_requests,
        "error_count": error_count,
        "latency_samples": latency_samples,
        "total_request_bytes": total_request_bytes,
        "total_response_bytes": total_response_bytes,
        "avg_latency_ms": avg_latency_ms,
        "avg_request_size_bytes": avg_request_size_bytes,
        "avg_response_size_bytes": avg_response_size_bytes,
    })
}

// ── Playback ───────────────────────────────────────────────────────────────────

async fn start_playback(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.api_handler.start_playback().await;
    axum::http::StatusCode::OK
}

// ── Breakpoints ────────────────────────────────────────────────────────────────

async fn list_bp_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_breakpoint_rules().await)
}

async fn add_bp_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<BreakpointRule>,
) -> impl IntoResponse {
    let normalized = rule.pattern.trim();
    rule.pattern = if normalized.is_empty() || normalized == "*" {
        ".*".to_string()
    } else {
        normalized.to_string()
    };
    if let Err(e) = regex::Regex::new(&rule.pattern) {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({ "error": format!("invalid breakpoint regex: {e}") })),
        )
            .into_response();
    }
    rule.id = uuid::Uuid::new_v4().to_string();
    state.api_handler.add_breakpoint_rule(rule).await;
    let rules = state.api_handler.list_breakpoint_rules().await;
    if let Err(e) = storage::save_breakpoints(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::CREATED.into_response()
}

async fn delete_bp_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    state.api_handler.delete_breakpoint_rule(&id).await;
    let rules = state.api_handler.list_breakpoint_rules().await;
    if let Err(e) = storage::save_breakpoints(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

async fn update_bp_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<BreakpointRule>,
) -> impl IntoResponse {
    let normalized = rule.pattern.trim();
    rule.pattern = if normalized.is_empty() || normalized == "*" {
        ".*".to_string()
    } else {
        normalized.to_string()
    };
    if let Err(e) = regex::Regex::new(&rule.pattern) {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({ "error": format!("invalid breakpoint regex: {e}") })),
        )
            .into_response();
    }
    if !state.api_handler.update_breakpoint_rule(&id, rule).await {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let rules = state.api_handler.list_breakpoint_rules().await;
    if let Err(e) = storage::save_breakpoints(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

async fn list_pending_bp(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_pending().await)
}

#[derive(serde::Deserialize)]
struct ResolutionRequest {
    action: String,
    context: Option<BreakpointContext>,
}

async fn resolve_bp(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(req): axum::extract::Json<ResolutionRequest>,
) -> impl IntoResponse {
    let resolution = match req.action.as_str() {
        "drop" => BreakpointResolution::Drop,
        "modify" => req
            .context
            .map(BreakpointResolution::Modify)
            .unwrap_or(BreakpointResolution::Continue),
        _ => BreakpointResolution::Continue,
    };
    match state.api_handler.resolve_breakpoint(id, resolution).await {
        Ok(_) => axum::http::StatusCode::OK.into_response(),
        Err(e) => (axum::http::StatusCode::NOT_FOUND, e).into_response(),
    }
}

// ── Plugin Registry ────────────────────────────────────────────────────────────

async fn list_plugins(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let chain = state.middleware_chain.read().await;
    axum::Json(serde_json::json!({ "plugins": chain.list_plugins() }))
}

// ── Config Hot Reload ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
struct HotReloadBody {
    max_body_bytes: Option<usize>,
}

async fn reload_config(
    State(state): State<Arc<AppState>>,
    body: Option<axum::Json<HotReloadBody>>,
) -> impl IntoResponse {
    let posted = body.map(|b| b.0).unwrap_or_default();
    let max_body_bytes = if let Some(v) = posted.max_body_bytes {
        // Persist the UI-supplied override so it survives restarts
        if let Err(e) = storage::save_hot_config(
            &state.storage_path,
            &storage::HotConfig {
                max_body_bytes: Some(v),
            },
        ) {
            return storage_error_response(e);
        }
        v
    } else {
        // No value supplied — re-read from YAML
        crate::config::Config::load().max_body_bytes
    };
    state.proxy_engine.set_max_body_bytes(max_body_bytes);
    tracing::info!(max_body_bytes, "Config hot-reloaded");
    axum::Json(serde_json::json!({
        "reloaded": true,
        "max_body_bytes": max_body_bytes,
        "note": "timeout_secs and pool settings require restart to take effect",
    }))
    .into_response()
}

// ── Capture Filter ────────────────────────────────────────────────────────────

async fn get_capture_filter(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.capture_filter.read().await.clone())
}

async fn update_capture_filter(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_cfg): axum::extract::Json<CaptureFilterConfig>,
) -> impl IntoResponse {
    let mut cfg = state.capture_filter.write().await;
    *cfg = new_cfg;
    if let Err(e) = storage::save_capture_filter(&state.storage_path, &cfg) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── DNS Override ──────────────────────────────────────────────────────────────

async fn list_dns(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.dns_overrides.read().await.clone())
}

async fn update_dns(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_map): axum::extract::Json<HashMap<String, String>>,
) -> impl IntoResponse {
    let mut overrides = state.dns_overrides.write().await;
    *overrides = new_map;
    if let Err(e) = storage::save_dns_overrides(&state.storage_path, &overrides) {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

async fn delete_dns(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut overrides = state.dns_overrides.write().await;
    if overrides.remove(&host).is_some() {
        if let Err(e) = storage::save_dns_overrides(&state.storage_path, &overrides) {
            return storage_error_response(e);
        }
        axum::http::StatusCode::OK.into_response()
    } else {
        axum::http::StatusCode::NOT_FOUND.into_response()
    }
}

// ── CA certificate ─────────────────────────────────────────────────────────────

async fn get_ca_cert(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match &state.proxy_engine.ca {
        Some(ca) => {
            let cert = ca.get_root_cert_pem();
            (
                axum::http::StatusCode::OK,
                [
                    ("Content-Type", "application/x-pem-file"),
                    ("Content-Disposition", "attachment; filename=\"oproxy-ca.pem\""),
                ],
                cert,
            )
                .into_response()
        }
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

// ── Config ────────────────────────────────────────────────────────────────────

async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();
    axum::Json(serde_json::json!({
        "port": state.config.port,
        "bind_host": state.config.bind_host,
        "mitm_enabled": state.config.mitm.enabled,
        "max_body_bytes": state.proxy_engine.max_body_bytes(),
        "max_sessions": state.config.max_sessions,
        "max_retained_body_bytes": state.config.max_retained_body_bytes,
        "timeout_secs": state.config.timeout_secs,
        "inspect_ws_frames": state.config.inspect_ws_frames,
        "storage_path": state.storage_path.display().to_string(),
        "uptime_secs": uptime,
    }))
}

// ── Upstream Proxy ─────────────────────────────────────────────────────────────

async fn get_upstream_proxy(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let url = storage::load_upstream_proxy(&state.storage_path)
        .or_else(|| state.config.upstream_proxy.clone());
    axum::Json(serde_json::json!({ "upstream_proxy": url }))
}

#[derive(serde::Deserialize)]
struct UpstreamProxyBody {
    /// Empty string or null to disable. Valid URL to enable.
    upstream_proxy: Option<String>,
}

async fn set_upstream_proxy_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(body): axum::extract::Json<UpstreamProxyBody>,
) -> impl IntoResponse {
    let url = body.upstream_proxy.filter(|s| !s.is_empty());
    // Validate URL if provided
    if let Some(ref u) = url
        && reqwest::Proxy::all(u).is_err()
    {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({ "error": "invalid proxy URL" })),
        )
            .into_response();
    }
    if let Err(e) = storage::save_upstream_proxy(&state.storage_path, &url) {
        return storage_error_response(e);
    }
    state.proxy_engine.set_upstream_proxy(url.as_deref()).await;
    axum::Json(serde_json::json!({ "ok": true, "upstream_proxy": url })).into_response()
}

// ── Webhooks ──────────────────────────────────────────────────────────────────

async fn list_webhooks(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let hooks = state.webhooks.read().await.clone();
    axum::Json(hooks)
}

async fn create_webhook(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut hook): axum::extract::Json<WebhookConfig>,
) -> impl IntoResponse {
    sanitize_webhook_events(&mut hook.events);
    if hook.events.is_empty() {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({
                "error": "webhook must include request_captured or response_captured"
            })),
        )
            .into_response();
    }
    if hook.id.is_empty() {
        hook.id = uuid::Uuid::new_v4().to_string();
    }
    let mut hooks = state.webhooks.write().await;
    hooks.push(hook);
    if let Err(e) = storage::save_webhooks(&state.storage_path, &hooks) {
        return storage_error_response(e);
    }
    axum::Json(serde_json::json!({ "ok": true })).into_response()
}

async fn update_webhook(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut updated): axum::extract::Json<WebhookConfig>,
) -> impl IntoResponse {
    sanitize_webhook_events(&mut updated.events);
    if updated.events.is_empty() {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({
                "error": "webhook must include request_captured or response_captured"
            })),
        )
            .into_response();
    }
    let mut hooks = state.webhooks.write().await;
    if let Some(h) = hooks.iter_mut().find(|h| h.id == id) {
        *h = updated;
        if let Err(e) = storage::save_webhooks(&state.storage_path, &hooks) {
            return storage_error_response(e);
        }
        axum::Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "webhook not found").into_response()
    }
}

async fn delete_webhook(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut hooks = state.webhooks.write().await;
    let before = hooks.len();
    hooks.retain(|h| h.id != id);
    if hooks.len() < before {
        if let Err(e) = storage::save_webhooks(&state.storage_path, &hooks) {
            return storage_error_response(e);
        }
        axum::Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "webhook not found").into_response()
    }
}

// ── Mock Rules ────────────────────────────────────────────────────────────────

async fn list_mock_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.mock_rules.read().await.clone())
}

async fn create_mock_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<MockRule>,
) -> impl IntoResponse {
    if rule.id.is_empty() {
        rule.id = uuid::Uuid::new_v4().to_string();
    }
    let mut rules = state.mock_rules.write().await;
    rules.push(rule);
    if let Err(e) = storage::save_mock_rules(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::Json(serde_json::json!({ "ok": true })).into_response()
}

async fn update_mock_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(updated): axum::extract::Json<MockRule>,
) -> impl IntoResponse {
    let mut rules = state.mock_rules.write().await;
    if let Some(r) = rules.iter_mut().find(|r| r.id == id) {
        let preserved_count = r.call_count;
        *r = updated;
        r.call_count = preserved_count;
        if let Err(e) = storage::save_mock_rules(&state.storage_path, &rules) {
            return storage_error_response(e);
        }
        axum::Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "mock rule not found").into_response()
    }
}

async fn delete_mock_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut rules = state.mock_rules.write().await;
    let before = rules.len();
    rules.retain(|r| r.id != id);
    if rules.len() < before {
        if let Err(e) = storage::save_mock_rules(&state.storage_path, &rules) {
            return storage_error_response(e);
        }
        axum::Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "mock rule not found").into_response()
    }
}

async fn reset_mock_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut rules = state.mock_rules.write().await;
    if let Some(r) = rules.iter_mut().find(|r| r.id == id) {
        r.call_count = 0;
        if let Err(e) = storage::save_mock_rules(&state.storage_path, &rules) {
            return storage_error_response(e);
        }
        axum::Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "mock rule not found").into_response()
    }
}

// ── SOCKS5 Status ─────────────────────────────────────────────────────────────

async fn get_socks5_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let enabled = state.config.socks5_port.is_some();
    let mitm_active = enabled && state.proxy_engine.mitm_enabled;
    axum::Json(serde_json::json!({
        "enabled": enabled,
        "port": state.config.socks5_port,
        "mode": if mitm_active { "mitm" } else { "tunnel-only" },
        "captures_sessions": mitm_active,
    }))
}

// ── Lua Scripts ───────────────────────────────────────────────────────────────

async fn list_scripts(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.lua_scripts.read().await.clone())
}

async fn create_script(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut script): axum::extract::Json<LuaScript>,
) -> impl IntoResponse {
    if script.id.is_empty() {
        script.id = uuid::Uuid::new_v4().to_string();
    }
    let mut scripts = state.lua_scripts.write().await;
    scripts.push(script);
    if let Err(e) = storage::save_lua_scripts(&state.storage_path, &scripts) {
        return storage_error_response(e);
    }
    axum::Json(serde_json::json!({ "ok": true })).into_response()
}

async fn update_script(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(updated): axum::extract::Json<LuaScript>,
) -> impl IntoResponse {
    let mut scripts = state.lua_scripts.write().await;
    if let Some(s) = scripts.iter_mut().find(|s| s.id == id) {
        *s = updated;
        if let Err(e) = storage::save_lua_scripts(&state.storage_path, &scripts) {
            return storage_error_response(e);
        }
        axum::Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "script not found").into_response()
    }
}

async fn delete_script(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut scripts = state.lua_scripts.write().await;
    let before = scripts.len();
    scripts.retain(|s| s.id != id);
    if scripts.len() < before {
        if let Err(e) = storage::save_lua_scripts(&state.storage_path, &scripts) {
            return storage_error_response(e);
        }
        axum::Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "script not found").into_response()
    }
}

// ── Modifications ─────────────────────────────────────────────────────────────

async fn list_modifications(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_modifications().await)
}

async fn add_modification(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(rule): axum::extract::Json<ModificationRule>,
) -> impl IntoResponse {
    state.api_handler.add_modification(rule).await;
    let rules = state.api_handler.list_modifications().await;
    if let Err(e) = storage::save_modifications(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::Json(rules).into_response()
}

async fn delete_modification(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(index): axum::extract::Path<usize>,
) -> impl IntoResponse {
    state.api_handler.delete_modification(index).await;
    let rules = state.api_handler.list_modifications().await;
    if let Err(e) = storage::save_modifications(&state.storage_path, &rules) {
        return storage_error_response(e);
    }
    axum::Json(rules).into_response()
}

#[derive(serde::Deserialize)]
struct MapLocalEntry {
    host: String,
    file_path: String,
}

async fn list_map_local(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let map = state.map_local.read().await.clone();
    axum::Json(map)
}

async fn set_map_local(
    State(state): State<Arc<AppState>>,
    axum::Json(entry): axum::Json<MapLocalEntry>,
) -> impl IntoResponse {
    let file_path = match resolve_storage_file_for_read(&state.storage_path, &entry.file_path) {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };
    {
        let mut map = state.map_local.write().await;
        map.insert(entry.host, file_path);
    }
    let map = state.map_local.read().await.clone();
    if let Err(e) = storage::save_map_local(&state.storage_path, &map) {
        return storage_error_response(e);
    }
    axum::Json(map).into_response()
}

async fn delete_map_local(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut map = state.map_local.write().await;
        map.remove(&host);
    }
    let map = state.map_local.read().await.clone();
    if let Err(e) = storage::save_map_local(&state.storage_path, &map) {
        return storage_error_response(e);
    }
    axum::Json(map).into_response()
}

#[derive(serde::Deserialize)]
struct ForwardReq {
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(serde::Serialize)]
struct ForwardResp {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
    is_binary: bool,
    session_id: String,
}

async fn forward_request(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ForwardReq>,
) -> impl IntoResponse {
    let method = match reqwest::Method::from_bytes(req.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "error": format!("Invalid HTTP method: {}", req.method)
                })),
            )
                .into_response();
        }
    };
    let url_parsed = match reqwest::Url::parse(&req.url) {
        Ok(u) => u,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "error": format!("Invalid URL: {e}") })),
            )
                .into_response();
        }
    };
    if !matches!(url_parsed.scheme(), "http" | "https") {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": format!("Unsupported URL scheme: {}", url_parsed.scheme())
            })),
        )
            .into_response();
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let host = match (url_parsed.host_str(), url_parsed.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => String::new(),
    };
    let display_uri = req.url.clone();

    // Record request in session manager
    let req_ctx = RequestContext {
        method: req.method.clone(),
        uri: display_uri.clone(),
        host: host.clone(),
        headers: req.headers.clone(),
        body: req.body.clone().unwrap_or_default(),
        body_bytes: None,
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
            .annotate(&session_id, req.note.clone(), req.tags.clone());
    }

    // Build and send request using the proxy engine's http client
    let mut builder = state
        .proxy_engine
        .http_client()
        .await
        .request(method, &req.url);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    if let Some(body) = req.body {
        builder = builder.body(body);
    }

    let t0 = std::time::Instant::now();
    match builder.send().await {
        Ok(res) => {
            let ttfb_ms = t0.elapsed().as_millis() as u64;
            let status = res.status().as_u16();
            let mut res_headers: HashMap<String, String> = HashMap::new();
            for (k, v) in res.headers() {
                res_headers.insert(k.to_string(), v.to_str().unwrap_or("").to_string());
            }
            let content_type = res_headers.get("content-type").cloned().unwrap_or_default();
            let bytes = res.bytes().await.unwrap_or_default();
            let body_ms = t0.elapsed().as_millis() as u64 - ttfb_ms;
            let (body, is_binary) = if is_binary_content_type(&content_type) {
                (
                    base64::engine::general_purpose::STANDARD.encode(&bytes),
                    true,
                )
            } else {
                (String::from_utf8_lossy(&bytes).to_string(), false)
            };

            // Record response
            let res_ctx = ResponseContext {
                status,
                headers: res_headers.clone(),
                body: body.clone(),
                request_uri: display_uri,
                session_id: Some(session_id.clone()),
                ttfb_ms,
                body_ms,
                body_bytes: None,
            };
            let metrics = crate::session::InspectionMetrics {
                latency_ms: t0.elapsed().as_millis() as u64,
                request_size_bytes,
                response_size_bytes: bytes.len(),
                status_code: status,
                ttfb_ms,
                body_ms,
                ..Default::default()
            };
            state
                .api_handler
                .session_manager
                .record_response_with_metrics(session_id.clone(), res_ctx, metrics);

            axum::Json(ForwardResp {
                status,
                headers: res_headers,
                body,
                is_binary,
                session_id,
            })
            .into_response()
        }
        Err(e) => {
            let res_ctx = ResponseContext {
                status: 502,
                body: e.to_string(),
                request_uri: display_uri,
                session_id: Some(session_id.clone()),
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
            (axum::http::StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{RequestContext, ResponseContext};
    use crate::session::{Exchange, InspectionMetrics, SessionSource};
    use chrono::Utc;

    fn exchange(id: &str, source: SessionSource, status: Option<u16>) -> Exchange {
        let response = status.map(|code| ResponseContext {
            status: code,
            headers: HashMap::new(),
            body: String::new(),
            request_uri: "/test".to_string(),
            session_id: Some(id.to_string()),
            ttfb_ms: 4,
            body_ms: 2,
            body_bytes: None,
        });
        let metrics = status.map(|code| InspectionMetrics {
            latency_ms: 12,
            request_size_bytes: 3,
            response_size_bytes: 5,
            status_code: code,
            ttfb_ms: 4,
            body_ms: 2,
            ..Default::default()
        });
        Exchange {
            id: id.to_string(),
            timestamp: Utc::now(),
            updated_at: None,
            request: RequestContext {
                method: "GET".to_string(),
                uri: "/test".to_string(),
                headers: HashMap::new(),
                body: String::new(),
                host: "example.com".to_string(),
                body_bytes: None,
            },
            response,
            metrics,
            source,
            ws_frames: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
        }
    }

    #[test]
    fn metrics_payload_splits_session_sources_and_active_requests() {
        let sessions = vec![
            exchange("proxy-ok", SessionSource::Proxy, Some(200)),
            exchange("proxy-pending", SessionSource::Proxy, None),
            exchange("admin", SessionSource::AdminForward, Some(502)),
            exchange("imported", SessionSource::Imported, Some(201)),
        ];

        let metrics = build_metrics_payload(&sessions);

        assert_eq!(metrics["captured_session_count"], 4);
        assert_eq!(metrics["active_requests"], 1);
        assert_eq!(metrics["completed_requests"], 3);
        assert_eq!(metrics["proxied_requests"], 2);
        assert_eq!(metrics["admin_forward_requests"], 1);
        assert_eq!(metrics["imported_sessions"], 1);
        assert_eq!(metrics["inspected_requests"], 3);
        assert_eq!(metrics["error_count"], 1);
        assert_eq!(metrics["sessions"]["captured"], 4);
        assert_eq!(metrics["sessions"]["active_without_response"], 1);
        assert_eq!(metrics["sessions"]["completed"], 3);
        assert_eq!(metrics["sessions"]["by_source"]["proxy"], 2);
        assert_eq!(metrics["sessions"]["by_source"]["admin_forward"], 1);
        assert_eq!(metrics["sessions"]["by_source"]["imported"], 1);
        assert_eq!(metrics["requests"]["active"], 1);
        assert_eq!(metrics["requests"]["completed_with_metrics"], 3);
        assert_eq!(metrics["requests"]["proxied"], 2);
        assert_eq!(metrics["requests"]["admin_forward"], 1);
        assert!(metrics.get("total_requests").is_none());
        assert!(metrics.get("active_sessions").is_none());
    }

    #[test]
    fn management_host_accepts_localhost_and_configured_lan_bindings() {
        assert!(is_management_host("localhost:8080", "127.0.0.1"));
        assert!(is_management_host("127.0.0.1:8080", "127.0.0.1"));
        assert!(is_management_host("[::1]:8080", "127.0.0.1"));
        assert!(is_management_host("::1", "127.0.0.1"));
        assert!(is_management_host("192.168.1.10:8080", "192.168.1.10"));
        assert!(!is_management_host("example.com", "127.0.0.1"));
    }

    #[test]
    fn storage_file_write_rejects_path_traversal() {
        let dir =
            std::env::temp_dir().join(format!("oproxy_management_path_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let result = resolve_storage_file_for_write(&dir, "../outside.json");

        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn storage_file_read_rejects_absolute_path_outside_storage() {
        let dir =
            std::env::temp_dir().join(format!("oproxy_management_path_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let outside = std::env::temp_dir().join(format!(
            "oproxy_management_outside_{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&outside, "{}").unwrap();

        let result = resolve_storage_file_for_read(&dir, outside.to_str().unwrap());

        assert!(result.is_err());
        let _ = std::fs::remove_file(outside);
        let _ = std::fs::remove_dir_all(dir);
    }
}
