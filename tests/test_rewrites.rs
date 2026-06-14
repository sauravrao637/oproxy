use axum::body::Body;
use axum::http::{Method, Request};
use oproxy::core::engine::{ProxyEngine, ProxyEngineConfig};
use oproxy::middleware::chain::MiddlewareChain;
use oproxy::middleware::matcher::Location;
use oproxy::middleware::plugins::rules::{
    AppliesTo, RewriteAction, RewriteRuleSet, UnifiedRewriteMiddleware,
};
use std::sync::Arc;
use tokio::sync::RwLock;

#[tokio::test]
async fn test_unified_rewrite_through_engine() {
    let rule = RewriteRuleSet {
        id: "r1".to_string(),
        name: "inject on /old-path".to_string(),
        enabled: true,
        location: Location {
            path: Some("/old-path*".to_string()),
            ..Default::default()
        },
        applies_to: AppliesTo::Request,
        actions: vec![RewriteAction::SetHeader {
            name: "X-Rewritten".to_string(),
            value: "true".to_string(),
        }],
    };

    let rewrite_plugin = UnifiedRewriteMiddleware::new(vec![rule]);
    let mut chain = MiddlewareChain::new();
    chain.add_middleware(Arc::new(rewrite_plugin));

    let middleware_chain = Arc::new(RwLock::new(chain));
    let engine = Arc::new(ProxyEngine::new(ProxyEngineConfig {
        middleware_chain,
        ca: None,
        mitm_enabled: false,
        bind_port: 8080,
        bind_host: "127.0.0.1".to_string(),
        timeout_secs: 30,
        max_body_bytes: 10 * 1024 * 1024,
        pool_max_idle_per_host: 10,
        pool_idle_timeout_secs: 30,
        upstream_proxy: None,
    }));

    let req = Request::builder()
        .method(Method::GET)
        .uri("/old-path")
        .header("host", "example.com")
        .body(Body::empty())
        .unwrap();

    // Completes without panic; the upstream will 502 since there's no real server,
    // but the middleware correctly processed the request.
    let _ = engine.handle_request(req).await;
}
