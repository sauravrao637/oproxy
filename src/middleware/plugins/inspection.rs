use crate::middleware::{
    BodyObserver, Middleware, MiddlewareAction, RequestContext, ResponseContext,
};
use crate::session::SharedSessionManager;
use async_trait::async_trait;
use bytes::Bytes;
use uuid::Uuid;

pub struct InspectionMiddleware {
    session_manager: SharedSessionManager,
    label: &'static str,
}

impl InspectionMiddleware {
    pub fn new(session_manager: SharedSessionManager) -> Self {
        Self {
            session_manager,
            label: "InspectionMiddleware",
        }
    }

    /// Second-pass instance that records responses from short-circuit middlewares
    /// (MapLocal, Mock, Lua). Shown as a distinct entry in /admin/plugins.
    pub fn new_response_pass(session_manager: SharedSessionManager) -> Self {
        Self {
            session_manager,
            label: "InspectionMiddleware:response-pass",
        }
    }
}

#[async_trait]
impl Middleware for InspectionMiddleware {
    fn name(&self) -> &str {
        self.label
    }

    fn body_hint(&self, _head: &RequestContext) -> crate::core::forward::BodyHint {
        crate::core::forward::BodyHint::StreamingInspect {
            granularity: crate::core::forward::Granularity::Bytes,
        }
    }

    fn stream_observer(&self, req: &RequestContext) -> Option<Box<dyn BodyObserver>> {
        if req.skip_recording {
            return None;
        }
        let session_id = req.session_id.clone()?;
        Some(Box::new(InspectionObserver {
            session_manager: self.session_manager.clone(),
            session_id,
            res_ctx: None,
            start: None,
            byte_count: 0,
        }))
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
        // On the streaming path the engine sets this flag and collects a BodyObserver
        // instead. Recording happens in InspectionObserver::finish after the body is
        // fully streamed, so we have the correct byte count. Skip here to avoid a
        // double-record with an empty body.
        if ctx.response_body_observer_pending {
            return MiddlewareAction::Continue;
        }
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
                protocol: ctx.protocol.clone(),
                ..Default::default()
            };
            // Telemetry: emit a per-exchange OTel span. Gated to the
            // `otel` feature — the finalized-exchange clone only happens when the
            // feature is on, so default builds pay nothing.
            #[cfg(feature = "otel")]
            {
                let mut finalized = session.clone();
                finalized.response = Some(ctx.clone());
                finalized.metrics = Some(metrics.clone());
                crate::telemetry::export_exchange(&finalized);
            }
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

/// Per-stream observer that records an exchange after the response body has
/// been fully streamed. Created by [`InspectionMiddleware::stream_observer`];
/// driven by `forward_stream` in the engine.
struct InspectionObserver {
    session_manager: SharedSessionManager,
    session_id: String,
    /// Set in `on_response_head` once the upstream response head is available.
    res_ctx: Option<ResponseContext>,
    start: Option<std::time::Instant>,
    byte_count: u64,
}

#[async_trait]
impl BodyObserver for InspectionObserver {
    async fn on_response_head(&mut self, res: &ResponseContext, start: std::time::Instant) {
        self.res_ctx = Some(res.clone());
        self.start = Some(start);
    }

    async fn on_chunk(&mut self, chunk: Bytes) -> Option<Bytes> {
        self.byte_count += chunk.len() as u64;
        Some(chunk)
    }

    async fn finish(self: Box<Self>) {
        let Some(res_ctx) = self.res_ctx else { return };
        let start = self.start.unwrap_or_else(std::time::Instant::now);
        let latency_ms = start.elapsed().as_millis() as u64;
        let metrics = crate::session::InspectionMetrics {
            latency_ms,
            request_size_bytes: 0, // body not buffered on streaming path
            response_size_bytes: self.byte_count as usize,
            status_code: res_ctx.status,
            ttfb_ms: res_ctx.ttfb_ms,
            body_ms: latency_ms.saturating_sub(res_ctx.ttfb_ms),
            protocol: res_ctx.protocol.clone(),
            ..Default::default()
        };
        let tags = res_ctx.tags.clone();
        self.session_manager.record_response_with_metrics(
            self.session_id.clone(),
            res_ctx,
            metrics,
        );
        if !tags.is_empty() {
            self.session_manager
                .annotate(&self.session_id, None, Some(tags))
                .await;
        }
    }
}

/// Internal engine headers that must never appear in session recordings.
/// Only THESE specific names are stripped. User-defined rewrite rules that
/// inject headers with `x-oproxy-*` names are preserved so users can verify
/// the injected values in the session detail panel.
const INTERNAL_HEADERS: &[&str] = &[
    "x-oproxy-session-id",
    "x-oproxy-destination",
    "x-oproxy-mock-response",
    "x-oproxy-tags",
    "x-oproxy-grpc-direction",
    "x-oproxy-grpc-compressed",
    "x-oproxy-frame-direction",
    "x-oproxy-frame-opcode",
    "x-oproxy-admin-token",
];

fn strip_internal_headers(headers: &mut crate::middleware::HeaderMap) {
    headers.retain(|name, _| {
        let name = name.trim().to_ascii_lowercase();
        !INTERNAL_HEADERS.contains(&name.as_str())
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
    async fn stream_observer_records_correct_byte_count() {
        let sm = Arc::new(SessionManager::new(10_000));
        let mw = InspectionMiddleware::new(sm.clone());

        // Simulate on_request recording the session and setting a session_id.
        let mut rq = req("/stream");
        mw.on_request(&mut rq).await;
        sm.flush().await;
        let session_id = rq.session_id.clone().unwrap();

        // Simulate the response head arriving (no body yet).
        let mut rs = ResponseContext {
            status: 200,
            headers: crate::middleware::HeaderMap::new(),
            request_uri: "/stream".to_string(),
            session_id: rq.session_id.clone(),
            response_body_observer_pending: true,
            ..Default::default()
        };

        // on_response must skip recording because the observer is pending.
        mw.on_response(&mut rs).await;
        sm.flush().await;
        assert!(
            sm.get_session(&session_id).unwrap().response.is_none(),
            "on_response must not record when observer is pending"
        );

        // Create the observer and feed chunks.
        let start = std::time::Instant::now();
        let mut obs = mw
            .stream_observer(&rq)
            .expect("stream_observer must return Some");
        obs.on_response_head(&rs, start).await;
        let chunk1 = bytes::Bytes::from_static(b"hello ");
        let chunk2 = bytes::Bytes::from_static(b"world");
        assert_eq!(obs.on_chunk(chunk1.clone()).await, Some(chunk1));
        assert_eq!(obs.on_chunk(chunk2.clone()).await, Some(chunk2));
        obs.finish().await;
        sm.flush().await;

        let ex = sm.get_session(&session_id).unwrap();
        let m = ex.metrics.as_ref().expect("metrics must be recorded");
        assert_eq!(m.status_code, 200);
        assert_eq!(m.response_size_bytes, 11, "must count bytes across chunks");
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
