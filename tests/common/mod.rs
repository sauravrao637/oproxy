use oproxy::core::engine::ProxyEngine;
use oproxy::middleware::chain::MiddlewareChain;
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn create_test_engine() -> ProxyEngine {
    let chain = Arc::new(RwLock::new(MiddlewareChain::new()));
    ProxyEngine::new(chain, None, false, 30, 10 * 1024 * 1024, 10, 30, None)
}
