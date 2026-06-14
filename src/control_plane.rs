use axum::{Router, extract::State, response::IntoResponse, routing::get};
use std::sync::Arc;
use std::time::Instant;

use crate::AppState;

mod assets;
mod assistant;
mod assistant_action_contracts;
mod assistant_actions;
mod assistant_compat;
mod assistant_context;
mod assistant_contracts;
mod assistant_payload_repair;
mod assistant_prompt;
mod assistant_provider;
mod assistant_redaction;
mod assistant_registry;
mod assistant_tools;
mod auth;
mod breakpoints;
mod error;
mod extensions;
mod forward;
mod login;
mod metrics;
mod policy;
mod sessions;
mod settings;
mod storage_paths;
mod update;
mod webhooks;
mod workspace;

use assets::{
    not_found, robots_txt, serve_design_app_css, serve_design_app_js, serve_icon, serve_index,
    serve_manifest, serve_setup_wizard, serve_sw,
};
pub(crate) use assistant::{SharedAssistantState, new_assistant_state};
use assistant::{
    cancel_assistant_action, chat_assistant, execute_assistant_action, get_assistant_tools,
};
use assistant_compat::check_assistant_compatibility;
pub use auth::proxy_dispatch_layer;
use auth::{admin_auth_layer, security_headers};
use breakpoints::{
    add_bp_rule, delete_bp_rule, list_bp_diagnostics, list_bp_rules, list_pending_bp, resolve_bp,
    update_bp_rule,
};
use extensions::{
    create_mock_rule, create_script, delete_mock_rule, delete_script, list_mock_rules,
    list_plugins, list_scripts, reset_mock_rule, start_playback, update_mock_rule, update_script,
};
use forward::{forward_request, forward_websocket};
use login::{get_login, post_login};
pub(crate) use metrics::{SharedEndpointMetrics, new_endpoint_metrics};
use metrics::{build_metrics_payload, endpoint_timing_payload, record_endpoint_timing};
use policy::{
    create_access_rule, create_map_local_rule, create_map_remote_rule, create_rule_set,
    delete_access_rule, delete_dns, delete_map_local_fixture, delete_map_local_rule,
    delete_map_remote_rule, delete_rule_set, get_capture_filter, get_map_local_fixture,
    get_rule_set, get_throttling, list_access_rules, list_dns, list_map_local_fixtures,
    list_map_local_rules, list_map_remote_rules, list_rule_sets, reorder_rule_sets,
    test_map_remote, update_access_rule, update_capture_filter, update_dns, update_map_local_rule,
    update_map_remote_rule, update_rule_set, update_throttling, upload_map_local_fixture,
    upsert_dns,
};
use sessions::{
    annotate_session, clear_sessions, diff_sessions, export_har, export_session, get_session,
    get_session_timing, get_ws_frames, import_curl, import_har, import_sessions, list_connections,
    list_sessions, load_sessions, protocol_metrics, save_sessions, sessions_stream,
};
use settings::{
    get_ca_cert, get_ca_qr, get_config, get_socks5_status, get_upstream_proxy, reload_config,
    set_upstream_proxy_handler,
};
use update::get_update_status;
pub(crate) use update::{SharedUpdateStatus, new_update_status, refresh_update_status};
use webhooks::{create_webhook, delete_webhook, list_webhooks, update_webhook};
pub(crate) use workspace::{SharedWorkspaceState, new_workspace_state};
use workspace::{execute_workspace_action, get_workspace, patch_workspace};

/// Builds the control-plane router: UI, admin API, static assets, and proxy fallback.
/// The caller is responsible for applying the proxy-dispatch layer on top.
pub fn control_plane_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/login", get(get_login).post(post_login))
        .route("/health", get(health))
        .merge(session_routes())
        .merge(asset_routes())
        .merge(policy_routes())
        .merge(assistant_routes())
        .merge(extension_routes())
        .merge(settings_routes())
        .fallback(proxy_fallback)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            admin_auth_layer,
        ))
        .layer(axum::middleware::from_fn(security_headers))
        .with_state(state)
}

fn policy_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/admin/throttling",
            get(get_throttling)
                .put(update_throttling)
                .post(update_throttling),
        )
        .route(
            "/admin/access-rules",
            get(list_access_rules).post(create_access_rule),
        )
        .route(
            "/admin/access-rules/{id}",
            axum::routing::put(update_access_rule).delete(delete_access_rule),
        )
        .route(
            "/admin/rule-sets",
            get(list_rule_sets).post(create_rule_set),
        )
        .route(
            "/admin/rule-sets/reorder",
            axum::routing::patch(reorder_rule_sets),
        )
        .route(
            "/admin/rule-sets/{id}",
            get(get_rule_set)
                .put(update_rule_set)
                .delete(delete_rule_set),
        )
        .route(
            "/admin/capture-filter",
            get(get_capture_filter).post(update_capture_filter),
        )
        .route("/admin/dns", get(list_dns).post(update_dns))
        .route(
            "/admin/dns/{host}",
            axum::routing::put(upsert_dns).delete(delete_dns),
        )
        .route(
            "/admin/map-local-rules",
            get(list_map_local_rules).post(create_map_local_rule),
        )
        .route(
            "/admin/map-local-rules/fixtures",
            get(list_map_local_fixtures),
        )
        .route(
            "/admin/map-local-rules/fixtures/{name}",
            get(get_map_local_fixture)
                .post(upload_map_local_fixture)
                .delete(delete_map_local_fixture),
        )
        .route(
            "/admin/map-local-rules/{id}",
            axum::routing::put(update_map_local_rule).delete(delete_map_local_rule),
        )
        .route(
            "/admin/map-remote-rules",
            get(list_map_remote_rules).post(create_map_remote_rule),
        )
        .route(
            "/admin/map-remote-rules/test",
            axum::routing::post(test_map_remote),
        )
        .route(
            "/admin/map-remote-rules/{id}",
            axum::routing::put(update_map_remote_rule).delete(delete_map_remote_rule),
        )
}

fn settings_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/ca", get(get_ca_cert))
        .route("/admin/ca/qr", get(get_ca_qr))
        .route("/admin/update", get(get_update_status))
        .route("/admin/metrics", get(get_metrics))
        .route("/admin/config/reload", axum::routing::post(reload_config))
        .route("/admin/config", get(get_config))
        .route("/admin/forward", axum::routing::post(forward_request))
        .route(
            "/admin/forward/websocket",
            axum::routing::post(forward_websocket),
        )
        .route(
            "/admin/upstream-proxy",
            get(get_upstream_proxy).post(set_upstream_proxy_handler),
        )
        .route("/admin/webhooks", get(list_webhooks).post(create_webhook))
        .route(
            "/admin/webhooks/{id}",
            axum::routing::put(update_webhook).delete(delete_webhook),
        )
        .route("/admin/socks5/status", get(get_socks5_status))
        .route("/admin/setup/network-info", get(get_network_info))
}

fn assistant_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/assistant/tools", get(get_assistant_tools))
        .route("/admin/assistant/chat", axum::routing::post(chat_assistant))
        .route(
            "/admin/assistant/compatibility",
            axum::routing::post(check_assistant_compatibility),
        )
        .route(
            "/admin/assistant/actions/execute",
            axum::routing::post(execute_assistant_action),
        )
        .route(
            "/admin/assistant/actions/cancel",
            axum::routing::post(cancel_assistant_action),
        )
        .route(
            "/admin/workspace",
            get(get_workspace).patch(patch_workspace),
        )
        .route(
            "/admin/workspace/actions",
            axum::routing::post(execute_workspace_action),
        )
}

fn extension_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/playback", axum::routing::post(start_playback))
        .route("/admin/breakpoints", get(list_bp_rules).post(add_bp_rule))
        .route("/admin/breakpoints/pending", get(list_pending_bp))
        .route("/admin/breakpoints/diagnostics", get(list_bp_diagnostics))
        .route(
            "/admin/breakpoints/pending/{id}/resolve",
            axum::routing::post(resolve_bp),
        )
        .route(
            "/admin/breakpoints/{id}",
            axum::routing::put(update_bp_rule).delete(delete_bp_rule),
        )
        .route("/admin/plugins", get(list_plugins))
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
}

fn session_routes() -> Router<Arc<AppState>> {
    Router::new()
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
        .route("/api/connections", get(list_connections))
        .route("/api/metrics/protocol", get(protocol_metrics))
        .route("/api/import/curl", axum::routing::post(import_curl))
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
}

fn asset_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/setup", get(serve_setup_wizard))
        .route("/setup/mobile", get(serve_setup_wizard))
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
        .route("/favicon.ico", get(serve_icon))
        .route("/.well-known/{*path}", get(not_found))
        .route("/robots.txt", get(robots_txt))
}

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
    if matches!(req.uri().path(), path if path.starts_with("/admin/") || path.starts_with("/api/"))
    {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    state.proxy_engine.clone().handle_request(req).await
}

async fn get_network_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let lan_ip = crate::setup::public_lan_ip_for_setup();
    let port = state.config.port;
    let socks5_port = state.config.socks5_port;
    let remote_ip = lan_ip.clone().unwrap_or_else(|| "127.0.0.1".to_string());
    let lan_ip_available = lan_ip.is_some();
    let ca_url = format!("http://{}:{}/admin/ca", remote_ip, port);
    axum::Json(serde_json::json!({
        "lan_ip": remote_ip,
        "port": port,
        "socks5_port": socks5_port,
        "localhost_proxy": format!("127.0.0.1:{port}"),
        "lan_proxy": format!("{remote_ip}:{port}"),
        "ca_url": ca_url,
        "ca_local_url": format!("http://127.0.0.1:{port}/admin/ca"),
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
