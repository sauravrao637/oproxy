use axum::{extract::State, response::IntoResponse};
use base64::Engine as _;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::core::engine::is_binary_content_type;
use crate::middleware::{RequestContext, ResponseContext};
use crate::security::{AdminEgressPolicy, enforce_admin_egress_policy};
use crate::session::SessionSource;

use super::admin_egress_policy_response;

#[derive(serde::Deserialize)]
pub(super) struct ForwardReq {
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(serde::Serialize)]
struct ForwardResp {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
    is_binary: bool,
    session_id: String,
}

pub(super) async fn forward_request(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ForwardReq>,
) -> impl IntoResponse {
    let method = match reqwest::Method::from_bytes(req.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "error": format!("Invalid HTTP method: {}", req.method)
                })),
            )
                .into_response();
        }
    };
    let url_parsed = match reqwest::Url::parse(&req.url) {
        Ok(u) => u,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "error": format!("Invalid URL: {e}") })),
            )
                .into_response();
        }
    };
    if !matches!(url_parsed.scheme(), "http" | "https") {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": format!("Unsupported URL scheme: {}", url_parsed.scheme())
            })),
        )
            .into_response();
    }
    if let Err(e) =
        enforce_admin_egress_policy(&url_parsed, AdminEgressPolicy::from_config(&state.config))
            .await
    {
        return admin_egress_policy_response(e);
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let host = match (url_parsed.host_str(), url_parsed.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => String::new(),
    };
    let display_uri = req.url.clone();

    // Record request in session manager
    let req_ctx = RequestContext {
        method: req.method.clone(),
        uri: display_uri.clone(),
        host: host.clone(),
        headers: req.headers.clone(),
        body: req.body.clone().unwrap_or_default(),
        body_bytes: None,
    };
    let request_size_bytes = req_ctx.body.len();
    state
        .api_handler
        .session_manager
        .record_request_with_source(session_id.clone(), req_ctx, SessionSource::AdminForward);
    if req.note.is_some() || req.tags.is_some() {
        state
            .api_handler
            .session_manager
            .annotate(&session_id, req.note.clone(), req.tags.clone());
    }

    // Build and send request using the proxy engine's http client
    let mut builder = state
        .proxy_engine
        .http_client()
        .await
        .request(method, &req.url);
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
                (
                    base64::engine::general_purpose::STANDARD.encode(&bytes),
                    true,
                )
            } else {
                (String::from_utf8_lossy(&bytes).to_string(), false)
            };

            // Record response
            let res_ctx = ResponseContext {
                status,
                headers: res_headers.clone(),
                body: body.clone(),
                request_uri: display_uri,
                session_id: Some(session_id.clone()),
                ttfb_ms,
                body_ms,
                body_bytes: None,
            };
            let metrics = crate::session::InspectionMetrics {
                latency_ms: t0.elapsed().as_millis() as u64,
                request_size_bytes,
                response_size_bytes: bytes.len(),
                status_code: status,
                ttfb_ms,
                body_ms,
                ..Default::default()
            };
            state
                .api_handler
                .session_manager
                .record_response_with_metrics(session_id.clone(), res_ctx, metrics);

            axum::Json(ForwardResp {
                status,
                headers: res_headers,
                body,
                is_binary,
                session_id,
            })
            .into_response()
        }
        Err(e) => {
            let res_ctx = ResponseContext {
                status: 502,
                body: e.to_string(),
                request_uri: display_uri,
                session_id: Some(session_id.clone()),
                ..Default::default()
            };
            let metrics = crate::session::InspectionMetrics {
                latency_ms: t0.elapsed().as_millis() as u64,
                request_size_bytes,
                response_size_bytes: e.to_string().len(),
                status_code: 502,
                ttfb_ms: t0.elapsed().as_millis() as u64,
                body_ms: 0,
                ..Default::default()
            };
            state
                .api_handler
                .session_manager
                .record_response_with_metrics(session_id.clone(), res_ctx, metrics);
            (axum::http::StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}
