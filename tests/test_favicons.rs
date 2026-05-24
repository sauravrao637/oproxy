mod common;

use crate::common::create_test_engine;
use axum::body::Body;
use axum::http::{Method, Request};
use oproxy::core::engine::ProxyEngine;
use std::sync::Arc;

#[tokio::test]
async fn test_favicon_retrieval() {
    let engine: Arc<ProxyEngine> = Arc::new(create_test_engine().await);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/favicon.ico")
        .header("host", "example.com")
        .body(Body::empty())
        .unwrap();

    let response = engine.handle_request(req).await;

    assert!(response.status().is_client_error() || response.status().is_server_error());
}
