#[cfg(test)]
mod tests {
    use crate::core::engine::{ProxyEngine, ProxyEngineConfig};
    use crate::middleware::chain::MiddlewareChain;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn engine_created_without_mitm_has_correct_flag() {
        let engine = ProxyEngine::new(ProxyEngineConfig {
            middleware_chain: Arc::new(RwLock::new(MiddlewareChain::new())),
            mitm_enabled: false,
            bind_host: "127.0.0.1".to_string(),
            ..Default::default()
        });
        assert!(!engine.mitm_enabled);
    }
}
