use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
#[allow(unused_imports)]
use serde_json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub path_pattern: String,
    pub responses: Vec<MockResponse>,
    #[serde(default)]
    pub call_count: u64,
}

impl MockRule {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn matches(&self, ctx: &RequestContext) -> bool {
        if !self.enabled {
            return false;
        }
        if let Some(ref m) = self.method
            && !m.eq_ignore_ascii_case(&ctx.method)
        {
            return false;
        }
        if let Some(ref h) = self.host
            && !h.is_empty()
            && !ctx.host.to_lowercase().contains(&h.to_lowercase())
        {
            return false;
        }
        if let Ok(re) = Regex::new(&self.path_pattern) {
            let path = extract_path(&ctx.uri);
            re.is_match(path)
        } else {
            false
        }
    }

    pub fn current_response(&self) -> Option<&MockResponse> {
        if self.responses.is_empty() {
            return None;
        }
        let idx = (self.call_count as usize) % self.responses.len();
        self.responses.get(idx)
    }
}

fn extract_path(uri: &str) -> &str {
    // Remove scheme + host if present
    let without_scheme = if let Some(rest) = uri.strip_prefix("http://") {
        rest
    } else if let Some(rest) = uri.strip_prefix("https://") {
        rest
    } else {
        return uri;
    };
    // Find first '/' after host
    if let Some(idx) = without_scheme.find('/') {
        &without_scheme[idx..]
    } else {
        "/"
    }
}

/// Substitute capture group references `${0}`, `${1}` etc. in body template.
pub fn apply_template(template: &str, captures: &regex::Captures<'_>) -> String {
    let mut result = template.to_string();
    for i in 0..captures.len() {
        let placeholder = format!("${{{}}}", i);
        let value = captures.get(i).map(|m| m.as_str()).unwrap_or("");
        result = result.replace(&placeholder, value);
    }
    result
}

pub type SharedMockRules = Arc<RwLock<Vec<MockRule>>>;

pub struct MockMiddleware {
    pub rules: SharedMockRules,
    regex_cache: Arc<RwLock<HashMap<String, Regex>>>,
}

impl MockMiddleware {
    pub fn new(rules: SharedMockRules) -> Self {
        Self {
            rules,
            regex_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn compiled(&self, pattern: &str) -> Option<Regex> {
        if let Some(re) = self.regex_cache.read().await.get(pattern) {
            return Some(re.clone());
        }
        let re = Regex::new(pattern).ok()?;
        self.regex_cache
            .write()
            .await
            .insert(pattern.to_string(), re.clone());
        Some(re)
    }
}

#[async_trait]
impl Middleware for MockMiddleware {
    fn name(&self) -> &str {
        "MockMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let path = extract_path(&ctx.uri).to_string();

        // Snapshot patterns + metadata without holding the write lock during regex work.
        let rule_snapshots: Vec<(usize, String, Option<String>, Option<String>, bool)> = {
            let rules = self.rules.read().await;
            rules
                .iter()
                .enumerate()
                .filter(|(_, r)| r.enabled)
                .map(|(i, r)| {
                    (
                        i,
                        r.path_pattern.clone(),
                        r.method.clone(),
                        r.host.clone(),
                        true,
                    )
                })
                .collect()
        };

        for (idx, pattern, method_filter, host_filter, _) in rule_snapshots {
            if let Some(ref m) = method_filter {
                if !m.eq_ignore_ascii_case(&ctx.method) {
                    continue;
                }
            }
            if let Some(ref h) = host_filter {
                if !h.is_empty() && !ctx.host.to_lowercase().contains(&h.to_lowercase()) {
                    continue;
                }
            }
            let re = match self.compiled(&pattern).await {
                Some(r) => r,
                None => continue,
            };
            if !re.is_match(&path) {
                continue;
            }

            let (resp, body, delay_ms) = {
                let mut rules = self.rules.write().await;
                let rule = &mut rules[idx];
                let resp = match rule.current_response() {
                    Some(r) => r.clone(),
                    None => continue,
                };
                rule.call_count += 1;
                let delay_ms = resp.delay_ms;
                let body = if let Some(caps) = re.captures(&path) {
                    apply_template(&resp.body, &caps)
                } else {
                    resp.body.clone()
                };
                (resp, body, delay_ms)
            };

            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }

            let mut resp_headers = resp.headers.clone();
            if !resp_headers.contains_key("content-length") {
                resp_headers.insert("content-length".to_string(), body.len().to_string());
            }
            resp_headers.insert("x-oproxy-tags".to_string(), "mock".to_string());

            let mock_payload = serde_json::json!({
                "status": resp.status,
                "headers": resp_headers,
                "body": body,
            });
            ctx.headers.insert(
                "x-oproxy-mock-response".to_string(),
                mock_payload.to_string(),
            );
            return MiddlewareAction::StopAndReturn;
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
    use crate::middleware::RequestContext;

    fn make_ctx(method: &str, uri: &str) -> RequestContext {
        RequestContext {
            method: method.to_string(),
            uri: uri.to_string(),
            headers: HashMap::new(),
            body: String::new(),
            host: "example.com".to_string(),
            body_bytes: None,
        }
    }

    fn simple_rule(
        id: &str,
        method: Option<&str>,
        path_pattern: &str,
        status: u16,
        body: &str,
    ) -> MockRule {
        MockRule {
            id: id.to_string(),
            name: id.to_string(),
            enabled: true,
            method: method.map(|s| s.to_string()),
            host: None,
            path_pattern: path_pattern.to_string(),
            responses: vec![MockResponse {
                status,
                headers: HashMap::new(),
                body: body.to_string(),
                delay_ms: 0,
            }],
            call_count: 0,
        }
    }

    #[test]
    fn rule_matches_by_method_and_path() {
        let rule = simple_rule("r1", Some("GET"), "^/api/users$", 200, "[]");
        let ctx = make_ctx("GET", "http://example.com/api/users");
        assert!(rule.matches(&ctx));
    }

    #[test]
    fn rule_does_not_match_wrong_method() {
        let rule = simple_rule("r1", Some("POST"), "^/api/users$", 200, "[]");
        let ctx = make_ctx("GET", "http://example.com/api/users");
        assert!(!rule.matches(&ctx));
    }

    #[test]
    fn rule_any_method_matches() {
        let rule = simple_rule("r1", None, "^/api/users$", 200, "[]");
        let get = make_ctx("GET", "http://example.com/api/users");
        let post = make_ctx("POST", "http://example.com/api/users");
        assert!(rule.matches(&get));
        assert!(rule.matches(&post));
    }

    #[test]
    fn disabled_rule_never_matches() {
        let mut rule = simple_rule("r1", None, "^/api/users$", 200, "[]");
        rule.enabled = false;
        let ctx = make_ctx("GET", "http://example.com/api/users");
        assert!(!rule.matches(&ctx));
    }

    #[test]
    fn response_rotates_on_multiple_calls() {
        let mut rule = MockRule {
            id: "r1".to_string(),
            name: "r1".to_string(),
            enabled: true,
            method: None,
            host: None,
            path_pattern: "^/api$".to_string(),
            responses: vec![
                MockResponse {
                    status: 200,
                    headers: HashMap::new(),
                    body: "first".to_string(),
                    delay_ms: 0,
                },
                MockResponse {
                    status: 201,
                    headers: HashMap::new(),
                    body: "second".to_string(),
                    delay_ms: 0,
                },
            ],
            call_count: 0,
        };
        assert_eq!(rule.current_response().unwrap().status, 200);
        rule.call_count = 1;
        assert_eq!(rule.current_response().unwrap().status, 201);
        rule.call_count = 2;
        assert_eq!(rule.current_response().unwrap().status, 200); // wraps
    }

    #[test]
    fn template_substitution_applied() {
        let re = Regex::new("^/users/([0-9]+)$").unwrap();
        let caps = re.captures("/users/42").unwrap();
        let result = apply_template("user id is ${1}", &caps);
        assert_eq!(result, "user id is 42");
    }

    #[tokio::test]
    async fn middleware_returns_stop_and_return_for_matching_rule() {
        let rule = simple_rule("r1", Some("GET"), "^/api$", 200, "mocked");
        let rules = Arc::new(RwLock::new(vec![rule]));
        let mw = MockMiddleware::new(rules);
        let mut ctx = make_ctx("GET", "http://example.com/api");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        // Mock response is encoded into request context header
        let mock_resp = ctx.headers.get("x-oproxy-mock-response").unwrap();
        let v: serde_json::Value = serde_json::from_str(mock_resp).unwrap();
        assert_eq!(v["status"], 200);
        assert_eq!(v["body"], "mocked");
    }

    #[tokio::test]
    async fn middleware_returns_continue_for_unmatched_request() {
        let rule = simple_rule("r1", Some("GET"), "^/api$", 200, "mocked");
        let rules = Arc::new(RwLock::new(vec![rule]));
        let mw = MockMiddleware::new(rules);
        let mut ctx = make_ctx("GET", "http://example.com/other");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn call_count_increments_after_match() {
        let rule = simple_rule("r1", None, "^/api$", 200, "ok");
        let rules = Arc::new(RwLock::new(vec![rule]));
        let mw = MockMiddleware::new(rules.clone());
        let mut ctx = make_ctx("GET", "http://example.com/api");
        mw.on_request(&mut ctx).await;
        let count = rules.read().await[0].call_count;
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn middleware_respects_host_filter() {
        let mut rule = simple_rule("r1", None, "^/api$", 200, "ok");
        rule.host = Some("api.example.com".to_string());
        let rules = Arc::new(RwLock::new(vec![rule]));
        let mw = MockMiddleware::new(rules);

        let mut other = make_ctx("GET", "http://static.example.com/api");
        other.host = "static.example.com".to_string();
        assert_eq!(mw.on_request(&mut other).await, MiddlewareAction::Continue);

        let mut matched = make_ctx("GET", "http://api.example.com/api");
        matched.host = "api.example.com".to_string();
        assert_eq!(
            mw.on_request(&mut matched).await,
            MiddlewareAction::StopAndReturn
        );
    }

    #[tokio::test]
    async fn first_matching_rule_wins_before_later_rules() {
        let first = simple_rule("first", None, "^/api$", 201, "first");
        let second = simple_rule("second", None, "^/api$", 202, "second");
        let rules = Arc::new(RwLock::new(vec![first, second]));
        let mw = MockMiddleware::new(rules);
        let mut ctx = make_ctx("GET", "http://example.com/api");

        assert_eq!(
            mw.on_request(&mut ctx).await,
            MiddlewareAction::StopAndReturn
        );
        let mock_resp = ctx.headers.get("x-oproxy-mock-response").unwrap();
        let v: serde_json::Value = serde_json::from_str(mock_resp).unwrap();
        assert_eq!(v["status"], 201);
        assert_eq!(v["body"], "first");
    }

    #[test]
    fn extract_path_strips_host() {
        assert_eq!(extract_path("http://example.com/api/v1"), "/api/v1");
        assert_eq!(extract_path("https://api.com/users?q=1"), "/users?q=1");
        assert_eq!(extract_path("/direct/path"), "/direct/path");
    }
}
