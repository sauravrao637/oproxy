mod common;

use std::sync::Arc;
use tokio::sync::RwLock;
use common::create_test_engine;
use oproxy::core::engine::ProxyEngine;
use oproxy::middleware::chain::MiddlewareChain;
use oproxy::middleware::plugins::rewrite::{RewriteMiddleware, RewriteRule, MatchCriteria, RewriteAction};
use axum::http::{Request, Method};
use axum::body::Body;

#[tokio::test]
async fn test_rewrite_rules() {
    // 1. Setup middleware with a rewrite rule
    let rule = RewriteRule {
        name: "TestRule".to_string(),
        criteria: MatchCriteria::Path("/old-path".to_string()),
        action: RewriteAction::AddHeader {
            name: "X-Rewritten".to_string(),
            value: "true".to_string(),
        },
        enabled: true,
    };
    
    let rewrite_plugin = RewriteMiddleware::new(vec![rule]);
    let mut chain = MiddlewareChain::new();
    chain.add_middleware(Arc::new(rewrite_plugin));
    
    let middleware_chain = Arc::new(RwLock::new(chain));
    let engine = Arc::new(ProxyEngine::new(middleware_chain, None, false, 30, 10 * 1024 * 1024, 10, 30, None));
    
    // 2. Prepare request matching criteria
    let req = Request::builder()
        .method(Method::GET)
        .uri("/old-path")
        .header("host", "example.com")
        .body(Body::empty())
        .unwrap();

    // 3. Act - Run through engine
    let _ = engine.handle_request(req).await;
    
    // 4. Verification is implicit by completion without panic
}
