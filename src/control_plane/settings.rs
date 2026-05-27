use axum::{extract::State, response::IntoResponse};
use std::sync::Arc;

use crate::AppState;
use crate::storage;

use super::storage_error_response;

#[derive(serde::Deserialize, Default)]
pub(super) struct HotReloadBody {
    max_body_bytes: Option<usize>,
}

pub(super) async fn reload_config(
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
        // No value supplied - re-read from YAML.
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

pub(super) async fn get_ca_cert(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match &state.proxy_engine.ca {
        Some(ca) => {
            let cert = ca.get_root_cert_pem();
            (
                axum::http::StatusCode::OK,
                [
                    ("Content-Type", "application/x-pem-file"),
                    (
                        "Content-Disposition",
                        "attachment; filename=\"oproxy-ca.pem\"",
                    ),
                ],
                cert,
            )
                .into_response()
        }
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();
    axum::Json(serde_json::json!({
        "port": state.config.port,
        "bind_host": state.config.bind_host,
        "mitm_enabled": state.config.mitm.enabled,
        "max_body_bytes": state.proxy_engine.max_body_bytes(),
        "max_sessions": state.config.max_sessions,
        "max_retained_body_bytes": state.config.max_retained_body_bytes,
        "max_connections": state.config.max_connections,
        "timeout_secs": state.config.timeout_secs,
        "connect_timeout_secs": state.config.connect_timeout_secs,
        "handshake_timeout_secs": state.config.handshake_timeout_secs,
        "shutdown_grace_secs": state.config.shutdown_grace_secs,
        "inspect_ws_frames": state.config.inspect_ws_frames,
        "allow_remote_admin": state.config.allow_remote_admin,
        "allow_private_admin_egress": state.config.allow_private_admin_egress,
        "admin_auth_enabled": state.config.admin_token.as_deref().is_some_and(|token| !token.trim().is_empty()),
        "storage_path": state.storage_path.display().to_string(),
        "uptime_secs": uptime,
    }))
}

pub(super) async fn get_upstream_proxy(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let url = storage::load_upstream_proxy(&state.storage_path)
        .or_else(|| state.config.upstream_proxy.clone());
    axum::Json(serde_json::json!({ "upstream_proxy": url }))
}

#[derive(serde::Deserialize)]
pub(super) struct UpstreamProxyBody {
    /// Empty string or null to disable. Valid URL to enable.
    upstream_proxy: Option<String>,
}

pub(super) async fn set_upstream_proxy_handler(
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

pub(super) async fn get_socks5_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let enabled = state.config.socks5_port.is_some();
    let mitm_active = enabled && state.proxy_engine.mitm_enabled;
    axum::Json(serde_json::json!({
        "enabled": enabled,
        "port": state.config.socks5_port,
        "mode": if mitm_active { "mitm" } else { "tunnel-only" },
        "captures_sessions": mitm_active,
    }))
}
