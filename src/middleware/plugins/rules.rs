//! Unified rewrite-rule engine — the single replacement for the former
//! `RewriteMiddleware`, `HeaderMapMiddleware`, and `ModificationMiddleware`.
//!
//! A [`RewriteRuleSet`] couples a [`Location`] (multi-criteria matcher) with
//! an ordered list of [`RewriteAction`]s and an [`AppliesTo`] selector.
//! Rules are evaluated in insertion order; all enabled rules whose location
//! matches are applied (not first-match). Within a rule, actions are applied
//! in Vec order. If a `Block` or `Redirect` action fires it short-circuits the
//! rest of the chain for that request.

use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{
    InterceptedResponse, Middleware, MiddlewareAction, RequestContext, ResponseContext,
    append_header, path_of, remove_header, remove_query_param, set_header, set_query_param,
    split_path_query,
};
use async_trait::async_trait;
use bytes::Bytes;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;
use uuid::Uuid;

/// A regex pattern that compiles once on first use and caches the result.
/// Serialises/deserialises as a plain string for JSON compatibility.
#[derive(Debug)]
pub struct CompiledPattern {
    pub pattern: String,
    regex: OnceLock<Option<Arc<Regex>>>,
}

impl CompiledPattern {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            regex: OnceLock::new(),
        }
    }

    pub fn regex(&self) -> Option<&Regex> {
        self.regex
            .get_or_init(|| Regex::new(&self.pattern).ok().map(Arc::new))
            .as_deref()
    }
}

impl Clone for CompiledPattern {
    fn clone(&self) -> Self {
        Self::new(self.pattern.clone())
    }
}

impl PartialEq for CompiledPattern {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern
    }
}

impl Serialize for CompiledPattern {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.pattern.serialize(s)
    }
}

impl<'de> Deserialize<'de> for CompiledPattern {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::new(String::deserialize(d)?))
    }
}

impl From<String> for CompiledPattern {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for CompiledPattern {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

// ── Data model ────────────────────────────────────────────────────────────────

/// Which side of the exchange a rule applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppliesTo {
    /// Modify the outgoing request before forwarding.
    Request,
    /// Modify the incoming response before returning to the client.
    Response,
    /// Apply to both the request and the response.
    #[default]
    Both,
}

/// A single transformation to apply when a rule matches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RewriteAction {
    // ── Header mutations (request + response) ─────────────────────────────
    /// Set (overwrite) a header to a fixed value. Creates it if absent.
    SetHeader { name: String, value: String },
    /// Append a value to an existing header (CSV). Creates it if absent.
    AppendHeader { name: String, value: String },
    /// Remove a header entirely. No-op if absent.
    RemoveHeader { name: String },

    // ── Query-param mutations (request only) ──────────────────────────────
    /// Add or overwrite a query parameter.
    SetQueryParam { name: String, value: String },
    /// Remove a query parameter by name.
    RemoveQueryParam { name: String },

    // ── URL-part rewrites (request only) ─────────────────────────────────
    /// Replace the request's `Host` header value and internal host field.
    SetHost { value: String },
    /// Regex find-and-replace on the request path (not including query string).
    SetPath {
        pattern: CompiledPattern,
        replacement: String,
    },

    // ── Response status (response only) ──────────────────────────────────
    /// Override the HTTP response status code.
    SetStatus { code: u16 },

    // ── Body mutations (request + response) ──────────────────────────────
    /// Regex find-and-replace on the body text. Silently skips binary bodies.
    ReplaceBody {
        pattern: CompiledPattern,
        replacement: String,
    },

    // ── Short-circuit responses (request only) ────────────────────────────
    /// Return a redirect response immediately, bypassing the upstream.
    Redirect { status: u16, location: String },
    /// Return an error response immediately, bypassing the upstream.
    Block { status: u16 },
}

/// A named, ordered rule: when `location` matches and `applies_to` is
/// satisfied, every action in `actions` is applied in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteRuleSet {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub location: Location,
    #[serde(default)]
    pub applies_to: AppliesTo,
    #[serde(default)]
    pub actions: Vec<RewriteAction>,
}

fn default_true() -> bool {
    true
}

impl RewriteRuleSet {
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }
}

/// Shared handle to the live rule list — cloned into every handler that needs it.
pub type SharedRuleSets = Arc<RwLock<Vec<RewriteRuleSet>>>;

// ── Middleware ────────────────────────────────────────────────────────────────

pub struct UnifiedRewriteMiddleware {
    pub rules: SharedRuleSets,
}

impl UnifiedRewriteMiddleware {
    pub fn new(rules: Vec<RewriteRuleSet>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
        }
    }
}

#[async_trait]
impl Middleware for UnifiedRewriteMiddleware {
    fn name(&self) -> &str {
        "UnifiedRewriteMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        let target = MatchTarget::from_request(ctx);
        for rule in rules.iter().filter(|r| r.enabled) {
            if !matches!(rule.applies_to, AppliesTo::Response)
                && rule.location.matches(&target)
                && let Some(action) = apply_request_actions(rule, ctx)
            {
                return action;
            }
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        let target = MatchTarget::from_response(ctx);
        for rule in rules.iter().filter(|r| r.enabled) {
            if !matches!(rule.applies_to, AppliesTo::Request) && rule.location.matches(&target) {
                apply_response_actions(rule, ctx);
            }
        }
        MiddlewareAction::Continue
    }
}

// ── Request action application ────────────────────────────────────────────────

/// Apply all actions in a rule to the request context.
/// Returns `Some(StopAndReturn)` if a short-circuit action fires; otherwise `None`.
fn apply_request_actions(
    rule: &RewriteRuleSet,
    ctx: &mut RequestContext,
) -> Option<MiddlewareAction> {
    for action in &rule.actions {
        match action {
            RewriteAction::SetHeader { name, value } => {
                set_header(&mut ctx.headers, name, value.clone());
            }
            RewriteAction::AppendHeader { name, value } => {
                append_header(&mut ctx.headers, name, value);
            }
            RewriteAction::RemoveHeader { name } => {
                remove_header(&mut ctx.headers, name);
            }
            RewriteAction::SetQueryParam { name, value } => {
                ctx.uri = set_query_param(&ctx.uri, name, value);
            }
            RewriteAction::RemoveQueryParam { name } => {
                ctx.uri = remove_query_param(&ctx.uri, name);
            }
            RewriteAction::SetHost { value } => {
                ctx.host = value.clone();
                set_header(&mut ctx.headers, "host", value.clone());
                // Clear any existing destination so the engine re-resolves from new host.
                ctx.destination = None;
            }
            RewriteAction::SetPath {
                pattern,
                replacement,
            } => {
                if let Some(re) = pattern.regex() {
                    let (_, query) = split_path_query(&ctx.uri);
                    let new_path = re
                        .replace_all(path_of(&ctx.uri), replacement.as_str())
                        .to_string();
                    ctx.uri = if query.is_empty() {
                        new_path
                    } else {
                        format!("{new_path}?{query}")
                    };
                }
            }
            RewriteAction::ReplaceBody {
                pattern,
                replacement,
            } => {
                if let Some(re) = pattern.regex() {
                    let text = ctx.body_text().into_owned();
                    let new_body = re.replace_all(&text, replacement.as_str()).to_string();
                    if new_body != text {
                        ctx.set_body_text(new_body);
                        remove_header(&mut ctx.headers, "content-length");
                    }
                }
            }
            RewriteAction::Redirect { status, location } => {
                let mut headers = crate::middleware::HeaderMap::new();
                headers.insert("Location".to_string(), location.clone());
                ctx.mock_response = Some(InterceptedResponse {
                    status: *status,
                    headers,
                    body: Bytes::new(),
                    tags: Vec::new(),
                });
                return Some(MiddlewareAction::StopAndReturn);
            }
            RewriteAction::Block { status } => {
                ctx.mock_response = Some(InterceptedResponse {
                    status: *status,
                    headers: crate::middleware::HeaderMap::new(),
                    body: Bytes::new(),
                    tags: Vec::new(),
                });
                return Some(MiddlewareAction::StopAndReturn);
            }
            // Response-only actions are silently skipped on request.
            RewriteAction::SetStatus { .. } => {}
        }
    }
    None
}

// ── Response action application ───────────────────────────────────────────────

fn apply_response_actions(rule: &RewriteRuleSet, ctx: &mut ResponseContext) {
    for action in &rule.actions {
        match action {
            RewriteAction::SetHeader { name, value } => {
                set_header(&mut ctx.headers, name, value.clone());
            }
            RewriteAction::AppendHeader { name, value } => {
                append_header(&mut ctx.headers, name, value);
            }
            RewriteAction::RemoveHeader { name } => {
                remove_header(&mut ctx.headers, name);
            }
            RewriteAction::SetStatus { code } => {
                ctx.status = *code;
            }
            RewriteAction::ReplaceBody {
                pattern,
                replacement,
            } => {
                if let Some(re) = pattern.regex() {
                    let text = ctx.body_text().into_owned();
                    let new_body = re.replace_all(&text, replacement.as_str()).to_string();
                    if new_body != text {
                        ctx.set_body_text(new_body);
                        remove_header(&mut ctx.headers, "content-length");
                    }
                }
            }
            // Request-only actions are silently skipped on response.
            RewriteAction::SetQueryParam { .. }
            | RewriteAction::RemoveQueryParam { .. }
            | RewriteAction::SetHost { .. }
            | RewriteAction::SetPath { .. }
            | RewriteAction::Redirect { .. }
            | RewriteAction::Block { .. } => {}
        }
    }
}



// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::matcher::MatchMode;
    use crate::middleware::{
        HeaderMap, Middleware, MiddlewareAction, RequestContext, ResponseContext,
    };
    use bytes::Bytes;

    fn req(method: &str, host: &str, uri: &str, body: &str) -> RequestContext {
        RequestContext {
            method: method.into(),
            host: host.into(),
            uri: uri.into(),
            body: Bytes::from(body.to_string()),
            headers: HeaderMap::new(),
            ..Default::default()
        }
    }

    fn res(host: &str, method: &str, uri: &str, status: u16) -> ResponseContext {
        ResponseContext {
            status,
            request_uri: uri.into(),
            request_host: host.into(),
            request_method: method.into(),
            headers: HeaderMap::new(),
            body: Bytes::from("hello world"),
            ..Default::default()
        }
    }

    fn rule_set(
        location: Location,
        applies_to: AppliesTo,
        actions: Vec<RewriteAction>,
    ) -> RewriteRuleSet {
        RewriteRuleSet {
            id: "test".into(),
            name: "test rule".into(),
            enabled: true,
            location,
            applies_to,
            actions,
        }
    }

    fn host_loc(host: &str) -> Location {
        Location {
            host: Some(host.into()),
            ..Default::default()
        }
    }

    fn path_loc(path: &str) -> Location {
        Location {
            path: Some(path.into()),
            ..Default::default()
        }
    }

    fn path_regex_loc(path: &str) -> Location {
        Location {
            path: Some(path.into()),
            mode: MatchMode::Regex,
            ..Default::default()
        }
    }

    // ── disabled rule ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn disabled_rule_is_skipped() {
        let mut rs = rule_set(
            host_loc("example.com"),
            AppliesTo::Request,
            vec![RewriteAction::SetHeader {
                name: "x-test".into(),
                value: "1".into(),
            }],
        );
        rs.enabled = false;
        let mw = UnifiedRewriteMiddleware::new(vec![rs]);
        let mut ctx = req("GET", "example.com", "/", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-test"));
    }

    // ── SetHeader / AppendHeader / RemoveHeader ────────────────────────────

    #[tokio::test]
    async fn set_header_creates_and_overwrites() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::SetHeader {
                name: "X-Foo".into(),
                value: "bar".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/", "");
        ctx.headers.insert("x-foo", "old");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.headers.get("x-foo").map(String::as_str), Some("bar"));
    }

    #[tokio::test]
    async fn append_header_joins_with_comma() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::AppendHeader {
                name: "Accept".into(),
                value: "text/html".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/", "");
        ctx.headers
            .insert("accept", "application/json");
        mw.on_request(&mut ctx).await;
        assert_eq!(
            ctx.headers.get("accept").map(String::as_str),
            Some("application/json, text/html")
        );
    }

    #[tokio::test]
    async fn remove_header_case_insensitive() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::RemoveHeader {
                name: "Authorization".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/", "");
        ctx.headers
            .insert("authorization", "Bearer s");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("authorization"));
    }

    // ── Query param mutations ──────────────────────────────────────────────

    #[tokio::test]
    async fn set_query_param_adds_new() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::SetQueryParam {
                name: "debug".into(),
                value: "true".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/api/users?x=1", "");
        mw.on_request(&mut ctx).await;
        assert!(ctx.uri.contains("debug=true"), "uri={}", ctx.uri);
        assert!(ctx.uri.contains("x=1"), "original param preserved");
    }

    #[tokio::test]
    async fn set_query_param_overwrites_existing() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::SetQueryParam {
                name: "page".into(),
                value: "2".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/x?page=1", "");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.uri, "/x?page=2");
    }

    #[tokio::test]
    async fn remove_query_param_drops_it() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::RemoveQueryParam {
                name: "debug".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/x?x=1&debug=true&y=2", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.uri.contains("debug"), "uri={}", ctx.uri);
        assert!(ctx.uri.contains("x=1") && ctx.uri.contains("y=2"));
    }

    // ── SetHost / SetPath ──────────────────────────────────────────────────

    #[tokio::test]
    async fn set_host_updates_host_and_header() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            host_loc("old.host"),
            AppliesTo::Request,
            vec![RewriteAction::SetHost {
                value: "new.host".into(),
            }],
        )]);
        let mut ctx = req("GET", "old.host", "/", "");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.host, "new.host");
        assert_eq!(
            ctx.headers.get("host").map(String::as_str),
            Some("new.host")
        );
        assert!(
            ctx.destination.is_none(),
            "destination cleared for re-resolve"
        );
    }

    #[tokio::test]
    async fn set_path_regex_replace() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::SetPath {
                pattern: r"^/v1/(.+)".into(),
                replacement: "/v2/$1".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/v1/users?page=1", "");
        mw.on_request(&mut ctx).await;
        assert!(ctx.uri.starts_with("/v2/users"), "uri={}", ctx.uri);
        assert!(ctx.uri.contains("page=1"), "query preserved");
    }

    // ── ReplaceBody ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn replace_body_regex_and_clears_content_length() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::ReplaceBody {
                pattern: "secret".into(),
                replacement: "REDACTED".into(),
            }],
        )]);
        let mut ctx = req("POST", "h", "/", "my secret data");
        ctx.headers.insert("content-length", "14");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.body_text(), "my REDACTED data");
        assert!(!ctx.headers.contains_key("content-length"));
    }

    // ── Redirect / Block ───────────────────────────────────────────────────

    #[tokio::test]
    async fn redirect_action_returns_stop_and_return() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::Redirect {
                status: 302,
                location: "https://new.example.com".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/", "");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        let mock = ctx.mock_response.unwrap();
        assert_eq!(mock.status, 302);
        assert_eq!(
            mock.headers.get("Location").map(String::as_str),
            Some("https://new.example.com")
        );
    }

    #[tokio::test]
    async fn block_action_returns_stop_and_return() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            path_loc("/admin/*"),
            AppliesTo::Request,
            vec![RewriteAction::Block { status: 403 }],
        )]);
        let mut ctx = req("GET", "h", "/admin/secret", "");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        assert_eq!(ctx.mock_response.unwrap().status, 403);
    }

    // ── multi-criteria Location ────────────────────────────────────────────

    #[tokio::test]
    async fn host_and_path_both_required() {
        let loc = Location {
            host: Some("api.example.com".into()),
            path: Some("/v2/*".into()),
            ..Default::default()
        };
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            loc,
            AppliesTo::Request,
            vec![RewriteAction::SetHeader {
                name: "x-hit".into(),
                value: "1".into(),
            }],
        )]);
        // host matches, path doesn't
        let mut ctx = req("GET", "api.example.com", "/v1/users", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-hit"));
        // both match
        let mut ctx2 = req("GET", "api.example.com", "/v2/users", "");
        mw.on_request(&mut ctx2).await;
        assert!(ctx2.headers.contains_key("x-hit"));
    }

    // ── AppliesTo ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn request_only_rule_does_not_run_on_response() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Request,
            vec![RewriteAction::SetHeader {
                name: "x-req".into(),
                value: "1".into(),
            }],
        )]);
        let mut ctx = res("h", "GET", "/", 200);
        mw.on_response(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-req"));
    }

    #[tokio::test]
    async fn response_only_rule_does_not_run_on_request() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Response,
            vec![RewriteAction::SetHeader {
                name: "x-res".into(),
                value: "1".into(),
            }],
        )]);
        let mut ctx = req("GET", "h", "/", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-res"));
    }

    #[tokio::test]
    async fn both_rule_runs_on_request_and_response() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Both,
            vec![RewriteAction::SetHeader {
                name: "x-both".into(),
                value: "1".into(),
            }],
        )]);
        let mut req_ctx = req("GET", "h", "/", "");
        mw.on_request(&mut req_ctx).await;
        assert!(req_ctx.headers.contains_key("x-both"));

        let mut res_ctx = res("h", "GET", "/", 200);
        mw.on_response(&mut res_ctx).await;
        assert!(res_ctx.headers.contains_key("x-both"));
    }

    // ── Response-side actions ──────────────────────────────────────────────

    #[tokio::test]
    async fn set_status_on_response() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Response,
            vec![RewriteAction::SetStatus { code: 201 }],
        )]);
        let mut ctx = res("h", "GET", "/", 200);
        mw.on_response(&mut ctx).await;
        assert_eq!(ctx.status, 201);
    }

    #[tokio::test]
    async fn replace_body_on_response() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            Location::default(),
            AppliesTo::Response,
            vec![RewriteAction::ReplaceBody {
                pattern: "hello".into(),
                replacement: "goodbye".into(),
            }],
        )]);
        let mut ctx = res("h", "GET", "/", 200);
        ctx.headers.insert("content-length", "11");
        mw.on_response(&mut ctx).await;
        assert_eq!(ctx.body_text(), "goodbye world");
        assert!(!ctx.headers.contains_key("content-length"));
    }

    // ── Response host/method matching ─────────────────────────────────────

    #[tokio::test]
    async fn response_matches_by_request_host() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            host_loc("api.example.com"),
            AppliesTo::Response,
            vec![RewriteAction::SetHeader {
                name: "x-host-hit".into(),
                value: "1".into(),
            }],
        )]);
        let mut ctx = res("api.example.com", "GET", "/", 200);
        mw.on_response(&mut ctx).await;
        assert!(ctx.headers.contains_key("x-host-hit"));

        let mut ctx2 = res("other.com", "GET", "/", 200);
        mw.on_response(&mut ctx2).await;
        assert!(!ctx2.headers.contains_key("x-host-hit"));
    }

    #[tokio::test]
    async fn response_matches_by_request_path() {
        let mw = UnifiedRewriteMiddleware::new(vec![rule_set(
            path_regex_loc(r"^/api/"),
            AppliesTo::Response,
            vec![RewriteAction::SetHeader {
                name: "x-api".into(),
                value: "1".into(),
            }],
        )]);
        let mut hit = res("h", "GET", "/api/users", 200);
        mw.on_response(&mut hit).await;
        assert!(hit.headers.contains_key("x-api"));

        let mut miss = res("h", "GET", "/static/img.png", 200);
        mw.on_response(&mut miss).await;
        assert!(!miss.headers.contains_key("x-api"));
    }

    // ── Multiple rules all applied ─────────────────────────────────────────

    #[tokio::test]
    async fn multiple_matching_rules_all_applied() {
        let mw = UnifiedRewriteMiddleware::new(vec![
            rule_set(
                Location::default(),
                AppliesTo::Request,
                vec![RewriteAction::SetHeader {
                    name: "x-first".into(),
                    value: "1".into(),
                }],
            ),
            rule_set(
                Location::default(),
                AppliesTo::Request,
                vec![RewriteAction::SetHeader {
                    name: "x-second".into(),
                    value: "2".into(),
                }],
            ),
        ]);
        let mut ctx = req("GET", "h", "/", "");
        mw.on_request(&mut ctx).await;
        assert!(ctx.headers.contains_key("x-first"));
        assert!(ctx.headers.contains_key("x-second"));
    }

    // ── query helpers unit tests ───────────────────────────────────────────

    #[test]
    fn query_helpers() {
        assert_eq!(set_query_param("/p?a=1&b=2", "b", "99"), "/p?a=1&b=99");
        assert_eq!(set_query_param("/p", "x", "1"), "/p?x=1");
        assert_eq!(remove_query_param("/p?a=1&b=2&c=3", "b"), "/p?a=1&c=3");
        assert_eq!(remove_query_param("/p?only=1", "only"), "/p");
        assert_eq!(remove_query_param("/p", "x"), "/p");
    }
}
