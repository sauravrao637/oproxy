use axum::{extract::State, response::IntoResponse};
use std::sync::Arc;

use crate::AppState;
use crate::security::{AdminEgressPolicy, enforce_admin_egress_policy};
use crate::storage;
use crate::webhooks::{WebhookConfig, sanitize_webhook_events};

use super::{admin_egress_policy_response, storage_error_response};

pub(super) async fn list_webhooks(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let hooks = state.webhooks.read().await.clone();
    axum::Json(hooks)
}

pub(super) async fn create_webhook(
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
    let webhook_url = match reqwest::Url::parse(&hook.url) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url,
        Ok(url) => {
            return (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": format!("unsupported webhook URL scheme: {}", url.scheme())
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({ "error": format!("invalid webhook URL: {e}") })),
            )
                .into_response();
        }
    };
    if let Err(e) =
        enforce_admin_egress_policy(&webhook_url, AdminEgressPolicy::from_config(&state.config))
            .await
    {
        return admin_egress_policy_response(e);
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

pub(super) async fn update_webhook(
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
    let webhook_url = match reqwest::Url::parse(&updated.url) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url,
        Ok(url) => {
            return (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": format!("unsupported webhook URL scheme: {}", url.scheme())
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({ "error": format!("invalid webhook URL: {e}") })),
            )
                .into_response();
        }
    };
    if let Err(e) =
        enforce_admin_egress_policy(&webhook_url, AdminEgressPolicy::from_config(&state.config))
            .await
    {
        return admin_egress_policy_response(e);
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

pub(super) async fn delete_webhook(
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
