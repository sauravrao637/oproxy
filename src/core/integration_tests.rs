#[cfg(test)]
mod tests {
    use crate::core::engine::ProxyEngine;
    use crate::middleware::chain::MiddlewareChain;
    use crate::middleware::plugins::capture_filter::{
        CaptureFilterConfig, CaptureFilterMiddleware, FilterMode,
    };
    use crate::middleware::plugins::inspection::InspectionMiddleware;
    use crate::middleware::plugins::routing::RoutingMiddleware;
    use crate::session::{SessionManager, SharedSessionManager};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    fn engine_with_capture_filter(
        session_manager: SharedSessionManager,
        mode: FilterMode,
        hosts: &[&str],
    ) -> Arc<ProxyEngine> {
        let mut chain = MiddlewareChain::new();
        let capture_filter = Arc::new(RwLock::new(CaptureFilterConfig {
            mode,
            hosts: hosts.iter().map(|s| s.to_string()).collect(),
        }));
        chain.add_middleware(Arc::new(CaptureFilterMiddleware::new(capture_filter)));
        chain.add_middleware(Arc::new(InspectionMiddleware::new(session_manager)));

        Arc::new(ProxyEngine::new(
            Arc::new(RwLock::new(chain)),
            None,
            false,
            30,
            10 * 1024 * 1024,
            10,
            30,
            None,
        ))
    }

    async fn request_unreachable_loopback(engine: Arc<ProxyEngine>, path: &str) -> StatusCode {
        let app = Router::new().fallback(move |req| {
            let engine = engine.clone();
            async move { engine.handle_request(req).await }
        });

        app.oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                // Port 19177 is very unlikely to have anything listening
                .header("host", "127.0.0.1:19177")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    /// Proxy engine with an empty middleware chain forwards to the host in the Host header.
    /// We use a loopback address on a port that is not listening so the connection is
    /// refused immediately (no network dependency, fully deterministic).
    #[tokio::test]
    async fn test_proxy_unreachable_host_returns_bad_gateway() {
        let middleware_chain = Arc::new(RwLock::new(MiddlewareChain::new()));
        let engine = Arc::new(ProxyEngine::new(
            middleware_chain,
            None,
            false,
            30,
            10 * 1024 * 1024,
            10,
            30,
            None,
        ));

        let app = Router::new().fallback(move |req| {
            let engine = engine.clone();
            async move { engine.handle_request(req).await }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/")
                    // Port 19177 is very unlikely to have anything listening
                    .header("host", "127.0.0.1:19177")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    /// When RoutingMiddleware is present and the host has no registered route,
    /// the engine must still attempt the request (pass-through forward proxy behaviour).
    /// We use a port that nothing listens on so it fails fast with BAD_GATEWAY —
    /// but the important assertion is that it is NOT 403 (StopAndReturn is not triggered).
    #[tokio::test]
    async fn test_proxy_unregistered_host_passes_through() {
        let routing_table = Arc::new(RwLock::new(HashMap::new()));
        let mut chain = MiddlewareChain::new();
        chain.add_middleware(Arc::new(RoutingMiddleware::new(routing_table)));
        let middleware_chain = Arc::new(RwLock::new(chain));
        let engine = Arc::new(ProxyEngine::new(
            middleware_chain,
            None,
            false,
            30,
            10 * 1024 * 1024,
            10,
            30,
            None,
        ));

        let app = Router::new().fallback(move |req| {
            let engine = engine.clone();
            async move { engine.handle_request(req).await }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/data")
                    .header("host", "127.0.0.1:19177")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Must NOT be 403 — unregistered hosts are forwarded, not blocked.
        // 502 (connection refused on the loopback) proves the request was attempted.
        assert_ne!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn capture_filter_denylist_skips_recording_without_blocking_proxy_attempt() {
        let sessions = Arc::new(SessionManager::new(10_000));
        let engine = engine_with_capture_filter(
            sessions.clone(),
            FilterMode::Denylist,
            &["127.0.0.1:19177"],
        );

        let status = request_unreachable_loopback(engine, "/filtered-deny").await;

        // BAD_GATEWAY means the request reached the forwarding path and was not blocked
        // by the filter; no listening loopback server is needed for this contract.
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(
            sessions.get_all_sessions().is_empty(),
            "denylisted traffic must not be recorded"
        );
    }

    #[tokio::test]
    async fn capture_filter_allowlist_records_matches_and_skips_non_matches() {
        let matched_sessions = Arc::new(SessionManager::new(10_000));
        let matched_engine = engine_with_capture_filter(
            matched_sessions.clone(),
            FilterMode::Allowlist,
            &["127.0.0.1:19177"],
        );

        let matched_status = request_unreachable_loopback(matched_engine, "/allowed").await;

        assert_eq!(matched_status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            matched_sessions.get_all_sessions().len(),
            1,
            "allowlisted traffic should still be recorded"
        );

        let skipped_sessions = Arc::new(SessionManager::new(10_000));
        let skipped_engine = engine_with_capture_filter(
            skipped_sessions.clone(),
            FilterMode::Allowlist,
            &["does-not-match.local"],
        );

        let skipped_status = request_unreachable_loopback(skipped_engine, "/skipped").await;

        assert_eq!(skipped_status, StatusCode::BAD_GATEWAY);
        assert!(
            skipped_sessions.get_all_sessions().is_empty(),
            "non-allowlisted traffic must not be recorded"
        );
    }
}
