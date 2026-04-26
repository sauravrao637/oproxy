use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use async_trait::async_trait;
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};

pub struct DnsOverrideMiddleware {
    pub overrides: Arc<RwLock<HashMap<String, String>>>,
}

#[async_trait]
impl Middleware for DnsOverrideMiddleware {
    fn name(&self) -> &str { "dns_override" }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let ovr = self.overrides.read().await;
        if ovr.is_empty() {
            return MiddlewareAction::Continue;
        }
        // Strip port from host to get the bare hostname for lookup.
        let (hostname, port) = if let Some(colon) = ctx.host.rfind(':') {
            (&ctx.host[..colon], &ctx.host[colon + 1..])
        } else {
            (ctx.host.as_str(), "")
        };
        if let Some(ip) = ovr.get(hostname) {
            let new_host = if port.is_empty() {
                ip.clone()
            } else {
                format!("{}:{}", ip, port)
            };
            let scheme_port = if port == "443" { "https" } else { "http" };
            let dest = format!("{}://{}", scheme_port, new_host);
            ctx.host = new_host;
            ctx.headers.insert("x-proxy-destination".to_string(), dest);
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }
}
