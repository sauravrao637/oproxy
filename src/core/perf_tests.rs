#[cfg(test)]
mod tests {
    use crate::core::engine::{ProxyEngine, ProxyEngineConfig};
    use crate::middleware::chain::MiddlewareChain;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn test_proxy_performance_overhead() {
        let middleware_chain = Arc::new(RwLock::new(MiddlewareChain::new()));
        let engine = Arc::new(ProxyEngine::new(ProxyEngineConfig {
            middleware_chain,
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        }));

        let start = Instant::now();
        let iterations = 1000;

        // The generous threshold catches severe regressions without making the
        // test sensitive to host load.
        for _ in 0..iterations {
            let middleware_chain_read = engine.middleware_chain.read().await;
            let _ = middleware_chain_read
                .execute_request(&mut crate::middleware::RequestContext {
                    method: "GET".to_string(),
                    uri: "/".to_string(),
                    headers: crate::middleware::HeaderMap::new(),
                    body: bytes::Bytes::new(),
                    host: "example.com".to_string(),
                    ..Default::default()
                })
                .await;
        }

        let duration = start.elapsed();
        println!(
            "Performed {} middleware checks in {:?}",
            iterations, duration
        );
        assert!(duration.as_millis() < 5000); // Should be very fast
    }
}
