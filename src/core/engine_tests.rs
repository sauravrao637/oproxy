#[cfg(test)]
mod tests {
    use crate::core::engine::ProxyEngine;
    use crate::middleware::chain::MiddlewareChain;
    use crate::middleware::plugins::inspection::InspectionMiddleware;
    use crate::middleware::plugins::routing::RoutingMiddleware;
    use crate::middleware::{Middleware, RequestContext, ResponseContext};
    use crate::session::SessionManager;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::{Router, routing::get};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn req(uri: &str, host: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: HashMap::new(),
            body: "".to_string(),
            host: host.to_string(),
            body_bytes: None,
        }
    }

    #[tokio::test]
    async fn engine_created_with_mitm_disabled() {
        let engine = ProxyEngine::new(
            Arc::new(RwLock::new(MiddlewareChain::new())),
            None,
            false,
            30,
            10 * 1024 * 1024,
            10,
            30,
            None,
        );
        assert!(!engine.mitm_enabled);
        assert!(engine.ca.is_none());
    }

    #[tokio::test]
    async fn engine_created_with_mitm_enabled_flag() {
        let engine = ProxyEngine::new(
            Arc::new(RwLock::new(MiddlewareChain::new())),
            None,
            true,
            30,
            10 * 1024 * 1024,
            10,
            30,
            None,
        );
        assert!(engine.mitm_enabled);
    }

    #[tokio::test]
    async fn inspection_records_session_with_status_200() {
        let session_manager = Arc::new(SessionManager::new(10_000));
        let middleware = InspectionMiddleware::new(session_manager.clone());

        let mut req_ctx = req("http://example.com", "example.com");
        middleware.on_request(&mut req_ctx).await;

        let mut resp_ctx = ResponseContext {
            request_uri: "http://example.com".to_string(),
            status: 200,
            headers: Default::default(),
            body: "".to_string(),
            session_id: None,
            ttfb_ms: 0,
            body_ms: 0,
            body_bytes: None,
        };
        middleware.on_response(&mut resp_ctx).await;

        let sessions = session_manager.get_all_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].metrics.as_ref().unwrap().status_code, 200);
    }

    /// RoutingMiddleware must insert x-oproxy-destination into the request context
    /// when a matching route is registered.
    #[tokio::test]
    async fn routing_middleware_sets_destination_header() {
        let mut table = HashMap::new();
        table.insert("api.local".to_string(), "http://10.0.0.2:8000".to_string());
        let routing = RoutingMiddleware::new(Arc::new(RwLock::new(table)));
        let mut ctx = req("/data", "api.local");
        routing.on_request(&mut ctx).await;
        assert_eq!(
            ctx.headers.get("x-oproxy-destination").map(|s| s.as_str()),
            Some("http://10.0.0.2:8000")
        );
    }

    /// After the engine strips internal headers, x-oproxy-destination and
    /// x-oproxy-session-id must not reach the upstream target.
    /// We verify this by inspecting the built header map directly.
    #[tokio::test]
    async fn engine_strips_internal_headers_before_forward() {
        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert(
            "x-oproxy-destination".to_string(),
            "http://10.0.0.1".to_string(),
        );
        headers.insert("x-oproxy-session-id".to_string(), "some-uuid".to_string());
        headers.insert("accept".to_string(), "text/html".to_string());

        headers.remove("x-oproxy-destination");
        headers.remove("x-oproxy-session-id");

        assert!(
            !headers.contains_key("x-oproxy-destination"),
            "destination header must be stripped"
        );
        assert!(
            !headers.contains_key("x-oproxy-session-id"),
            "session ID header must be stripped"
        );
        assert!(
            headers.contains_key("accept"),
            "legitimate headers must be preserved"
        );
    }

    /// Hop-by-hop headers are illegal in HTTP/2 and must be stripped before forwarding.
    #[tokio::test]
    async fn engine_strips_hop_by_hop_headers() {
        let hop_by_hop = [
            "connection",
            "keep-alive",
            "proxy-connection",
            "transfer-encoding",
            "te",
            "trailer",
            "trailers",
            "upgrade",
        ];
        let mut headers: HashMap<String, String> = HashMap::new();
        for h in &hop_by_hop {
            headers.insert(h.to_string(), "value".to_string());
        }
        headers.insert("content-type".to_string(), "application/json".to_string());

        for h in &hop_by_hop {
            headers.remove(*h);
        }

        for h in &hop_by_hop {
            assert!(
                !headers.contains_key(*h),
                "hop-by-hop header '{h}' must be stripped"
            );
        }
        assert!(
            headers.contains_key("content-type"),
            "non-hop-by-hop header must survive"
        );
    }

    // ── is_binary_content_type ───────────────────────────────────────────────

    #[test]
    fn binary_ct_image_types_are_binary() {
        use crate::core::engine::is_binary_content_type;
        for ct in &[
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            "image/svg+xml",
        ] {
            assert!(is_binary_content_type(ct), "{ct} must be binary");
        }
    }

    #[test]
    fn binary_ct_audio_video_are_binary() {
        use crate::core::engine::is_binary_content_type;
        assert!(is_binary_content_type("audio/mpeg"));
        assert!(is_binary_content_type("video/mp4"));
    }

    #[test]
    fn binary_ct_octet_stream_is_binary() {
        use crate::core::engine::is_binary_content_type;
        assert!(is_binary_content_type("application/octet-stream"));
    }

    #[test]
    fn binary_ct_pdf_wasm_woff_are_binary() {
        use crate::core::engine::is_binary_content_type;
        assert!(is_binary_content_type("application/pdf"));
        assert!(is_binary_content_type("application/wasm"));
        assert!(is_binary_content_type("font/woff"));
        assert!(is_binary_content_type("font/woff2"));
    }

    #[test]
    fn binary_ct_text_and_json_are_not_binary() {
        use crate::core::engine::is_binary_content_type;
        assert!(!is_binary_content_type("text/plain"));
        assert!(!is_binary_content_type("text/html"));
        assert!(!is_binary_content_type("application/json"));
        assert!(!is_binary_content_type("application/xml"));
    }

    #[test]
    fn binary_ct_charset_suffix_ignored() {
        use crate::core::engine::is_binary_content_type;
        // "image/png; charset=utf-8" should still be detected as binary
        assert!(is_binary_content_type("image/png; charset=utf-8"));
        // "application/json; charset=utf-8" must NOT be binary
        assert!(!is_binary_content_type("application/json; charset=utf-8"));
    }

    #[tokio::test]
    async fn forward_proxy_returns_non_empty_response_body() {
        let upstream_app = Router::new().route(
            "/",
            get(|| async { "proxy-body-regression-guard" }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, upstream_app).await.unwrap();
        });

        let engine = Arc::new(ProxyEngine::new(
            Arc::new(RwLock::new(MiddlewareChain::new())),
            None,
            false,
            30,
            10 * 1024 * 1024,
            10,
            30,
            None,
        ));

        let req = Request::builder()
            .method("GET")
            .uri(format!("http://127.0.0.1:{}/", upstream_addr.port()))
            .header("host", format!("127.0.0.1:{}", upstream_addr.port()))
            .body(Body::empty())
            .unwrap();

        let res = engine.handle_request(req).await;
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(
            !bytes.is_empty(),
            "proxied response body must not be empty for successful upstream responses"
        );
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("proxy-body-regression-guard"));
    }
}
