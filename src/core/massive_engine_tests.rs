#[cfg(test)]
mod tests {
    use crate::core::engine::ProxyEngine;
    use crate::middleware::chain::MiddlewareChain;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn engine_created_without_mitm_has_correct_flag() {
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
    }
}
