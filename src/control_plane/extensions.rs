use axum::{extract::State, response::IntoResponse};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::plugins::lua_engine::LuaScript;
use crate::middleware::plugins::mock::MockRule;
use crate::storage;

use super::storage_error_response;

pub(super) async fn start_playback(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.api_handler.start_playback().await;
    axum::http::StatusCode::OK
}

pub(super) async fn list_plugins(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let chain = state.middleware_chain.read().await;
    axum::Json(serde_json::json!({ "plugins": chain.list_plugins() }))
}

pub(super) async fn list_mock_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.mock_rules.read().await.clone())
}

pub(super) async fn create_mock_rule(
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

pub(super) async fn update_mock_rule(
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

pub(super) async fn delete_mock_rule(
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

pub(super) async fn reset_mock_rule(
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

pub(super) async fn list_scripts(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.lua_scripts.read().await.clone())
}

pub(super) async fn create_script(
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

pub(super) async fn update_script(
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

pub(super) async fn delete_script(
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
