use async_trait::async_trait;
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Serialize, Deserialize)]
pub struct ModificationRule {
    pub request_uri_pattern: String,
    #[serde(default)]
    pub header_replacements: HashMap<String, String>,
    #[serde(default)]
    pub body_replacement: Option<String>,
}

pub struct ModificationMiddleware {
    pub rules: Arc<RwLock<Vec<ModificationRule>>>,
}

impl ModificationMiddleware {
    pub fn new(rules: Vec<ModificationRule>) -> Self {
        Self { rules: Arc::new(RwLock::new(rules)) }
    }
}

#[async_trait]
impl Middleware for ModificationMiddleware {
    fn name(&self) -> &str {
        "ModificationMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        for rule in rules.iter() {
            if ctx.uri.contains(&rule.request_uri_pattern) {
                for (key, value) in &rule.header_replacements {
                    ctx.headers.insert(key.clone(), value.clone());
                }
                if let Some(ref body) = rule.body_replacement {
                    ctx.body = body.clone();
                    ctx.body_bytes = None;
                }
            }
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        // Response modification can be implemented similarly
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
    use std::collections::HashMap;

    fn req(uri: &str) -> RequestContext {
        RequestContext { method: "GET".to_string(), uri: uri.to_string(), headers: HashMap::new(), body: "original".to_string(), host: "localhost".to_string(), body_bytes: None }
    }

    fn rule(pattern: &str, hdrs: Vec<(&str, &str)>, body: Option<&str>) -> ModificationRule {
        ModificationRule {
            request_uri_pattern: pattern.to_string(),
            header_replacements: hdrs.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body_replacement: body.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn matching_rule_inserts_headers() {
        let mw = ModificationMiddleware::new(vec![rule("/api", vec![("x-modified", "yes")], None)]);
        let mut ctx = req("/api/resource");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-modified").map(|s| s.as_str()), Some("yes"));
        assert_eq!(ctx.body, "original");
    }

    #[tokio::test]
    async fn matching_rule_replaces_body() {
        let mw = ModificationMiddleware::new(vec![rule("/api", vec![], Some("replaced"))]);
        let mut ctx = req("/api/resource");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.body, "replaced");
    }

    #[tokio::test]
    async fn non_matching_uri_leaves_context_unchanged() {
        let mw = ModificationMiddleware::new(vec![rule("/admin", vec![("x-admin", "1")], Some("admin-body"))]);
        let mut ctx = req("/api/resource");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-admin"));
        assert_eq!(ctx.body, "original");
    }

    #[tokio::test]
    async fn empty_rule_list_is_noop() {
        let mw = ModificationMiddleware::new(vec![]);
        let mut ctx = req("/any");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert_eq!(ctx.body, "original");
    }

    #[tokio::test]
    async fn multiple_matching_rules_all_applied_in_order() {
        let mw = ModificationMiddleware::new(vec![
            rule("/path", vec![("x-first", "1")], None),
            rule("/path", vec![("x-second", "2")], Some("final")),
        ]);
        let mut ctx = req("/path");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-first").map(|s| s.as_str()), Some("1"));
        assert_eq!(ctx.headers.get("x-second").map(|s| s.as_str()), Some("2"));
        assert_eq!(ctx.body, "final");
    }

    #[tokio::test]
    async fn on_response_always_continues_unchanged() {
        let mw = ModificationMiddleware::new(vec![rule("/any", vec![("x-h", "v")], Some("body"))]);
        let mut ctx = ResponseContext { status: 200, headers: HashMap::new(), body: "resp".to_string(), request_uri: "/any".to_string(), session_id: None, ttfb_ms: 0, body_ms: 0, body_bytes: None };
        let action = mw.on_response(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert_eq!(ctx.body, "resp", "response body must not be touched by ModificationMiddleware");
    }
}
