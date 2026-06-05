use axum::{extract::State, response::IntoResponse};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::matcher::MatchMode;
use crate::middleware::plugins::breakpoints::{
    BreakpointContext, BreakpointResolution, BreakpointRule,
};
use crate::storage;

use super::storage_error_response;

pub(super) async fn list_bp_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_breakpoint_rules().await)
}

/// Validate the Location's path as a regex when mode == Regex; return an error response if
/// the pattern is malformed so callers get a clear 422 rather than a silent never-match rule.
fn validate_location(rule: &BreakpointRule) -> Option<axum::response::Response> {
    if rule.location.mode == MatchMode::Regex
        && let Some(ref path) = rule.location.path
        && let Err(e) = regex::Regex::new(path)
    {
        return Some(
            (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({ "error": format!("invalid path regex: {e}") })),
            )
                .into_response(),
        );
    }
    None
}

pub(super) async fn add_bp_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<BreakpointRule>,
) -> impl IntoResponse {
    if let Some(err) = validate_location(&rule) {
        return err;
    }
    rule.id = uuid::Uuid::new_v4().to_string();
    let saved = rule.clone();
    state.api_handler.add_breakpoint_rule(rule).await;
    let rules = state.api_handler.list_breakpoint_rules().await;
    if let Err(e) = storage::save_breakpoints(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn delete_bp_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    state.api_handler.delete_breakpoint_rule(&id).await;
    let rules = state.api_handler.list_breakpoint_rules().await;
    if let Err(e) = storage::save_breakpoints(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn update_bp_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(rule): axum::extract::Json<BreakpointRule>,
) -> impl IntoResponse {
    if let Some(err) = validate_location(&rule) {
        return err;
    }
    if !state.api_handler.update_breakpoint_rule(&id, rule).await {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let rules = state.api_handler.list_breakpoint_rules().await;
    if let Err(e) = storage::save_breakpoints(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn list_pending_bp(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.api_handler.list_pending().await)
}

#[derive(serde::Deserialize)]
pub(super) struct ResolutionRequest {
    action: String,
    context: Option<BreakpointContext>,
}

pub(super) async fn resolve_bp(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(req): axum::extract::Json<ResolutionRequest>,
) -> impl IntoResponse {
    let resolution = match req.action.as_str() {
        "drop" => BreakpointResolution::Drop,
        "modify" => req
            .context
            .map(|bc| BreakpointResolution::Modify(Box::new(bc)))
            .unwrap_or(BreakpointResolution::Continue),
        _ => BreakpointResolution::Continue,
    };
    match state.api_handler.resolve_breakpoint(id, resolution).await {
        Ok(_) => axum::http::StatusCode::OK.into_response(),
        Err(e) => (axum::http::StatusCode::NOT_FOUND, e).into_response(),
    }
}
