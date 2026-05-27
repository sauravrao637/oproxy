use crate::middleware::plugins::capture_filter::SKIP_RECORDING_HEADER;
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use crate::session::SharedSessionManager;
use async_trait::async_trait;
use uuid::Uuid;

pub struct InspectionMiddleware {
    session_manager: SharedSessionManager,
}

impl InspectionMiddleware {
    pub fn new(session_manager: SharedSessionManager) -> Self {
        Self { session_manager }
    }
}

#[async_trait]
impl Middleware for InspectionMiddleware {
    fn name(&self) -> &str {
        "InspectionMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        // CaptureFilterMiddleware signals "don't record this host" via a header.
        // Strip it here so it never leaks to the upstream server.
        if ctx.headers.remove(SKIP_RECORDING_HEADER).is_some() {
            return MiddlewareAction::Continue;
        }

        // Extract inspector data injected by upstream inspector middlewares, then strip.
        let jwt_info: Option<crate::session::JwtInfo> = ctx
            .headers
            .remove("x-oproxy-jwt")
            .and_then(|v| serde_json::from_str(&v).ok());
        let graphql_info: Option<crate::session::GraphQLInfo> = ctx
            .headers
            .remove("x-oproxy-graphql")
            .and_then(|v| serde_json::from_str(&v).ok());
        let grpc_info: Option<crate::session::GrpcInfo> = ctx
            .headers
            .remove("x-oproxy-grpc")
            .and_then(|v| serde_json::from_str(&v).ok());

        let id = Uuid::new_v4().to_string();
        ctx.headers
            .insert("x-oproxy-session-id".to_string(), id.clone());

        let mut recorded = ctx.clone();
        strip_internal_headers(&mut recorded.headers);
        self.session_manager.record_request(id.clone(), recorded);

        if jwt_info.is_some() || graphql_info.is_some() || grpc_info.is_some() {
            let data = crate::session::InspectorData {
                jwt: jwt_info,
                graphql: graphql_info,
                grpc: grpc_info,
            };
            self.session_manager.update_inspector_data(&id, data);
        }

        MiddlewareAction::Continue
    }

    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        let tags: Vec<String> = ctx
            .headers
            .remove("x-oproxy-tags")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        // Use the session ID injected during on_request for exact lookup.
        // This fixes correlation when multiple concurrent requests target the same URI.
        let session = if let Some(ref id) = ctx.session_id {
            self.session_manager.get_session(id)
        } else {
            // Fallback: URI match (best-effort, breaks under concurrent same-URI requests)
            self.session_manager
                .get_all_sessions()
                .into_iter()
                .find(|s| s.request.uri == ctx.request_uri && s.response.is_none())
        };

        if let Some(session) = session {
            let latency_ms = (chrono::Utc::now() - session.timestamp).num_milliseconds() as u64;
            let response_size_bytes = ctx
                .body_bytes
                .as_ref()
                .map(|bytes| bytes.len())
                .or_else(|| content_length(&ctx.headers))
                .unwrap_or(ctx.body.len());
            let metrics = crate::session::InspectionMetrics {
                latency_ms,
                request_size_bytes: session.request.body.len(),
                response_size_bytes,
                status_code: ctx.status,
                ttfb_ms: ctx.ttfb_ms,
                body_ms: ctx.body_ms,
                ..Default::default()
            };
            self.session_manager.record_response_with_metrics(
                session.id.clone(),
                ctx.clone(),
                metrics,
            );
            if !tags.is_empty() {
                let mut merged = session.tags.clone();
                for tag in tags {
                    if !merged.iter().any(|existing| existing == &tag) {
                        merged.push(tag);
                    }
                }
                self.session_manager
                    .annotate(&session.id, None, Some(merged));
            }
        }

        MiddlewareAction::Continue
    }
}

fn strip_internal_headers(headers: &mut std::collections::HashMap<String, String>) {
    headers.retain(|name, _| {
        let name = name.trim().to_ascii_lowercase();
        !name.starts_with("x-oproxy-")
    });
}

fn content_length(headers: &std::collections::HashMap<String, String>) -> Option<usize> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
    use crate::session::SessionManager;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn req(uri: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: HashMap::new(),
            body: "body12345".to_string(),
            host: "localhost".to_string(),
            body_bytes: None,
        }
    }

    fn res(uri: &str, status: u16, body: &str) -> ResponseContext {
        ResponseContext {
            status,
            headers: HashMap::new(),
            body: body.to_string(),
            request_uri: uri.to_string(),
            session_id: None,
            ttfb_ms: 0,
            body_ms: 0,
            body_bytes: None,
        }
    }

    #[tokio::test]
    async fn on_request_records_session() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/test");
        mw.on_request(&mut ctx).await;
        assert_eq!(sm.get_all_sessions().len(), 1);
    }

    #[tokio::test]
    async fn on_request_does_not_store_internal_proxy_headers() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/test");
        ctx.headers.insert(
            "x-oproxy-destination".to_string(),
            "https://example.com".to_string(),
        );
        mw.on_request(&mut ctx).await;
        let ex = sm.get_all_sessions().pop().unwrap();
        assert!(!ex.request.headers.contains_key("x-oproxy-session-id"));
        assert!(!ex.request.headers.contains_key("x-oproxy-destination"));
    }

    #[tokio::test]
    async fn on_request_injects_session_id_header() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/test");
        mw.on_request(&mut ctx).await;
        assert!(
            ctx.headers.contains_key("x-oproxy-session-id"),
            "session ID header must be injected"
        );
    }

    #[tokio::test]
    async fn on_request_returns_continue() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/test");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn on_response_records_metrics_with_correct_status() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rq = req("/check");
        mw.on_request(&mut rq).await;
        let mut rs = res("/check", 201, "resp-body");
        mw.on_response(&mut rs).await;
        let sessions = sm.get_all_sessions();
        let m = sessions[0]
            .metrics
            .as_ref()
            .expect("metrics must be recorded");
        assert_eq!(m.status_code, 201);
    }

    #[tokio::test]
    async fn on_response_records_body_sizes() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rq = req("/sizes");
        mw.on_request(&mut rq).await;
        let mut rs = res("/sizes", 200, "response-payload");
        mw.on_response(&mut rs).await;
        let sessions = sm.get_all_sessions();
        let m = sessions[0].metrics.as_ref().unwrap();
        assert_eq!(m.request_size_bytes, "body12345".len());
        assert_eq!(m.response_size_bytes, "response-payload".len());
    }

    #[tokio::test]
    async fn on_response_records_binary_byte_size_instead_of_base64_len() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rq = req("/binary");
        mw.on_request(&mut rq).await;
        let mut rs = res("/binary", 200, "AQIDBA==");
        rs.body_bytes = Some(bytes::Bytes::from_static(&[1, 2, 3, 4]));
        mw.on_response(&mut rs).await;
        let sessions = sm.get_all_sessions();
        let m = sessions[0].metrics.as_ref().unwrap();
        assert_eq!(m.response_size_bytes, 4);
    }

    #[tokio::test]
    async fn on_response_records_streamed_content_length_when_body_not_retained() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rq = req("/large");
        mw.on_request(&mut rq).await;
        let mut rs = res("/large", 200, "");
        rs.headers
            .insert("Content-Length".to_string(), "614400".to_string());
        mw.on_response(&mut rs).await;
        let sessions = sm.get_all_sessions();
        let m = sessions[0].metrics.as_ref().unwrap();
        assert_eq!(m.response_size_bytes, 614400);
    }

    #[tokio::test]
    async fn on_response_applies_internal_tags_without_leaking_header() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rq = req("/mocked");
        mw.on_request(&mut rq).await;
        let session_id = rq.headers.get("x-oproxy-session-id").cloned().unwrap();
        let mut rs = res("/mocked", 200, "ok");
        rs.session_id = Some(session_id.clone());
        rs.headers
            .insert("x-oproxy-tags".to_string(), "mock,replay".to_string());

        mw.on_response(&mut rs).await;

        let ex = sm.get_session(&session_id).unwrap();
        assert_eq!(ex.tags, vec!["mock".to_string(), "replay".to_string()]);
        assert!(!rs.headers.contains_key("x-oproxy-tags"));
    }

    #[tokio::test]
    async fn on_response_returns_continue() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rq = req("/test");
        mw.on_request(&mut rq).await;
        let mut rs = res("/test", 200, "");
        assert_eq!(mw.on_response(&mut rs).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn skip_recording_header_prevents_session_creation() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/filtered");
        ctx.headers.insert(
            crate::middleware::plugins::capture_filter::SKIP_RECORDING_HEADER.to_string(),
            "true".to_string(),
        );
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert!(
            sm.get_all_sessions().is_empty(),
            "filtered host must not be recorded"
        );
    }

    #[tokio::test]
    async fn skip_recording_header_is_stripped_from_context() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/filtered");
        ctx.headers.insert(
            crate::middleware::plugins::capture_filter::SKIP_RECORDING_HEADER.to_string(),
            "true".to_string(),
        );
        mw.on_request(&mut ctx).await;
        assert!(
            !ctx.headers
                .contains_key(crate::middleware::plugins::capture_filter::SKIP_RECORDING_HEADER),
            "skip header must be removed so it never reaches upstream"
        );
    }

    #[tokio::test]
    async fn on_response_with_no_prior_request_is_safe() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rs = res("/orphan", 200, "body");
        // Must not panic, sessions store must remain empty
        let action = mw.on_response(&mut rs).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert!(sm.get_all_sessions().is_empty());
    }
}
