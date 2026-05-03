#[cfg(test)]
mod tests {
    use crate::core::engine::ProxyEngine;
    use crate::middleware::chain::MiddlewareChain;
    use crate::middleware::plugins::routing::RoutingMiddleware;
    use axum::{routing::get, Router};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;
    use axum::http::{Request, StatusCode};
    use axum::body::Body;

    /// Proxy engine with an empty middleware chain forwards to the host in the Host header.
    /// We use a loopback address on a port that is not listening so the connection is
    /// refused immediately (no network dependency, fully deterministic).
    #[tokio::test]
    async fn test_proxy_unreachable_host_returns_bad_gateway() {
        let middleware_chain = Arc::new(RwLock::new(MiddlewareChain::new()));
        let engine = Arc::new(ProxyEngine::new(middleware_chain, None, false, 30, 10*1024*1024, 10, 30, None));

        let app = Router::new()
            .fallback(move |req| {
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
        let engine = Arc::new(ProxyEngine::new(middleware_chain, None, false, 30, 10*1024*1024, 10, 30, None));

        let app = Router::new()
            .fallback(move |req| {
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
}
