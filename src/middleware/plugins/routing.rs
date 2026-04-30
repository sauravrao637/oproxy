use async_trait::async_trait;
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

pub struct RoutingMiddleware {
    pub routing_table: Arc<RwLock<HashMap<String, String>>>,
    /// host → absolute path on disk: serve the file instead of forwarding.
    pub map_local: Arc<RwLock<HashMap<String, String>>>,
}

impl RoutingMiddleware {
    pub fn new(routing_table: Arc<RwLock<HashMap<String, String>>>) -> Self {
        Self {
            routing_table,
            map_local: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl Middleware for RoutingMiddleware {
    fn name(&self) -> &str {
        "RoutingMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        // Map-local: serve a file from disk and short-circuit forwarding.
        let map_local = self.map_local.read().await;
        if let Some(file_path) = map_local.get(&ctx.host) {
            match tokio::fs::read_to_string(file_path).await {
                Ok(contents) => {
                    // Stash the file contents in the body and mark as a local response via header.
                    ctx.body = contents;
                    ctx.headers.insert("x-oproxy-map-local-file".to_string(), file_path.clone());
                    return MiddlewareAction::StopAndReturn;
                }
                Err(e) => {
                    tracing::warn!(path=%file_path, error=%e, "map_local: could not read file");
                }
            }
        }
        drop(map_local);

        let table = self.routing_table.read().await;
        if let Some(destination) = table.get(&ctx.host) {
            ctx.headers.insert("x-oproxy-destination".to_string(), destination.clone());
        }
        // No entry → forward to original host; engine.rs falls back to http://<host><path>
        MiddlewareAction::Continue
    }

    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }
}

pub struct ThrottlingMiddleware {
    pub config: Arc<RwLock<ThrottlingConfig>>,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThrottlingConfig {
    pub latency_ms: u64,
    pub bandwidth_limit_kbps: u64,
    pub enabled: bool,
}

#[async_trait]
impl Middleware for ThrottlingMiddleware {
    fn name(&self) -> &str {
        "ThrottlingMiddleware"
    }

    async fn on_request(&self, _ctx: &mut RequestContext) -> MiddlewareAction {
        let config = self.config.read().await;
        if config.enabled && config.latency_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(config.latency_ms)).await;
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        let config = self.config.read().await;
        if config.enabled && config.bandwidth_limit_kbps > 0 && !ctx.body.is_empty() {
            // Simulate bandwidth limiting: compute how long this body would take to transfer
            // at the configured rate, then sleep for that duration.
            // bytes / (kbps * 1024 / 8) = bytes * 8 / (kbps * 1024) seconds
            let body_bytes = ctx.body.len() as u64;
            let bytes_per_sec = config.bandwidth_limit_kbps * 1024 / 8;
            if bytes_per_sec > 0 {
                let transfer_ms = body_bytes * 1000 / bytes_per_sec;
                if transfer_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(transfer_ms)).await;
                }
            }
        }
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
    use std::collections::HashMap;

    fn req(host: &str) -> RequestContext {
        RequestContext { method: "GET".to_string(), uri: "/".to_string(), headers: HashMap::new(), body: "".to_string(), host: host.to_string(), body_bytes: None }
    }

    fn routing_with(entries: Vec<(&str, &str)>) -> RoutingMiddleware {
        let mut map = HashMap::new();
        for (k, v) in entries { map.insert(k.to_string(), v.to_string()); }
        RoutingMiddleware {
            routing_table: Arc::new(RwLock::new(map)),
            map_local: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // --- RoutingMiddleware ---

    #[tokio::test]
    async fn known_host_sets_destination_header_and_continues() {
        let mw = routing_with(vec![("api.local", "http://10.0.0.1:3000")]);
        let mut ctx = req("api.local");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
        assert_eq!(ctx.headers.get("x-oproxy-destination").map(|s| s.as_str()), Some("http://10.0.0.1:3000"));
    }

    #[tokio::test]
    async fn unknown_host_passes_through_without_destination_header() {
        let mw = routing_with(vec![]);
        let mut ctx = req("unknown.host");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
        assert!(!ctx.headers.contains_key("x-oproxy-destination"), "no destination header for unmapped host");
    }

    #[tokio::test]
    async fn localhost_8080_not_in_table_still_continues() {
        let mw = routing_with(vec![]);
        let mut ctx = req("localhost:8080");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn loopback_ip_8080_not_in_table_still_continues() {
        let mw = routing_with(vec![]);
        let mut ctx = req("127.0.0.1:8080");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn routing_on_response_always_continues() {
        let mw = routing_with(vec![]);
        let mut ctx = ResponseContext { status: 200, headers: HashMap::new(), body: "".to_string(), request_uri: "/".to_string(), session_id: None, ttfb_ms: 0, body_ms: 0, body_bytes: None };
        assert_eq!(mw.on_response(&mut ctx).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn map_local_returns_stop_and_return_for_existing_file() {
        let tmp = std::env::temp_dir().join("oproxy_map_local_test.txt");
        tokio::fs::write(&tmp, "hello map local").await.unwrap();

        let mw = RoutingMiddleware {
            routing_table: Arc::new(RwLock::new(HashMap::new())),
            map_local: Arc::new(RwLock::new({
                let mut m = HashMap::new();
                m.insert("local.mock".to_string(), tmp.to_string_lossy().to_string());
                m
            })),
        };
        let mut ctx = req("local.mock");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        assert_eq!(ctx.body, "hello map local");

        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[tokio::test]
    async fn map_local_falls_through_when_file_missing() {
        let mw = RoutingMiddleware {
            routing_table: Arc::new(RwLock::new(HashMap::new())),
            map_local: Arc::new(RwLock::new({
                let mut m = HashMap::new();
                m.insert("local.mock".to_string(), "/nonexistent/file.txt".to_string());
                m
            })),
        };
        let mut ctx = req("local.mock");
        // Should fall through to Continue (file read failed → normal forwarding)
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
    }

    // --- ThrottlingMiddleware ---

    #[tokio::test]
    async fn disabled_throttling_does_not_delay() {
        let mw = ThrottlingMiddleware { config: Arc::new(RwLock::new(ThrottlingConfig { latency_ms: 5000, bandwidth_limit_kbps: 0, enabled: false })) };
        let start = std::time::Instant::now();
        mw.on_request(&mut req("x")).await;
        assert!(start.elapsed().as_millis() < 200, "disabled throttling must not delay");
    }

    #[tokio::test]
    async fn enabled_throttling_applies_latency() {
        let mw = ThrottlingMiddleware { config: Arc::new(RwLock::new(ThrottlingConfig { latency_ms: 50, bandwidth_limit_kbps: 0, enabled: true })) };
        let start = std::time::Instant::now();
        let action = mw.on_request(&mut req("x")).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert!(start.elapsed().as_millis() >= 50, "enabled throttling must delay >= latency_ms");
    }

    #[tokio::test]
    async fn zero_latency_with_enabled_flag_does_not_delay() {
        let mw = ThrottlingMiddleware { config: Arc::new(RwLock::new(ThrottlingConfig { latency_ms: 0, bandwidth_limit_kbps: 0, enabled: true })) };
        let start = std::time::Instant::now();
        mw.on_request(&mut req("x")).await;
        assert!(start.elapsed().as_millis() < 200);
    }

    #[tokio::test]
    async fn throttling_on_response_always_continues_when_disabled() {
        let mw = ThrottlingMiddleware { config: Arc::new(RwLock::new(ThrottlingConfig { latency_ms: 0, bandwidth_limit_kbps: 0, enabled: false })) };
        let mut ctx = ResponseContext { status: 200, headers: HashMap::new(), body: "".to_string(), request_uri: "/".to_string(), session_id: None, ttfb_ms: 0, body_ms: 0, body_bytes: None };
        assert_eq!(mw.on_response(&mut ctx).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn bandwidth_limit_delays_proportional_to_body_size() {
        // 8 kbps = 1 KB/s; 1 KB body → ~1000 ms delay
        let mw = ThrottlingMiddleware {
            config: Arc::new(RwLock::new(ThrottlingConfig {
                latency_ms: 0,
                bandwidth_limit_kbps: 8,
                enabled: true,
            })),
        };
        // 1 KB body: expected transfer_ms = 1024 * 1000 / (8 * 1024 / 8) = 1024*1000/1024 = 1000ms
        let mut ctx = ResponseContext {
            status: 200,
            headers: HashMap::new(),
            body: "a".repeat(1024),
            request_uri: "/".to_string(),
            session_id: None,
            ttfb_ms: 0,
            body_ms: 0,
            body_bytes: None,
        };
        let start = std::time::Instant::now();
        mw.on_response(&mut ctx).await;
        let elapsed = start.elapsed().as_millis();
        assert!(elapsed >= 900, "bandwidth limit should delay ~1s for 1KB at 8kbps, got {}ms", elapsed);
    }
}
