use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{InterceptedResponse, Middleware, MiddlewareAction, RequestContext};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AccessAction {
    Block,
    Allow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub location: Location,
    pub action: AccessAction,
}

fn default_true() -> bool {
    true
}

impl AccessRule {
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }
}

pub type SharedAccessRules = Arc<RwLock<Vec<AccessRule>>>;

pub struct AccessControlMiddleware {
    pub rules: SharedAccessRules,
}

impl AccessControlMiddleware {
    pub fn new(rules: Vec<AccessRule>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
        }
    }

    fn block_response(status: u16) -> InterceptedResponse {
        InterceptedResponse {
            status,
            headers: crate::middleware::HeaderMap::from_iter([(
                "Content-Type".to_string(),
                "text/plain".to_string(),
            )]),
            body: Bytes::from("Blocked by access control rule"),
            tags: vec!["access-blocked".to_string()],
        }
    }
}

#[async_trait]
impl Middleware for AccessControlMiddleware {
    fn name(&self) -> &str {
        "AccessControlMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        let enabled: Vec<&AccessRule> = rules.iter().filter(|r| r.enabled).collect();
        if enabled.is_empty() {
            return MiddlewareAction::Continue;
        }

        let target = MatchTarget::from_request(ctx);

        // Block rules: any match → 403 immediately.
        for rule in enabled.iter().filter(|r| r.action == AccessAction::Block) {
            if rule.location.matches(&target) {
                ctx.mock_response = Some(Self::block_response(403));
                return MiddlewareAction::StopAndReturn;
            }
        }

        // Allow rules: if any exist, the request must match at least one.
        let allow_rules: Vec<&&AccessRule> = enabled
            .iter()
            .filter(|r| r.action == AccessAction::Allow)
            .collect();
        if !allow_rules.is_empty() {
            let allowed = allow_rules.iter().any(|r| r.location.matches(&target));
            if !allowed {
                ctx.mock_response = Some(Self::block_response(403));
                return MiddlewareAction::StopAndReturn;
            }
        }

        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::matcher::Location;
    use crate::middleware::{HeaderMap, Middleware, MiddlewareAction, ResponseContext};
    use bytes::Bytes;

    fn req(host: &str, path: &str) -> RequestContext {
        RequestContext {
            method: "GET".into(),
            host: host.into(),
            uri: path.into(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
            ..Default::default()
        }
    }

    fn block_rule(host: &str) -> AccessRule {
        AccessRule {
            id: "t".into(),
            name: "t".into(),
            enabled: true,
            location: Location {
                host: Some(host.into()),
                ..Default::default()
            },
            action: AccessAction::Block,
        }
    }

    fn allow_rule(host: &str) -> AccessRule {
        AccessRule {
            id: "t".into(),
            name: "t".into(),
            enabled: true,
            location: Location {
                host: Some(host.into()),
                ..Default::default()
            },
            action: AccessAction::Allow,
        }
    }

    #[tokio::test]
    async fn no_rules_passes_all() {
        let mw = AccessControlMiddleware::new(vec![]);
        let mut ctx = req("anything.example.com", "/");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
        assert!(ctx.mock_response.is_none());
    }

    #[tokio::test]
    async fn block_rule_matching_host_returns_stop() {
        let mw = AccessControlMiddleware::new(vec![block_rule("blocked.example.com")]);
        let mut ctx = req("blocked.example.com", "/");
        assert_eq!(
            mw.on_request(&mut ctx).await,
            MiddlewareAction::StopAndReturn
        );
        assert_eq!(ctx.mock_response.as_ref().unwrap().status, 403);
        assert!(
            ctx.mock_response
                .unwrap()
                .tags
                .contains(&"access-blocked".to_string())
        );
    }

    #[tokio::test]
    async fn block_rule_non_matching_host_passes() {
        let mw = AccessControlMiddleware::new(vec![block_rule("blocked.example.com")]);
        let mut ctx = req("other.example.com", "/");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
        assert!(ctx.mock_response.is_none());
    }

    #[tokio::test]
    async fn allow_rule_matching_host_passes() {
        let mw = AccessControlMiddleware::new(vec![allow_rule("allowed.example.com")]);
        let mut ctx = req("allowed.example.com", "/");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
        assert!(ctx.mock_response.is_none());
    }

    #[tokio::test]
    async fn allow_rule_non_matching_host_blocked() {
        let mw = AccessControlMiddleware::new(vec![allow_rule("allowed.example.com")]);
        let mut ctx = req("other.example.com", "/");
        assert_eq!(
            mw.on_request(&mut ctx).await,
            MiddlewareAction::StopAndReturn
        );
        assert_eq!(ctx.mock_response.as_ref().unwrap().status, 403);
    }

    #[tokio::test]
    async fn disabled_block_rule_does_not_block() {
        let mut rule = block_rule("blocked.example.com");
        rule.enabled = false;
        let mw = AccessControlMiddleware::new(vec![rule]);
        let mut ctx = req("blocked.example.com", "/");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn block_takes_priority_over_allow() {
        // If a request matches a Block rule, it is blocked even if Allow rules exist.
        let mw = AccessControlMiddleware::new(vec![
            block_rule("evil.example.com"),
            allow_rule("evil.example.com"),
        ]);
        let mut ctx = req("evil.example.com", "/");
        assert_eq!(
            mw.on_request(&mut ctx).await,
            MiddlewareAction::StopAndReturn
        );
    }

    #[tokio::test]
    async fn path_filter_on_block_rule() {
        let mw = AccessControlMiddleware::new(vec![AccessRule {
            id: "t".into(),
            name: "t".into(),
            enabled: true,
            location: Location {
                host: Some("api.local".into()),
                path: Some("/admin/*".into()),
                ..Default::default()
            },
            action: AccessAction::Block,
        }]);
        // /admin/settings → blocked
        let mut ctx1 = req("api.local", "/admin/settings");
        assert_eq!(
            mw.on_request(&mut ctx1).await,
            MiddlewareAction::StopAndReturn
        );
        // /api/data → allowed through
        let mut ctx2 = req("api.local", "/api/data");
        assert_eq!(mw.on_request(&mut ctx2).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn multiple_allow_rules_any_match_passes() {
        let mw = AccessControlMiddleware::new(vec![
            allow_rule("a.example.com"),
            allow_rule("b.example.com"),
        ]);
        let mut ctx_a = req("a.example.com", "/");
        assert_eq!(mw.on_request(&mut ctx_a).await, MiddlewareAction::Continue);
        let mut ctx_b = req("b.example.com", "/");
        assert_eq!(mw.on_request(&mut ctx_b).await, MiddlewareAction::Continue);
        let mut ctx_c = req("c.example.com", "/");
        assert_eq!(
            mw.on_request(&mut ctx_c).await,
            MiddlewareAction::StopAndReturn
        );
    }

    #[tokio::test]
    async fn on_response_always_continues() {
        let mw = AccessControlMiddleware::new(vec![block_rule("any.host")]);
        let mut ctx = ResponseContext {
            status: 200,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            request_uri: "/".to_string(),
            ..Default::default()
        };
        assert_eq!(mw.on_response(&mut ctx).await, MiddlewareAction::Continue);
    }
}
