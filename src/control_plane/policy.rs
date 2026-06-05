use axum::{extract::State, response::IntoResponse};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::plugins::access_control::AccessRule;
use crate::middleware::plugins::capture_filter::CaptureFilterConfig;
use crate::middleware::plugins::map_local::MapLocalRule;
use crate::middleware::plugins::map_remote::MapRemoteRule;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::middleware::plugins::rules::RewriteRuleSet;
use crate::storage;

use super::storage_error_response;

// ── Access control rules ───────────────────────────────────────────────────────

pub(super) async fn list_access_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.access_rules.read().await.clone())
}

pub(super) async fn create_access_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<AccessRule>,
) -> impl IntoResponse {
    rule.id = AccessRule::new_id();
    let saved = rule.clone();
    state.access_rules.write().await.push(rule);
    let rules = state.access_rules.read().await.clone();
    if let Err(e) = storage::save_access_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn update_access_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<AccessRule>,
) -> impl IntoResponse {
    rule.id = id.clone();
    {
        let mut rules = state.access_rules.write().await;
        match rules.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = rule,
            None => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    let rules = state.access_rules.read().await.clone();
    if let Err(e) = storage::save_access_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_access_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut rules = state.access_rules.write().await;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }
    let rules = state.access_rules.read().await.clone();
    if let Err(e) = storage::save_access_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Throttling ─────────────────────────────────────────────────────────────────

pub(super) async fn get_throttling(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.throttling_config.read().await.clone())
}

pub(super) async fn update_throttling(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_config): axum::extract::Json<ThrottlingConfig>,
) -> impl IntoResponse {
    let mut config = state.throttling_config.write().await;
    *config = new_config;
    if let Err(e) = storage::save_throttle(&state.storage_path, &config).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Unified rule sets ──────────────────────────────────────────────────────────

pub(super) async fn list_rule_sets(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.rule_sets.read().await.clone())
}

pub(super) async fn create_rule_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<RewriteRuleSet>,
) -> impl IntoResponse {
    rule.id = RewriteRuleSet::new_id();
    let saved = rule.clone();
    state.rule_sets.write().await.push(rule);
    let rules = state.rule_sets.read().await.clone();
    if let Err(e) = storage::save_rule_sets(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn get_rule_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let rules = state.rule_sets.read().await;
    match rules.iter().find(|r| r.id == id) {
        Some(r) => axum::Json(r.clone()).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn update_rule_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<RewriteRuleSet>,
) -> impl IntoResponse {
    rule.id = id.clone();
    {
        let mut rules = state.rule_sets.write().await;
        match rules.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = rule,
            None => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    let rules = state.rule_sets.read().await.clone();
    if let Err(e) = storage::save_rule_sets(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_rule_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut rules = state.rule_sets.write().await;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }
    let rules = state.rule_sets.read().await.clone();
    if let Err(e) = storage::save_rule_sets(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Map Local rules ────────────────────────────────────────────────────────────

pub(super) async fn list_map_local_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.map_local_rules.read().await.clone())
}

/// Validate that a MapLocalRule's file_path exists and is accessible.
/// Returns an error response if it doesn't so operators discover path issues
/// at rule-save time instead of silently at request time.
fn validate_map_local_path(rule: &MapLocalRule) -> Option<axum::response::Response> {
    let p = std::path::Path::new(&rule.file_path);
    if !p.exists() {
        return Some(
            (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": format!(
                        "file_path '{}' does not exist or is not accessible from this process. \
                         In containerized deployments ensure the path is mounted inside the container.",
                        rule.file_path
                    )
                })),
            )
                .into_response(),
        );
    }
    None
}

pub(super) async fn create_map_local_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<MapLocalRule>,
) -> impl IntoResponse {
    rule.id = MapLocalRule::new_id();
    if let Some(err) = validate_map_local_path(&rule) {
        return err;
    }
    let saved = rule.clone();
    state.map_local_rules.write().await.push(rule);
    let rules = state.map_local_rules.read().await.clone();
    if let Err(e) = storage::save_map_local_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn update_map_local_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<MapLocalRule>,
) -> impl IntoResponse {
    rule.id = id.clone();
    if let Some(err) = validate_map_local_path(&rule) {
        return err;
    }
    {
        let mut rules = state.map_local_rules.write().await;
        match rules.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = rule,
            None => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    let rules = state.map_local_rules.read().await.clone();
    if let Err(e) = storage::save_map_local_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_map_local_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut rules = state.map_local_rules.write().await;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }
    let rules = state.map_local_rules.read().await.clone();
    if let Err(e) = storage::save_map_local_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Map Remote rules ───────────────────────────────────────────────────────────

pub(super) async fn list_map_remote_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.map_remote_rules.read().await.clone())
}

pub(super) async fn create_map_remote_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<MapRemoteRule>,
) -> impl IntoResponse {
    rule.id = MapRemoteRule::new_id();
    let saved = rule.clone();
    state.map_remote_rules.write().await.push(rule);
    let rules = state.map_remote_rules.read().await.clone();
    if let Err(e) = storage::save_map_remote_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn update_map_remote_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<MapRemoteRule>,
) -> impl IntoResponse {
    rule.id = id.clone();
    {
        let mut rules = state.map_remote_rules.write().await;
        match rules.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = rule,
            None => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    let rules = state.map_remote_rules.read().await.clone();
    if let Err(e) = storage::save_map_remote_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_map_remote_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut rules = state.map_remote_rules.write().await;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }
    let rules = state.map_remote_rules.read().await.clone();
    if let Err(e) = storage::save_map_remote_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Capture filter ─────────────────────────────────────────────────────────────

pub(super) async fn get_capture_filter(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.capture_filter.read().await.clone())
}

pub(super) async fn update_capture_filter(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_cfg): axum::extract::Json<CaptureFilterConfig>,
) -> impl IntoResponse {
    let mut cfg = state.capture_filter.write().await;
    *cfg = new_cfg;
    if let Err(e) = storage::save_capture_filter(&state.storage_path, &cfg).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── DNS overrides ──────────────────────────────────────────────────────────────

pub(super) async fn list_dns(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.dns_overrides.read().await.clone())
}

pub(super) async fn update_dns(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_map): axum::extract::Json<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let mut overrides = state.dns_overrides.write().await;
    *overrides = new_map;
    if let Err(e) = storage::save_dns_overrides(&state.storage_path, &overrides).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_dns(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut overrides = state.dns_overrides.write().await;
    if overrides.remove(&host).is_some() {
        if let Err(e) = storage::save_dns_overrides(&state.storage_path, &overrides).await {
            return storage_error_response(e);
        }
        axum::http::StatusCode::OK.into_response()
    } else {
        axum::http::StatusCode::NOT_FOUND.into_response()
    }
}
