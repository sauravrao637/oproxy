#[cfg(test)]
mod tests {
    use crate::core::engine::{ProxyEngine, ProxyEngineConfig};
    use crate::middleware::chain::MiddlewareChain;
    use crate::middleware::plugins::capture_filter::{
        CaptureFilterConfig, CaptureFilterMiddleware, FilterMode,
    };
    use crate::middleware::plugins::inspection::InspectionMiddleware;
    use crate::middleware::plugins::map_remote::MapRemoteMiddleware;
    use crate::session::{SessionManager, SharedSessionManager};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
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

        Arc::new(ProxyEngine::new(ProxyEngineConfig {
            middleware_chain: Arc::new(RwLock::new(chain)),
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        }))
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
        let engine = Arc::new(ProxyEngine::new(ProxyEngineConfig {
            middleware_chain,
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        }));

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

    /// When MapRemoteMiddleware is present with no matching rules,
    /// the engine must still attempt the request (pass-through forward proxy behaviour).
    /// We use a port that nothing listens on so it fails fast with BAD_GATEWAY —
    /// but the important assertion is that it is NOT 403 (StopAndReturn is not triggered).
    #[tokio::test]
    async fn test_proxy_unregistered_host_passes_through() {
        let mut chain = MiddlewareChain::new();
        chain.add_middleware(Arc::new(MapRemoteMiddleware::new(vec![])));
        let middleware_chain = Arc::new(RwLock::new(chain));
        let engine = Arc::new(ProxyEngine::new(ProxyEngineConfig {
            middleware_chain,
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        }));

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

    /// HTTP/2 (and h3) forward-proxy requests carry the target authority in the
    /// URI (`:authority`), with no `Host` header. The engine must resolve the host
    /// from the URI authority and forward successfully instead of 502-ing on an
    /// "unknown" host.
    #[tokio::test]
    async fn forwards_when_authority_is_in_uri_without_host_header() {
        use axum::body::Bytes;
        use axum::routing::get;

        let upstream = Router::new().route("/echo", get(|| async { "auth-ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let engine = Arc::new(ProxyEngine::new(ProxyEngineConfig {
            middleware_chain: Arc::new(RwLock::new(MiddlewareChain::new())),
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        }));
        let app = Router::new().fallback(move |req| {
            let engine = engine.clone();
            async move { engine.handle_request(req).await }
        });

        // Absolute-form URI, NO Host header — the h2/h3 forward-proxy shape.
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("http://127.0.0.1:{}/echo", addr.port()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, Bytes::from_static(b"auth-ok"));
    }

    #[tokio::test]
    async fn requests_on_same_connection_share_id_with_distinct_stream_ids() {
        use crate::transport::http::DownstreamConn;

        let sessions: SharedSessionManager = Arc::new(SessionManager::new(10_000));
        let mut chain = MiddlewareChain::new();
        chain.add_middleware(Arc::new(InspectionMiddleware::new(sessions.clone())));
        let engine = Arc::new(ProxyEngine::new(ProxyEngineConfig {
            middleware_chain: Arc::new(RwLock::new(chain)),
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        }));

        let app = Router::new().fallback(move |req| {
            let engine = engine.clone();
            async move { engine.handle_request(req).await }
        });

        // Two requests carrying the SAME DownstreamConn extension = one connection.
        let conn = DownstreamConn::new();
        for path in ["/one", "/two"] {
            let _ = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .header("host", "127.0.0.1:19177")
                        .extension(conn.clone())
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
        }
        sessions.flush().await;

        let recorded = sessions.get_all_sessions();
        assert_eq!(recorded.len(), 2);
        let conn_ids: std::collections::HashSet<_> = recorded
            .iter()
            .filter_map(|e| e.connection_id.clone())
            .collect();
        assert_eq!(conn_ids.len(), 1, "both exchanges share one connection_id");
        let mut stream_ids: Vec<u64> = recorded.iter().filter_map(|e| e.stream_id).collect();
        stream_ids.sort_unstable();
        assert_eq!(
            stream_ids,
            vec![0, 1],
            "stream ids are distinct and monotonic"
        );
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
        sessions.flush().await;

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
        matched_sessions.flush().await;

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
        skipped_sessions.flush().await;

        assert_eq!(skipped_status, StatusCode::BAD_GATEWAY);
        assert!(
            skipped_sessions.get_all_sessions().is_empty(),
            "non-allowlisted traffic must not be recorded"
        );
    }

    /// A plugin that opts the exchange into the streaming (inspect-only) class.
    struct StreamingInspectPlugin;

    #[async_trait::async_trait]
    impl crate::middleware::Middleware for StreamingInspectPlugin {
        fn name(&self) -> &str {
            "streaming-inspect-test"
        }
        fn body_hint(
            &self,
            _head: &crate::middleware::RequestContext,
        ) -> crate::core::forward::BodyHint {
            crate::core::forward::BodyHint::StreamingInspect {
                granularity: crate::core::forward::Granularity::Bytes,
            }
        }
    }

    /// End-to-end proof that the streaming class relays a request body upstream
    /// without buffering and streams the response back intact. An upstream echo
    /// server returns the exact bytes it received; a large body (above the 512 KiB
    /// stream threshold) round-trips byte-for-byte through `forward_stream`.
    #[tokio::test]
    async fn streaming_class_round_trips_large_body_through_forward_stream() {
        use axum::body::Bytes;
        use axum::routing::post;

        // Upstream echo server.
        let upstream = Router::new().route("/echo", post(|body: Bytes| async move { body }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        // Engine whose only plugin opts into streaming.
        let mut chain = MiddlewareChain::new();
        chain.add_middleware(Arc::new(StreamingInspectPlugin));
        let engine = Arc::new(ProxyEngine::new(ProxyEngineConfig {
            middleware_chain: Arc::new(RwLock::new(chain)),
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        }));

        let app = Router::new().fallback(move |req| {
            let engine = engine.clone();
            async move { engine.handle_request(req).await }
        });

        let payload = vec![b'z'; 1024 * 1024]; // 1 MiB, above the stream threshold
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("http://127.0.0.1:{}/echo", addr.port()))
                    .header("host", format!("127.0.0.1:{}", addr.port()))
                    .body(Body::from(payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let echoed = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            echoed.len(),
            payload.len(),
            "streamed body must round-trip without truncation"
        );
        assert_eq!(
            &echoed[..],
            &payload[..],
            "streamed body must be byte-identical"
        );
    }
}
