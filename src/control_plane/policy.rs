use axum::{extract::State, response::IntoResponse};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::plugins::capture_filter::CaptureFilterConfig;
use crate::middleware::plugins::modification::ModificationRule;
use crate::middleware::plugins::rewrite::RewriteRule;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::storage;

use super::storage_error_response;
use super::storage_paths::resolve_storage_file_for_read;

pub(super) async fn list_routes(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.routing_table.read().await.clone())
}

pub(super) async fn update_routes(
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

pub(super) async fn get_throttling(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.throttling_config.read().await.clone())
}

pub(super) async fn update_throttling(
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

pub(super) async fn list_rewrites(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_rewrite_rules().await)
}

pub(super) async fn add_rewrite(
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

pub(super) async fn delete_rewrite(
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

pub(super) async fn update_rewrite(
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

pub(super) async fn replace_all_rewrites(
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

pub(super) async fn list_header_maps(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_header_maps().await)
}

pub(super) async fn add_header_map(
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

pub(super) async fn update_header_map(
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

pub(super) async fn delete_header_map(
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

pub(super) async fn get_capture_filter(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.capture_filter.read().await.clone())
}

pub(super) async fn update_capture_filter(
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

pub(super) async fn list_dns(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.dns_overrides.read().await.clone())
}

pub(super) async fn update_dns(
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

pub(super) async fn delete_dns(
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

pub(super) async fn list_modifications(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_modifications().await)
}

pub(super) async fn add_modification(
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

pub(super) async fn delete_modification(
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
pub(super) struct MapLocalEntry {
    host: String,
    file_path: String,
}

pub(super) async fn list_map_local(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let map = state.map_local.read().await.clone();
    axum::Json(map)
}

pub(super) async fn set_map_local(
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

pub(super) async fn delete_map_local(
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
