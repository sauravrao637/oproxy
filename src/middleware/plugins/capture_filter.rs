use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};

pub const SKIP_RECORDING_HEADER: &str = "x-oproxy-skip-recording";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    #[default]
    Disabled,   // Record all traffic
    Allowlist,  // Only record hosts in the list
    Denylist,   // Record everything except listed hosts
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaptureFilterConfig {
    pub mode: FilterMode,
    /// Substring-matched against request host. Case-insensitive.
    pub hosts: Vec<String>,
}

impl CaptureFilterConfig {
    pub fn should_skip(&self, host: &str) -> bool {
        let host_lc = host.to_lowercase();
        match self.mode {
            FilterMode::Disabled => false,
            FilterMode::Allowlist => !self.hosts.iter().any(|h| host_lc.contains(h.to_lowercase().as_str())),
            FilterMode::Denylist  =>  self.hosts.iter().any(|h| host_lc.contains(h.to_lowercase().as_str())),
        }
    }
}

pub struct CaptureFilterMiddleware {
    pub config: Arc<RwLock<CaptureFilterConfig>>,
}

impl CaptureFilterMiddleware {
    pub fn new(config: Arc<RwLock<CaptureFilterConfig>>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Middleware for CaptureFilterMiddleware {
    fn name(&self) -> &str {
        "CaptureFilterMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let cfg = self.config.read().await;
        if cfg.should_skip(&ctx.host) {
            ctx.headers.insert(SKIP_RECORDING_HEADER.to_string(), "true".to_string());
        }
        // Always continue — we never block proxying, only toggle recording.
        MiddlewareAction::Continue
    }

    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{Middleware, RequestContext};
    use std::collections::HashMap;

    fn cfg(mode: FilterMode, hosts: &[&str]) -> Arc<RwLock<CaptureFilterConfig>> {
        Arc::new(RwLock::new(CaptureFilterConfig {
            mode,
            hosts: hosts.iter().map(|s| s.to_string()).collect(),
        }))
    }

    fn req_ctx(host: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: format!("http://{}/", host),
            headers: HashMap::new(),
            body: String::new(),
            host: host.to_string(),
            body_bytes: None,
        }
    }

    #[tokio::test]
    async fn disabled_mode_does_not_inject_skip_header() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Disabled, &[]));
        let mut ctx = req_ctx("api.example.com");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key(SKIP_RECORDING_HEADER));
    }

    #[tokio::test]
    async fn allowlist_matching_host_not_skipped() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Allowlist, &["example.com"]));
        let mut ctx = req_ctx("api.example.com");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key(SKIP_RECORDING_HEADER));
    }

    #[tokio::test]
    async fn allowlist_non_matching_host_is_skipped() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Allowlist, &["example.com"]));
        let mut ctx = req_ctx("cdn.other.net");
        mw.on_request(&mut ctx).await;
        assert_eq!(
            ctx.headers.get(SKIP_RECORDING_HEADER).map(|s| s.as_str()),
            Some("true")
        );
    }

    #[tokio::test]
    async fn allowlist_empty_list_skips_everything() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Allowlist, &[]));
        let mut ctx = req_ctx("anything.com");
        mw.on_request(&mut ctx).await;
        assert!(ctx.headers.contains_key(SKIP_RECORDING_HEADER));
    }

    #[tokio::test]
    async fn denylist_listed_host_is_skipped() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Denylist, &["analytics."]));
        let mut ctx = req_ctx("analytics.google.com");
        mw.on_request(&mut ctx).await;
        assert!(ctx.headers.contains_key(SKIP_RECORDING_HEADER));
    }

    #[tokio::test]
    async fn denylist_non_listed_host_not_skipped() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Denylist, &["analytics."]));
        let mut ctx = req_ctx("api.example.com");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key(SKIP_RECORDING_HEADER));
    }

    #[tokio::test]
    async fn filter_is_case_insensitive() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Denylist, &["ANALYTICS"]));
        let mut ctx = req_ctx("Analytics.Google.Com");
        mw.on_request(&mut ctx).await;
        assert!(ctx.headers.contains_key(SKIP_RECORDING_HEADER));
    }

    #[tokio::test]
    async fn on_response_always_returns_continue() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Denylist, &["x.com"]));
        let mut ctx = ResponseContext::default();
        assert_eq!(mw.on_response(&mut ctx).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn on_request_always_returns_continue_even_when_skipping() {
        let mw = CaptureFilterMiddleware::new(cfg(FilterMode::Allowlist, &["example.com"]));
        let mut ctx = req_ctx("other.net");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
    }

    #[test]
    fn should_skip_disabled_always_false() {
        let cfg = CaptureFilterConfig { mode: FilterMode::Disabled, hosts: vec!["anything".to_string()] };
        assert!(!cfg.should_skip("anything.com"));
    }

    #[test]
    fn should_skip_allowlist_match() {
        let cfg = CaptureFilterConfig { mode: FilterMode::Allowlist, hosts: vec!["api.".to_string()] };
        assert!(!cfg.should_skip("api.example.com")); // match → don't skip
        assert!(cfg.should_skip("cdn.example.com"));  // no match → skip
    }

    #[test]
    fn should_skip_denylist_match() {
        let cfg = CaptureFilterConfig { mode: FilterMode::Denylist, hosts: vec!["cdn.".to_string()] };
        assert!(cfg.should_skip("cdn.example.com"));  // match → skip
        assert!(!cfg.should_skip("api.example.com")); // no match → don't skip
    }
}
