use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ThrottlingMiddleware {
    pub config: Arc<RwLock<ThrottlingConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
            if let Some(transfer_ms) = (body_bytes * 1000).checked_div(bytes_per_sec)
                && transfer_ms > 0
            {
                tokio::time::sleep(std::time::Duration::from_millis(transfer_ms)).await;
            }
        }
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
    use bytes::Bytes;
    use std::collections::HashMap;

    fn req() -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: "/".to_string(),
            headers: HashMap::new(),
            body: Bytes::new(),
            host: "x".to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn disabled_throttling_does_not_delay() {
        let mw = ThrottlingMiddleware {
            config: Arc::new(RwLock::new(ThrottlingConfig {
                latency_ms: 5000,
                bandwidth_limit_kbps: 0,
                enabled: false,
            })),
        };
        let start = std::time::Instant::now();
        mw.on_request(&mut req()).await;
        assert!(
            start.elapsed().as_millis() < 200,
            "disabled throttling must not delay"
        );
    }

    #[tokio::test]
    async fn enabled_throttling_applies_latency() {
        let mw = ThrottlingMiddleware {
            config: Arc::new(RwLock::new(ThrottlingConfig {
                latency_ms: 50,
                bandwidth_limit_kbps: 0,
                enabled: true,
            })),
        };
        let start = std::time::Instant::now();
        let action = mw.on_request(&mut req()).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert!(
            start.elapsed().as_millis() >= 50,
            "enabled throttling must delay >= latency_ms"
        );
    }

    #[tokio::test]
    async fn zero_latency_with_enabled_flag_does_not_delay() {
        let mw = ThrottlingMiddleware {
            config: Arc::new(RwLock::new(ThrottlingConfig {
                latency_ms: 0,
                bandwidth_limit_kbps: 0,
                enabled: true,
            })),
        };
        let start = std::time::Instant::now();
        mw.on_request(&mut req()).await;
        assert!(start.elapsed().as_millis() < 200);
    }

    #[tokio::test]
    async fn throttling_on_response_always_continues_when_disabled() {
        let mw = ThrottlingMiddleware {
            config: Arc::new(RwLock::new(ThrottlingConfig {
                latency_ms: 0,
                bandwidth_limit_kbps: 0,
                enabled: false,
            })),
        };
        let mut ctx = ResponseContext {
            status: 200,
            headers: HashMap::new(),
            body: Bytes::new(),
            request_uri: "/".to_string(),
            ..Default::default()
        };
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
        // 1 KB body: expected transfer_ms = 1024 * 1000 / (8 * 1024 / 8) = 1000ms
        let mut ctx = ResponseContext {
            status: 200,
            headers: HashMap::new(),
            body: Bytes::from("a".repeat(1024)),
            request_uri: "/".to_string(),
            ..Default::default()
        };
        let start = std::time::Instant::now();
        mw.on_response(&mut ctx).await;
        let elapsed = start.elapsed().as_millis();
        assert!(
            elapsed >= 900,
            "bandwidth limit should delay ~1s for 1KB at 8kbps, got {}ms",
            elapsed
        );
    }
}
