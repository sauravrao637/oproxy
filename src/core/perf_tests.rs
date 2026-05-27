#[cfg(test)]
mod tests {
    use crate::core::engine::ProxyEngine;
    use crate::middleware::chain::MiddlewareChain;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn test_proxy_performance_overhead() {
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

        let start = Instant::now();
        let iterations = 1000;

        // This is a rough check to ensure the middleware chain isn't pathologically slow
        for _ in 0..iterations {
            let middleware_chain_read = engine.middleware_chain.read().await;
            // Simple operation
            let _ = middleware_chain_read
                .execute_request(&mut crate::middleware::RequestContext {
                    method: "GET".to_string(),
                    uri: "/".to_string(),
                    headers: std::collections::HashMap::new(),
                    body: "".to_string(),
                    host: "example.com".to_string(),
                    body_bytes: None,
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
