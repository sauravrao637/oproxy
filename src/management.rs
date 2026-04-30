use std::sync::Arc;
use std::collections::HashMap;
use axum::{
    Router,
    routing::get,
    extract::State,
    http::header,
    middleware::Next,
    response::{Html, IntoResponse},
};
use axum::response::sse::{Event, Sse};
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

#[derive(serde::Deserialize, Default)]
struct SessionQuery {
    since: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}
use crate::AppState;
use crate::storage;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::middleware::plugins::modification::ModificationRule;
use crate::middleware::plugins::rewrite::RewriteRule;
use crate::middleware::plugins::breakpoints::{BreakpointRule, BreakpointResolution, BreakpointContext};
use crate::api::SessionFileRequest;
use crate::middleware::{RequestContext, ResponseContext};
use crate::core::engine::is_binary_content_type;
use base64::Engine as _;

/// Builds the management router: UI, admin API, static assets, and proxy fallback.
/// The caller is responsible for applying the proxy-dispatch layer on top.
pub fn management_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/health", get(health))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/stream", get(sessions_stream))
        .route("/api/sessions/:id", get(get_session))
        .route("/api/sessions/:id/ws-frames", get(get_ws_frames))
        .route("/admin/routes", get(list_routes).post(update_routes))
        .route("/admin/sessions", get(list_sessions).delete(clear_sessions))
        .route("/admin/sessions/save", axum::routing::post(save_sessions))
        .route("/admin/sessions/load", axum::routing::post(load_sessions))
        .route("/admin/sessions/import", axum::routing::post(import_sessions))
        .route("/admin/throttling", get(get_throttling).post(update_throttling))
        .route("/admin/rewrites", get(list_rewrites).post(add_rewrite))
        .route("/admin/rewrites/replace-all", axum::routing::post(replace_all_rewrites))
        .route("/admin/rewrites/:index", axum::routing::delete(delete_rewrite).put(update_rewrite))
        .route("/admin/header-maps", get(list_header_maps).post(add_header_map))
        .route("/admin/header-maps/:id", axum::routing::put(update_header_map).delete(delete_header_map))
        .route("/admin/ca", get(get_ca_cert))
        .route("/admin/metrics", get(get_metrics))
        .route("/admin/playback", axum::routing::post(start_playback))
        .route("/admin/breakpoints", get(list_bp_rules).post(add_bp_rule))
        .route("/admin/breakpoints/pending", get(list_pending_bp))
        .route("/admin/breakpoints/pending/:id/resolve", axum::routing::post(resolve_bp))
        .route("/admin/breakpoints/:id", axum::routing::delete(delete_bp_rule))
        .route("/admin/plugins", get(list_plugins))
        .route("/admin/plugins/:name", axum::routing::delete(remove_plugin))
        .route("/admin/config/reload", axum::routing::post(reload_config))
        .route("/admin/config", get(get_config))
        .route("/admin/modifications", get(list_modifications).post(add_modification))
        .route("/admin/modifications/:index", axum::routing::delete(delete_modification))
        .route("/admin/dns", get(list_dns).post(update_dns))
        .route("/admin/dns/:host", axum::routing::delete(delete_dns))
        .route("/admin/map-local", get(list_map_local).post(set_map_local))
        .route("/admin/map-local/:host", axum::routing::delete(delete_map_local))
        .route("/admin/forward", axum::routing::post(forward_request))
        .route("/manifest.json", get(serve_manifest))
        .route("/sw.js", get(serve_sw))
        .route("/icons/icon.svg", get(serve_icon))
        // Silence browser probes that would otherwise reach the proxy fallback
        .route("/favicon.ico", get(not_found))
        .route("/.well-known/*path", get(not_found))
        .fallback(proxy_fallback)
        .with_state(state)
}

// ── Proxy dispatch ─────────────────────────────────────────────────────────────

/// Tower layer applied before route matching. Requests whose Host is not
/// localhost/127.0.0.1 go straight to the proxy engine so management routes
/// (like GET /) are never accidentally served to proxied traffic.
pub async fn proxy_dispatch_layer(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let is_local = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .map(|h| {
            h.starts_with("localhost")
                || h.starts_with("127.0.0.1")
                || h.starts_with("[::1]")
                || h.starts_with("::1")
        })
        .unwrap_or(true); // no Host header → direct connection, treat as local

    if is_local {
        next.run(req).await
    } else {
        state.proxy_engine.clone().handle_request(req).await
    }
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
    Html(include_str!("index.html"))
}

async fn serve_manifest() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/manifest+json")], include_str!("manifest.json"))
}

async fn serve_sw() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], include_str!("sw.js"))
}

async fn serve_icon() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/svg+xml")], include_str!("icon.svg"))
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
    let since = q.since.as_deref()
        .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());
    axum::Json(state.api_handler.list_sessions(since, q.limit, q.offset).await)
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

async fn clear_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.api_handler.clear_sessions().await;
    axum::http::StatusCode::OK
}

async fn save_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(req): axum::extract::Json<SessionFileRequest>,
) -> impl IntoResponse {
    match state.api_handler.save_session(req.path).await {
        Ok(_) => axum::http::StatusCode::OK.into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn load_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(req): axum::extract::Json<SessionFileRequest>,
) -> impl IntoResponse {
    match state.api_handler.load_session(req.path).await {
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
fn bool_true() -> bool { true }

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
    storage::save_routes(&state.storage_path, &routes);
    axum::http::StatusCode::OK
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
    storage::save_throttle(&state.storage_path, &config);
    axum::http::StatusCode::OK
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
    storage::save_rewrites(&state.storage_path, &rules);
    axum::http::StatusCode::CREATED
}

async fn delete_rewrite(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(index): axum::extract::Path<usize>,
) -> impl IntoResponse {
    state.api_handler.delete_rewrite_rule(index).await;
    let rules = state.api_handler.list_rewrite_rules().await;
    storage::save_rewrites(&state.storage_path, &rules);
    axum::http::StatusCode::OK
}

async fn update_rewrite(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(index): axum::extract::Path<usize>,
    axum::extract::Json(rule): axum::extract::Json<RewriteRule>,
) -> impl IntoResponse {
    state.api_handler.update_rewrite_rule(index, rule).await;
    let rules = state.api_handler.list_rewrite_rules().await;
    storage::save_rewrites(&state.storage_path, &rules);
    axum::http::StatusCode::OK
}

async fn replace_all_rewrites(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(rules): axum::extract::Json<Vec<RewriteRule>>,
) -> impl IntoResponse {
    state.api_handler.replace_all_rewrite_rules(rules.clone()).await;
    storage::save_rewrites(&state.storage_path, &rules);
    axum::http::StatusCode::OK
}

// ── Header Maps ────────────────────────────────────────────────────────────────

async fn list_header_maps(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_header_maps().await)
}

async fn add_header_map(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<crate::middleware::plugins::header_map::HeaderMapRule>,
) -> impl IntoResponse {
    rule.id = uuid::Uuid::new_v4().to_string();
    let saved = rule.clone();
    state.api_handler.add_header_map(rule).await;
    let rules = state.api_handler.list_header_maps().await;
    storage::save_header_maps(&state.storage_path, &rules);
    axum::Json(saved)
}

async fn update_header_map(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(rule): axum::extract::Json<crate::middleware::plugins::header_map::HeaderMapRule>,
) -> impl IntoResponse {
    state.api_handler.update_header_map(&id, rule).await;
    let rules = state.api_handler.list_header_maps().await;
    storage::save_header_maps(&state.storage_path, &rules);
    axum::http::StatusCode::OK
}

async fn delete_header_map(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    state.api_handler.delete_header_map(&id).await;
    let rules = state.api_handler.list_header_maps().await;
    storage::save_header_maps(&state.storage_path, &rules);
    axum::http::StatusCode::OK
}

// ── Metrics ────────────────────────────────────────────────────────────────────

async fn get_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.get_all_metrics().await)
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
    rule.id = uuid::Uuid::new_v4().to_string();
    state.api_handler.add_breakpoint_rule(rule).await;
    let rules = state.api_handler.list_breakpoint_rules().await;
    storage::save_breakpoints(&state.storage_path, &rules);
    axum::http::StatusCode::CREATED
}

async fn delete_bp_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    state.api_handler.delete_breakpoint_rule(&id).await;
    let rules = state.api_handler.list_breakpoint_rules().await;
    storage::save_breakpoints(&state.storage_path, &rules);
    axum::http::StatusCode::OK
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
        "modify" => req.context
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

async fn remove_plugin(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut chain = state.middleware_chain.write().await;
    if chain.remove_plugin(&name) {
        tracing::info!(plugin=%name, "Plugin removed at runtime");
        axum::http::StatusCode::OK.into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, format!("plugin '{}' not found", name)).into_response()
    }
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
        storage::save_hot_config(&state.storage_path, &storage::HotConfig { max_body_bytes: Some(v) });
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
    storage::save_dns_overrides(&state.storage_path, &overrides);
    axum::http::StatusCode::OK
}

async fn delete_dns(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut overrides = state.dns_overrides.write().await;
    if overrides.remove(&host).is_some() {
        storage::save_dns_overrides(&state.storage_path, &overrides);
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
                    ("Content-Type", "application/x-x509-ca-cert"),
                    ("Content-Disposition", "attachment; filename=\"root.crt\""),
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
        "timeout_secs": state.config.timeout_secs,
        "inspect_ws_frames": state.config.inspect_ws_frames,
        "storage_path": state.storage_path.display().to_string(),
        "uptime_secs": uptime,
    }))
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
    storage::save_modifications(&state.storage_path, &rules);
    axum::Json(rules)
}

async fn delete_modification(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(index): axum::extract::Path<usize>,
) -> impl IntoResponse {
    state.api_handler.delete_modification(index).await;
    let rules = state.api_handler.list_modifications().await;
    storage::save_modifications(&state.storage_path, &rules);
    axum::Json(rules)
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
    {
        let mut map = state.map_local.write().await;
        map.insert(entry.host, entry.file_path);
    }
    let map = state.map_local.read().await.clone();
    storage::save_map_local(&state.storage_path, &map);
    axum::Json(map)
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
    storage::save_map_local(&state.storage_path, &map);
    axum::Json(map)
}

#[derive(serde::Deserialize)]
struct ForwardReq {
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
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
    let session_id = uuid::Uuid::new_v4().to_string();
    let url_parsed = match reqwest::Url::parse(&req.url) {
        Ok(u) => u,
        Err(e) => return (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let host = url_parsed.host_str().unwrap_or("").to_string();
    let uri = if url_parsed.query().is_some() {
        format!("{}?{}", url_parsed.path(), url_parsed.query().unwrap())
    } else {
        url_parsed.path().to_string()
    };

    // Record request in session manager
    let req_ctx = RequestContext {
        method: req.method.clone(),
        uri: uri.clone(),
        host: host.clone(),
        headers: req.headers.clone(),
        body: req.body.clone().unwrap_or_default(),
        body_bytes: None,
    };
    state.api_handler.session_manager.record_request(session_id.clone(), req_ctx);

    // Build and send request using the proxy engine's http client
    let method = match reqwest::Method::from_bytes(req.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => reqwest::Method::GET,
    };
    let mut builder = state.proxy_engine.http_client.request(method, &req.url);
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
                (base64::engine::general_purpose::STANDARD.encode(&bytes), true)
            } else {
                (String::from_utf8_lossy(&bytes).to_string(), false)
            };

            // Record response
            let res_ctx = ResponseContext {
                status,
                headers: res_headers.clone(),
                body: body.clone(),
                request_uri: uri,
                session_id: Some(session_id.clone()),
                ttfb_ms,
                body_ms,
                body_bytes: None,
            };
            state.api_handler.session_manager.record_response(session_id.clone(), res_ctx);

            axum::Json(ForwardResp { status, headers: res_headers, body, is_binary, session_id }).into_response()
        }
        Err(e) => {
            let res_ctx = ResponseContext {
                status: 502,
                body: e.to_string(),
                request_uri: uri,
                session_id: Some(session_id.clone()),
                ..Default::default()
            };
            state.api_handler.session_manager.record_response(session_id.clone(), res_ctx);
            (axum::http::StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}
