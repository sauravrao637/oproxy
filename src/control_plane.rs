use axum::{Router, extract::State, response::IntoResponse, routing::get};
use std::sync::Arc;
use std::time::Instant;

use crate::AppState;

mod assets;
mod auth;
mod breakpoints;
mod extensions;
mod forward;
mod metrics;
mod policy;
mod sessions;
mod settings;
mod storage_paths;
mod webhooks;

use assets::{
    not_found, robots_txt, serve_design_app_css, serve_design_app_js, serve_icon, serve_index,
    serve_manifest, serve_setup_wizard, serve_sw,
};
pub use auth::proxy_dispatch_layer;
use auth::{admin_auth_layer, security_headers};
use breakpoints::{
    add_bp_rule, delete_bp_rule, list_bp_rules, list_pending_bp, resolve_bp, update_bp_rule,
};
use extensions::{
    create_mock_rule, create_script, delete_mock_rule, delete_script, list_mock_rules,
    list_plugins, list_scripts, reset_mock_rule, start_playback, update_mock_rule, update_script,
};
use forward::forward_request;
pub(crate) use metrics::{SharedEndpointMetrics, new_endpoint_metrics};
use metrics::{build_metrics_payload, endpoint_timing_payload, record_endpoint_timing};
use policy::{
    add_header_map, add_modification, add_rewrite, delete_dns, delete_header_map, delete_map_local,
    delete_modification, delete_rewrite, get_capture_filter, get_throttling, list_dns,
    list_header_maps, list_map_local, list_modifications, list_rewrites, list_routes,
    replace_all_rewrites, set_map_local, update_capture_filter, update_dns, update_header_map,
    update_rewrite, update_routes, update_throttling,
};
use sessions::{
    annotate_session, clear_sessions, diff_sessions, export_har, export_session, get_session,
    get_session_timing, get_ws_frames, import_curl, import_har, import_sessions, list_sessions,
    load_sessions, save_sessions, sessions_stream,
};
use settings::{
    get_ca_cert, get_config, get_socks5_status, get_upstream_proxy, reload_config,
    set_upstream_proxy_handler,
};
use webhooks::{create_webhook, delete_webhook, list_webhooks, update_webhook};

/// Builds the control-plane router: UI, admin API, static assets, and proxy fallback.
/// The caller is responsible for applying the proxy-dispatch layer on top.
pub fn control_plane_router(state: Arc<AppState>) -> Router {
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
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            admin_auth_layer,
        ))
        .layer(axum::middleware::from_fn(security_headers))
        .with_state(state)
}

// ── Security helpers ───────────────────────────────────────────────────────────

fn admin_egress_policy_response(error: String) -> axum::response::Response {
    (
        axum::http::StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": error })),
    )
        .into_response()
}

/// Fallback handler: proxies any request that didn't match a control-plane route.
async fn proxy_fallback(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    state.proxy_engine.clone().handle_request(req).await
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

// ── Health ─────────────────────────────────────────────────────────────────────

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok",
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "mitm_enabled": state.proxy_engine.mitm_enabled,
    }))
}

fn storage_error_response(error: std::io::Error) -> axum::response::Response {
    tracing::warn!(error = %error, "Failed to persist control-plane state");
    (
        axum::http::StatusCode::INSUFFICIENT_STORAGE,
        axum::Json(serde_json::json!({
            "error": format!("failed to persist state: {error}")
        })),
    )
        .into_response()
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
