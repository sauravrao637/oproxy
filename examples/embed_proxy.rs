//! Embed the proxy engine in your own program.
//!
//! [`ProxyEngine`] owns the upstream HTTP clients and the middleware chain. You
//! construct it from a [`ProxyEngineConfig`] (named fields, so the call site is
//! self-documenting) and then drive it from a transport listener. This example
//! just builds an engine with an empty chain to show the construction API.
//!
//! Run with:
//!
//! ```text
//! cargo run --example embed_proxy
//! ```

use std::sync::Arc;

use oproxy::core::engine::{ProxyEngine, ProxyEngineConfig};
use oproxy::middleware::chain::MiddlewareChain;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() {
    // The chain is shared behind an `RwLock` so it can be hot-reloaded while the
    // proxy is running. Add your own middleware here (see `custom_middleware`).
    let chain = Arc::new(RwLock::new(MiddlewareChain::new()));

    let engine = ProxyEngine::new(ProxyEngineConfig {
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
    });

    println!(
        "proxy engine constructed; max body buffer = {} bytes",
        engine.max_body_bytes()
    );
}
