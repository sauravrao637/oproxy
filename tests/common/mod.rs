use std::sync::Arc;
use tokio::sync::RwLock;
use oproxy::middleware::chain::MiddlewareChain;
use oproxy::core::engine::ProxyEngine;

pub async fn create_test_engine() -> ProxyEngine {
    let chain = Arc::new(RwLock::new(MiddlewareChain::new()));
    ProxyEngine::new(chain, None, false, 30, 10 * 1024 * 1024, 10, 30)
}
