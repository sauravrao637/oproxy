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
        if ctx.session_id.is_some() {
            return MiddlewareAction::Continue;
        }

        // CaptureFilterMiddleware signals "don't record this host" via a typed flag.
        if ctx.skip_recording {
            return MiddlewareAction::Continue;
        }

        // Inspector data is populated by the upstream inspector middlewares into the
        // typed `ctx.inspector` side-channel.
        let inspector = std::mem::take(&mut ctx.inspector);

        let id = Uuid::new_v4().to_string();
        ctx.session_id = Some(id.clone());

        let mut recorded = ctx.clone();
        strip_internal_headers(&mut recorded.headers);
        self.session_manager.record_request(id.clone(), recorded);

        if inspector.jwt.is_some() || inspector.graphql.is_some() || inspector.grpc.is_some() {
            self.session_manager.update_inspector_data(&id, inspector);
        }

        MiddlewareAction::Continue
    }

    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        let tags = std::mem::take(&mut ctx.tags);
        // Use the session ID injected during on_request for exact lookup.
        // This fixes correlation when multiple concurrent requests target the same URI.
        let session = match ctx.session_id {
            Some(ref id) => self.session_manager.get_session(id),
            None => {
                // No correlated request session. The previous URI-match fallback was
                // removed because it mis-attributed responses under concurrent
                // same-URI requests; dropping the record is safer than corrupting one.
                tracing::warn!(
                    uri = %ctx.request_uri,
                    "response has no session_id; skipping correlation"
                );
                None
            }
        };

        if let Some(session) = session {
            if session.response.is_some() {
                return MiddlewareAction::Continue;
            }

            let latency_ms = (chrono::Utc::now() - session.timestamp).num_milliseconds() as u64;
            // `body` is now the canonical decoded bytes, so its length is the real
            // response size. Fall back to Content-Length only when the body wasn't
            // buffered (e.g. streamed responses leave it empty).
            let response_size_bytes = if ctx.body.is_empty() {
                content_length(&ctx.headers).unwrap_or(0)
            } else {
                ctx.body.len()
            };
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
                    .annotate(&session.id, None, Some(merged))
                    .await;
            }
        }

        MiddlewareAction::Continue
    }
}

fn strip_internal_headers(headers: &mut crate::middleware::HeaderMap) {
    headers.retain(|name, _| {
        let name = name.trim().to_ascii_lowercase();
        !name.starts_with("x-oproxy-")
    });
}

fn content_length(headers: &crate::middleware::HeaderMap) -> Option<usize> {
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
    use std::sync::Arc;

    fn req(uri: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: crate::middleware::HeaderMap::new(),
            body: bytes::Bytes::from_static(b"body12345"),
            host: "localhost".to_string(),
            ..Default::default()
        }
    }

    fn res(uri: &str, status: u16, body: &str) -> ResponseContext {
        ResponseContext {
            status,
            headers: crate::middleware::HeaderMap::new(),
            body: bytes::Bytes::from(body.to_string()),
            request_uri: uri.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn on_request_records_session() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/test");
        mw.on_request(&mut ctx).await;
        sm.flush().await;
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
        sm.flush().await;
        let ex = sm.get_all_sessions().pop().unwrap();
        assert!(!ex.request.headers.contains_key("x-oproxy-session-id"));
        assert!(!ex.request.headers.contains_key("x-oproxy-destination"));
    }

    #[tokio::test]
    async fn on_request_assigns_session_id() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/test");
        mw.on_request(&mut ctx).await;
        assert!(
            ctx.session_id.is_some(),
            "session ID must be assigned on the context"
        );
        assert!(
            !ctx.headers.contains_key("x-oproxy-session-id"),
            "session ID must not leak into forwarded headers"
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
        sm.flush().await;
        let mut rs = res("/check", 201, "resp-body");
        rs.session_id = rq.session_id.clone();
        mw.on_response(&mut rs).await;
        sm.flush().await;
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
        sm.flush().await;
        let mut rs = res("/sizes", 200, "response-payload");
        rs.session_id = rq.session_id.clone();
        mw.on_response(&mut rs).await;
        sm.flush().await;
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
        sm.flush().await;
        let mut rs = res("/binary", 200, "");
        rs.session_id = rq.session_id.clone();
        rs.body = bytes::Bytes::from_static(&[1, 2, 3, 4]);
        mw.on_response(&mut rs).await;
        sm.flush().await;
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
        sm.flush().await;
        let mut rs = res("/large", 200, "");
        rs.session_id = rq.session_id.clone();
        rs.headers
            .insert("Content-Length".to_string(), "614400".to_string());
        mw.on_response(&mut rs).await;
        sm.flush().await;
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
        sm.flush().await;
        let session_id = rq.session_id.clone().unwrap();
        let mut rs = res("/mocked", 200, "ok");
        rs.session_id = Some(session_id.clone());
        rs.tags = vec!["mock".to_string(), "replay".to_string()];

        mw.on_response(&mut rs).await;
        sm.flush().await;

        let ex = sm.get_session(&session_id).unwrap();
        assert_eq!(ex.tags, vec!["mock".to_string(), "replay".to_string()]);
    }

    #[tokio::test]
    async fn on_response_returns_continue() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rq = req("/test");
        mw.on_request(&mut rq).await;
        sm.flush().await;
        let mut rs = res("/test", 200, "");
        assert_eq!(mw.on_response(&mut rs).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn skip_recording_flag_prevents_session_creation() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut ctx = req("/filtered");
        ctx.skip_recording = true;
        let action = mw.on_request(&mut ctx).await;
        sm.flush().await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert!(
            sm.get_all_sessions().is_empty(),
            "filtered host must not be recorded"
        );
    }

    #[tokio::test]
    async fn on_response_with_no_prior_request_is_safe() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());
        let mut rs = res("/orphan", 200, "body");
        // Must not panic, sessions store must remain empty
        let action = mw.on_response(&mut rs).await;
        sm.flush().await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert!(sm.get_all_sessions().is_empty());
    }
}
