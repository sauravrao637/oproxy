use oproxy::core::engine::{ProxyEngine, ProxyEngineConfig};
use oproxy::middleware::chain::MiddlewareChain;
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn create_test_engine() -> ProxyEngine {
    let chain = Arc::new(RwLock::new(MiddlewareChain::new()));
    ProxyEngine::new(ProxyEngineConfig {
        middleware_chain: chain,
        ca: None,
        mitm_enabled: false,
        bind_port: 8080,
        bind_host: "127.0.0.1".to_string(),
        timeout_secs: 30,
        max_body_bytes: 10 * 1024 * 1024,
        pool_max_idle_per_host: 10,
        pool_idle_timeout_secs: 30,
        upstream_proxy: None,
    })
}
