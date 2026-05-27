use axum::{
    extract::State,
    http::header,
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
};
use std::sync::Arc;
use std::time::Instant;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

use crate::AppState;
use crate::api::SessionFileRequest;
use crate::diff::diff_exchanges;

use super::metrics::record_endpoint_timing;
use super::storage_paths::{resolve_storage_file_for_read, resolve_storage_file_for_write};

#[derive(serde::Deserialize, Default)]
pub(super) struct SessionQuery {
    since: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    q: Option<String>,
    include_bodies: Option<bool>,
}

pub(super) async fn list_sessions(
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
pub(super) async fn sessions_stream(
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

pub(super) async fn get_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.api_handler.get_session_details(&id).await {
        Some(detail) => axum::Json(detail).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn get_ws_frames(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.session_manager.get_session(&id) {
        Some(exchange) => axum::Json(exchange.ws_frames).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(serde::Deserialize, Default)]
pub(super) struct ExportQuery {
    format: Option<String>,
    raw: Option<bool>,
}

pub(super) async fn export_session(
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

pub(super) async fn get_session_timing(
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
pub(super) struct DiffQuery {
    a: String,
    b: String,
}

pub(super) async fn diff_sessions(
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
pub(super) struct CurlImportBody {
    curl: String,
}

pub(super) async fn import_curl(
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
pub(super) struct AnnotationPatch {
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

pub(super) async fn annotate_session(
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

pub(super) async fn clear_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.api_handler.clear_sessions().await;
    axum::http::StatusCode::OK
}

pub(super) async fn save_sessions(
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

pub(super) async fn load_sessions(
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
pub(super) struct ImportRequest {
    sessions: Vec<crate::session::Exchange>,
    #[serde(default = "bool_true")]
    merge: bool,
}

fn bool_true() -> bool {
    true
}

pub(super) async fn import_sessions(
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
pub(super) struct HarExportQuery {
    raw: Option<bool>,
    ids: Option<String>,
}

pub(super) async fn export_har(
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
pub(super) struct HarImportQuery {
    #[serde(default = "bool_true")]
    merge: bool,
}

pub(super) async fn import_har(
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
