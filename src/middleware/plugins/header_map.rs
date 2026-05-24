use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum HmScope {
    #[default]
    All,
    Host,
    Path,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum HmAction {
    #[default]
    Set,
    Append,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderMapRule {
    pub id: String,
    #[serde(default)]
    pub scope: HmScope,
    #[serde(default)]
    pub r#match: String,
    #[serde(default)]
    pub action: HmAction,
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl HeaderMapRule {
    fn matches_request(&self, req: &RequestContext) -> bool {
        if !self.enabled {
            return false;
        }
        match self.scope {
            HmScope::All => true,
            HmScope::Host => req.host.contains(&self.r#match),
            HmScope::Path => {
                if self.r#match.is_empty() {
                    return true;
                }
                Regex::new(&self.r#match)
                    .map(|re| re.is_match(&req.uri))
                    .unwrap_or(false)
            }
        }
    }

    fn apply_to_headers(&self, headers: &mut std::collections::HashMap<String, String>) {
        let key = self.name.to_lowercase();
        match self.action {
            HmAction::Set => {
                headers.insert(key, self.value.clone());
            }
            HmAction::Append => {
                let existing = headers.get(&key).cloned().unwrap_or_default();
                let sep = if existing.is_empty() { "" } else { ", " };
                headers.insert(key, format!("{existing}{sep}{}", self.value));
            }
            HmAction::Remove => {
                headers.remove(&key);
            }
        }
    }
}

pub struct HeaderMapMiddleware {
    pub rules: Arc<RwLock<Vec<HeaderMapRule>>>,
}

impl HeaderMapMiddleware {
    pub fn new(rules: Vec<HeaderMapRule>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
        }
    }
}

#[async_trait]
impl Middleware for HeaderMapMiddleware {
    fn name(&self) -> &'static str {
        "header_map"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        for rule in rules.iter() {
            if rule.matches_request(ctx) {
                rule.apply_to_headers(&mut ctx.headers);
            }
        }
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

    fn rule(
        scope: HmScope,
        r#match: &str,
        action: HmAction,
        name: &str,
        value: &str,
    ) -> HeaderMapRule {
        HeaderMapRule {
            id: "test".to_string(),
            scope,
            r#match: r#match.to_string(),
            action,
            name: name.to_string(),
            value: value.to_string(),
            enabled: true,
        }
    }

    fn req(uri: &str, host: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            host: host.to_string(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: None,
        }
    }

    // ── Action: Set ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_inserts_new_header() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::All,
            "",
            HmAction::Set,
            "X-Custom",
            "hello",
        )]);
        let mut ctx = req("/", "example.com");
        mw.on_request(&mut ctx).await;
        assert_eq!(
            ctx.headers.get("x-custom").map(String::as_str),
            Some("hello")
        );
    }

    #[tokio::test]
    async fn set_overwrites_existing_header() {
        let mw =
            HeaderMapMiddleware::new(vec![rule(HmScope::All, "", HmAction::Set, "X-Foo", "new")]);
        let mut ctx = req("/", "example.com");
        ctx.headers.insert("x-foo".to_string(), "old".to_string());
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-foo").map(String::as_str), Some("new"));
    }

    // ── Action: Append ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn append_joins_with_comma_space() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::All,
            "",
            HmAction::Append,
            "Accept",
            "text/html",
        )]);
        let mut ctx = req("/", "example.com");
        ctx.headers
            .insert("accept".to_string(), "application/json".to_string());
        mw.on_request(&mut ctx).await;
        assert_eq!(
            ctx.headers.get("accept").map(String::as_str),
            Some("application/json, text/html")
        );
    }

    #[tokio::test]
    async fn append_on_missing_header_sets_value() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::All,
            "",
            HmAction::Append,
            "X-New",
            "val",
        )]);
        let mut ctx = req("/", "example.com");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-new").map(String::as_str), Some("val"));
    }

    // ── Action: Remove ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn remove_deletes_header() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::All,
            "",
            HmAction::Remove,
            "Authorization",
            "",
        )]);
        let mut ctx = req("/", "example.com");
        ctx.headers
            .insert("authorization".to_string(), "Bearer secret".to_string());
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn remove_on_missing_header_is_noop() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::All,
            "",
            HmAction::Remove,
            "X-Ghost",
            "",
        )]);
        let mut ctx = req("/", "example.com");
        mw.on_request(&mut ctx).await; // must not panic
        assert!(!ctx.headers.contains_key("x-ghost"));
    }

    // ── Disabled rules ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn disabled_rule_is_skipped() {
        let mut r = rule(
            HmScope::All,
            "",
            HmAction::Set,
            "X-Should-Not-Appear",
            "yes",
        );
        r.enabled = false;
        let mw = HeaderMapMiddleware::new(vec![r]);
        let mut ctx = req("/", "example.com");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-should-not-appear"));
    }

    // ── Scope: Host ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn host_scope_matches_substring() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::Host,
            "api.example",
            HmAction::Set,
            "X-Api",
            "1",
        )]);
        let mut ctx = req("/data", "api.example.com");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-api").map(String::as_str), Some("1"));
    }

    #[tokio::test]
    async fn host_scope_does_not_match_different_host() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::Host,
            "api.example",
            HmAction::Set,
            "X-Api",
            "1",
        )]);
        let mut ctx = req("/data", "other.com");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-api"));
    }

    // ── Scope: Path ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn path_scope_matches_regex() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::Path,
            r"^/api/.*",
            HmAction::Set,
            "X-Api-Route",
            "yes",
        )]);
        let mut ctx = req("/api/users", "example.com");
        mw.on_request(&mut ctx).await;
        assert_eq!(
            ctx.headers.get("x-api-route").map(String::as_str),
            Some("yes")
        );
    }

    #[tokio::test]
    async fn path_scope_does_not_match_non_matching_uri() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::Path,
            r"^/api/.*",
            HmAction::Set,
            "X-Api-Route",
            "yes",
        )]);
        let mut ctx = req("/static/img.png", "example.com");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-api-route"));
    }

    #[tokio::test]
    async fn path_scope_invalid_regex_does_not_panic() {
        let mw = HeaderMapMiddleware::new(vec![rule(
            HmScope::Path,
            r"[invalid(",
            HmAction::Set,
            "X-Bad",
            "1",
        )]);
        let mut ctx = req("/api/test", "example.com");
        mw.on_request(&mut ctx).await; // must not panic
        assert!(!ctx.headers.contains_key("x-bad"));
    }

    // ── Multiple rules ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn multiple_rules_applied_in_order() {
        let mw = HeaderMapMiddleware::new(vec![
            rule(HmScope::All, "", HmAction::Set, "X-Step", "first"),
            rule(HmScope::All, "", HmAction::Set, "X-Step", "second"),
            rule(HmScope::All, "", HmAction::Append, "X-Log", "a"),
            rule(HmScope::All, "", HmAction::Append, "X-Log", "b"),
        ]);
        let mut ctx = req("/", "example.com");
        mw.on_request(&mut ctx).await;
        assert_eq!(
            ctx.headers.get("x-step").map(String::as_str),
            Some("second")
        );
        assert_eq!(ctx.headers.get("x-log").map(String::as_str), Some("a, b"));
    }
}
