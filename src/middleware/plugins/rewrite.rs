use async_trait::async_trait;
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use regex::Regex;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MatchCriteria {
    Host(String),
    Path(String),
    Header { name: String, value: String },
    Body(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RewriteAction {
    ReplaceBody { pattern: String, replacement: String },
    AddHeader { name: String, value: String },
    RemoveHeader { name: String },
    ReplaceHeader { name: String, pattern: String, replacement: String },
    Redirect { status: u16, location: String },
    Block { status: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteRule {
    pub name: String,
    pub criteria: MatchCriteria,
    pub action: RewriteAction,
    pub enabled: bool,
}

pub struct RewriteMiddleware {
    pub rules: Arc<RwLock<Vec<RewriteRule>>>,
}

impl RewriteMiddleware {
    pub fn new(rules: Vec<RewriteRule>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
        }
    }

    fn matches(&self, rule: &RewriteRule, req: &RequestContext) -> bool {
        match &rule.criteria {
            MatchCriteria::Host(host) => req.host.contains(host),
            MatchCriteria::Path(path) => {
                if let Ok(re) = Regex::new(path) {
                    re.is_match(&req.uri)
                } else {
                    false
                }
            }
            MatchCriteria::Header { name, value } => {
                req.headers.get(name).map_or(false, |v| v.contains(value))
            }
            MatchCriteria::Body(pattern) => {
                if let Ok(re) = Regex::new(pattern) {
                    re.is_match(&req.body)
                } else {
                    false
                }
            }
        }
    }

    fn matches_res(&self, rule: &RewriteRule, res: &ResponseContext) -> bool {
        match &rule.criteria {
            MatchCriteria::Path(path) => {
                if let Ok(re) = Regex::new(path) {
                    re.is_match(&res.request_uri)
                } else {
                    false
                }
            }
            MatchCriteria::Body(pattern) => {
                if let Ok(re) = Regex::new(pattern) {
                    re.is_match(&res.body)
                } else {
                    false
                }
            }
            _ => false, // Host and Header match criteria are primarily for requests
        }
    }

    fn apply_action(&self, rule: &RewriteRule, body: &mut String) {
        if let RewriteAction::ReplaceBody { pattern, replacement } = &rule.action {
            if let Ok(re) = Regex::new(pattern) {
                *body = re.replace_all(body, replacement).to_string();
            }
        }
    }

    fn apply_action_header(&self, rule: &RewriteRule, headers: &mut std::collections::HashMap<String, String>) {
        match &rule.action {
            RewriteAction::AddHeader { name, value } => {
                headers.insert(name.clone(), value.clone());
            }
            RewriteAction::RemoveHeader { name } => {
                headers.remove(name);
            }
            RewriteAction::ReplaceHeader { name, pattern, replacement } => {
                if let Some(val) = headers.get(name) {
                    if let Ok(re) = Regex::new(pattern) {
                        headers.insert(name.clone(), re.replace_all(val, replacement).to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

#[async_trait]
impl Middleware for RewriteMiddleware {
    fn name(&self) -> &str {
        "RewriteMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        for rule in rules.iter().filter(|r| r.enabled) {
            if self.matches(rule, ctx) {
                match &rule.action {
                    RewriteAction::Redirect { status, location } => {
                        let mock = serde_json::json!({
                            "status": status,
                            "headers": {"Location": location},
                            "body": ""
                        });
                        ctx.headers.insert("x-oproxy-mock-response".to_string(), mock.to_string());
                        return MiddlewareAction::StopAndReturn;
                    }
                    RewriteAction::Block { status } => {
                        let mock = serde_json::json!({
                            "status": status,
                            "headers": {},
                            "body": ""
                        });
                        ctx.headers.insert("x-oproxy-mock-response".to_string(), mock.to_string());
                        return MiddlewareAction::StopAndReturn;
                    }
                    _ => {
                        self.apply_action_header(rule, &mut ctx.headers);
                        let before = ctx.body.len();
                        self.apply_action(rule, &mut ctx.body);
                        if ctx.body.len() != before || ctx.body_bytes.is_some() {
                            ctx.body_bytes = None;
                        }
                    }
                }
            }
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        for rule in rules.iter().filter(|r| r.enabled) {
            if self.matches_res(rule, ctx) {
                self.apply_action_header(rule, &mut ctx.headers);
                let before = ctx.body.len();
                self.apply_action(&rule, &mut ctx.body);
                if ctx.body.len() != before || ctx.body_bytes.is_some() {
                    ctx.body_bytes = None;
                    // Content-Length from upstream is now stale — remove it so hyper
                    // doesn't panic on a length mismatch when we serve the modified body.
                    ctx.headers.remove("content-length");
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

    fn req(host: &str, uri: &str, body: &str) -> RequestContext {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        RequestContext { method: "GET".to_string(), uri: uri.to_string(), headers, body: body.to_string(), host: host.to_string(), body_bytes: None }
    }

    fn res(uri: &str, body: &str) -> ResponseContext {
        ResponseContext { status: 200, headers: HashMap::new(), body: body.to_string(), request_uri: uri.to_string(), session_id: None, ttfb_ms: 0, body_ms: 0, body_bytes: None }
    }

    fn rule(criteria: MatchCriteria, action: RewriteAction, enabled: bool) -> RewriteRule {
        RewriteRule { name: "test".to_string(), criteria, action, enabled }
    }

    // --- disabled rule ---

    #[tokio::test]
    async fn disabled_rule_is_skipped() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Host("example.com".to_string()),
            RewriteAction::AddHeader { name: "x-injected".to_string(), value: "1".to_string() },
            false,
        )]);
        let mut ctx = req("example.com", "/", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-injected"));
    }

    // --- Host criteria ---

    #[tokio::test]
    async fn host_criteria_substring_match() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Host("example".to_string()),
            RewriteAction::AddHeader { name: "x-hit".to_string(), value: "1".to_string() },
            true,
        )]);
        let mut ctx = req("api.example.com", "/", "");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-hit").map(|s| s.as_str()), Some("1"));
    }

    #[tokio::test]
    async fn host_criteria_no_match_leaves_headers_unchanged() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Host("other.com".to_string()),
            RewriteAction::AddHeader { name: "x-should-not-appear".to_string(), value: "1".to_string() },
            true,
        )]);
        let mut ctx = req("example.com", "/", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-should-not-appear"));
    }

    // --- Path criteria (regex) ---

    #[tokio::test]
    async fn path_regex_match_adds_header() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Path(r"^/api/".to_string()),
            RewriteAction::AddHeader { name: "x-api".to_string(), value: "true".to_string() },
            true,
        )]);
        let mut ctx = req("host", "/api/users", "");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-api").map(|s| s.as_str()), Some("true"));
    }

    #[tokio::test]
    async fn path_regex_no_match_does_nothing() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Path(r"^/api/".to_string()),
            RewriteAction::AddHeader { name: "x-api".to_string(), value: "true".to_string() },
            true,
        )]);
        let mut ctx = req("host", "/health", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-api"));
    }

    // --- Header criteria ---

    #[tokio::test]
    async fn header_criteria_value_substring_match() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Header { name: "content-type".to_string(), value: "json".to_string() },
            RewriteAction::AddHeader { name: "x-json".to_string(), value: "yes".to_string() },
            true,
        )]);
        let mut ctx = req("host", "/", "");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-json").map(|s| s.as_str()), Some("yes"));
    }

    // --- Body criteria ---

    #[tokio::test]
    async fn body_criteria_replace_body() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Body(r"secret".to_string()),
            RewriteAction::ReplaceBody { pattern: "secret".to_string(), replacement: "REDACTED".to_string() },
            true,
        )]);
        let mut ctx = req("host", "/", "my secret data");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.body, "my REDACTED data");
    }

    // --- Actions ---

    #[tokio::test]
    async fn add_header_action() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Host("h".to_string()),
            RewriteAction::AddHeader { name: "x-added".to_string(), value: "v".to_string() },
            true,
        )]);
        let mut ctx = req("h", "/", "");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-added").map(|s| s.as_str()), Some("v"));
    }

    #[tokio::test]
    async fn remove_header_action() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Host("h".to_string()),
            RewriteAction::RemoveHeader { name: "content-type".to_string() },
            true,
        )]);
        let mut ctx = req("h", "/", "");
        assert!(ctx.headers.contains_key("content-type"));
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("content-type"));
    }

    #[tokio::test]
    async fn replace_header_value_via_regex() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Host("h".to_string()),
            RewriteAction::ReplaceHeader {
                name: "content-type".to_string(),
                pattern: r"application/(.+)".to_string(),
                replacement: "text/$1".to_string(),
            },
            true,
        )]);
        let mut ctx = req("h", "/", "");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("content-type").map(|s| s.as_str()), Some("text/json"));
    }

    #[tokio::test]
    async fn replace_header_for_absent_header_is_noop() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Host("h".to_string()),
            RewriteAction::ReplaceHeader { name: "x-nonexistent".to_string(), pattern: ".*".to_string(), replacement: "val".to_string() },
            true,
        )]);
        let mut ctx = req("h", "/", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-nonexistent"));
    }

    // --- Invalid regex safety ---

    #[tokio::test]
    async fn invalid_path_regex_does_not_panic_and_skips_rule() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Path("[invalid regex".to_string()),
            RewriteAction::AddHeader { name: "x-bad".to_string(), value: "1".to_string() },
            true,
        )]);
        let mut ctx = req("host", "/test", "");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert!(!ctx.headers.contains_key("x-bad"));
    }

    // --- Response rewriting ---

    #[tokio::test]
    async fn response_path_criteria_replaces_body() {
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Path(r"^/api/".to_string()),
            RewriteAction::ReplaceBody { pattern: "foo".to_string(), replacement: "bar".to_string() },
            true,
        )]);
        let mut ctx = res("/api/test", "foo baz foo");
        mw.on_response(&mut ctx).await;
        assert_eq!(ctx.body, "bar baz bar");
    }

    #[tokio::test]
    async fn response_host_criteria_is_not_matched_on_response() {
        // Host and Header criteria return false for responses (no host on ResponseContext).
        let mw = RewriteMiddleware::new(vec![rule(
            MatchCriteria::Host("example.com".to_string()),
            RewriteAction::AddHeader { name: "x-res-host".to_string(), value: "1".to_string() },
            true,
        )]);
        let mut ctx = res("/any", "body");
        mw.on_response(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-res-host"), "Host criteria should not match on responses");
    }

    #[tokio::test]
    async fn multiple_rules_all_applied_when_matching() {
        let mw = RewriteMiddleware::new(vec![
            rule(MatchCriteria::Host("h".to_string()), RewriteAction::AddHeader { name: "x-first".to_string(), value: "1".to_string() }, true),
            rule(MatchCriteria::Host("h".to_string()), RewriteAction::AddHeader { name: "x-second".to_string(), value: "2".to_string() }, true),
        ]);
        let mut ctx = req("h", "/", "");
        mw.on_request(&mut ctx).await;
        assert!(ctx.headers.contains_key("x-first"));
        assert!(ctx.headers.contains_key("x-second"));
    }
}
