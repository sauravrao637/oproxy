use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{InterceptedResponse, Middleware, MiddlewareAction, RequestContext};
use bytes::Bytes;

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
    /// Full Location-based matching (host, path, port, protocol, query, methods, mode).
    pub location: Location,
    pub responses: Vec<MockResponse>,
    #[serde(default)]
    pub call_count: u64,
}

impl MockRule {
    pub fn matches(&self, ctx: &RequestContext) -> bool {
        self.enabled && self.location.matches(&MatchTarget::from_request(ctx))
    }

    pub fn current_response(&self) -> Option<&MockResponse> {
        if self.responses.is_empty() {
            return None;
        }
        let idx = (self.call_count as usize) % self.responses.len();
        self.responses.get(idx)
    }
}

/// Substitute capture group references `${0}`, `${1}` etc. in a body template.
/// Used when `location.mode == Regex` and the path pattern has capture groups.
pub fn apply_template(template: &str, captures: &regex::Captures<'_>) -> String {
    let mut result = template.to_string();
    for i in 0..captures.len() {
        let placeholder = format!("${{{i}}}");
        let value = captures.get(i).map(|m| m.as_str()).unwrap_or("");
        result = result.replace(&placeholder, value);
    }
    result
}

pub type SharedMockRules = Arc<RwLock<Vec<MockRule>>>;

pub struct MockMiddleware {
    pub rules: SharedMockRules,
}

impl MockMiddleware {
    pub fn new(rules: SharedMockRules) -> Self {
        Self { rules }
    }
}

#[async_trait]
impl Middleware for MockMiddleware {
    fn name(&self) -> &str {
        "MockMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let target = MatchTarget::from_request(ctx);

        // Snapshot enabled rules without holding the write lock during matching.
        let snapshots: Vec<(usize, MockRule)> = {
            let rules = self.rules.read().await;
            rules
                .iter()
                .enumerate()
                .filter(|(_, r)| r.enabled)
                .map(|(i, r)| (i, r.clone()))
                .collect()
        };

        for (idx, rule) in snapshots {
            if !rule.matches(ctx) {
                continue;
            }

            let (resp, body, delay_ms) = {
                let mut rules = self.rules.write().await;
                let rule_mut = &mut rules[idx];
                let resp = match rule_mut.current_response() {
                    Some(r) => r.clone(),
                    None => continue,
                };
                rule_mut.call_count += 1;
                let delay_ms = resp.delay_ms;

                // Capture-group template substitution for regex path patterns.
                // Reuse the already-cached compiled regex from location matching.
                let body = if let Some(re) = rule.location.compiled_path_regex() {
                    if let Some(caps) = re.captures(&target.path) {
                        apply_template(&resp.body, &caps)
                    } else {
                        resp.body.clone()
                    }
                } else {
                    resp.body.clone()
                };

                (resp, body, delay_ms)
            };

            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }

            let mut resp_headers: crate::middleware::HeaderMap = resp.headers.clone().into();
            if !resp_headers.contains_key("content-length") {
                resp_headers.insert("content-length".to_string(), body.len().to_string());
            }

            ctx.mock_response = Some(InterceptedResponse {
                status: resp.status,
                headers: resp_headers,
                body: Bytes::from(body),
                tags: vec!["mock".to_string()],
            });
            return MiddlewareAction::StopAndReturn;
        }
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::RequestContext;
    use crate::middleware::matcher::{Location, MatchMode};
    use regex::Regex;

    fn make_ctx(method: &str, host: &str, uri: &str) -> RequestContext {
        RequestContext {
            method: method.to_string(),
            uri: uri.to_string(),
            headers: crate::middleware::HeaderMap::new(),
            body: bytes::Bytes::new(),
            host: host.to_string(),
            ..Default::default()
        }
    }

    /// Build a rule matching on path (regex) + optional method list.
    fn rule_for_path(id: &str, path: &str, methods: &[&str], status: u16, body: &str) -> MockRule {
        MockRule {
            id: id.to_string(),
            name: id.to_string(),
            enabled: true,
            location: Location {
                path: Some(path.to_string()),
                mode: MatchMode::Regex,
                methods: methods.iter().map(|m| m.to_string()).collect(),
                ..Default::default()
            },
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
        let rule = rule_for_path("r1", "^/api/users$", &["GET"], 200, "[]");
        assert!(rule.matches(&make_ctx(
            "GET",
            "example.com",
            "http://example.com/api/users"
        )));
    }

    #[test]
    fn rule_does_not_match_wrong_method() {
        let rule = rule_for_path("r1", "^/api/users$", &["POST"], 200, "[]");
        assert!(!rule.matches(&make_ctx(
            "GET",
            "example.com",
            "http://example.com/api/users"
        )));
    }

    #[test]
    fn rule_any_method_matches() {
        let rule = rule_for_path("r1", "^/api/users$", &[], 200, "[]");
        assert!(rule.matches(&make_ctx(
            "GET",
            "example.com",
            "http://example.com/api/users"
        )));
        assert!(rule.matches(&make_ctx(
            "POST",
            "example.com",
            "http://example.com/api/users"
        )));
    }

    #[test]
    fn disabled_rule_never_matches() {
        let mut rule = rule_for_path("r1", "^/api/users$", &[], 200, "[]");
        rule.enabled = false;
        assert!(!rule.matches(&make_ctx(
            "GET",
            "example.com",
            "http://example.com/api/users"
        )));
    }

    #[test]
    fn host_filter_narrows_match() {
        let mut rule = rule_for_path("r1", "^/api$", &[], 200, "ok");
        rule.location.host = Some("api.example.com".to_string());
        // mode is Regex so host is matched as regex — "api.example.com" matches exactly
        assert!(!rule.matches(&make_ctx(
            "GET",
            "static.example.com",
            "http://static.example.com/api"
        )));
        assert!(rule.matches(&make_ctx(
            "GET",
            "api.example.com",
            "http://api.example.com/api"
        )));
    }

    #[test]
    fn response_rotates_on_multiple_calls() {
        let mut rule = MockRule {
            id: "r1".to_string(),
            name: "r1".to_string(),
            enabled: true,
            location: Location {
                path: Some("^/api$".to_string()),
                mode: MatchMode::Regex,
                ..Default::default()
            },
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
        let rule = rule_for_path("r1", "^/api$", &["GET"], 200, "mocked");
        let rules = Arc::new(RwLock::new(vec![rule]));
        let mw = MockMiddleware::new(rules);
        let mut ctx = make_ctx("GET", "example.com", "http://example.com/api");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        let mock = ctx.mock_response.as_ref().unwrap();
        assert_eq!(mock.status, 200);
        assert_eq!(&mock.body[..], b"mocked");
        assert_eq!(mock.tags, vec!["mock".to_string()]);
    }

    #[tokio::test]
    async fn middleware_returns_continue_for_unmatched_request() {
        let rule = rule_for_path("r1", "^/api$", &["GET"], 200, "mocked");
        let rules = Arc::new(RwLock::new(vec![rule]));
        let mw = MockMiddleware::new(rules);
        let mut ctx = make_ctx("GET", "example.com", "http://example.com/other");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn call_count_increments_after_match() {
        let rule = rule_for_path("r1", "^/api$", &[], 200, "ok");
        let rules = Arc::new(RwLock::new(vec![rule]));
        let mw = MockMiddleware::new(rules.clone());
        let mut ctx = make_ctx("GET", "example.com", "http://example.com/api");
        mw.on_request(&mut ctx).await;
        assert_eq!(rules.read().await[0].call_count, 1);
    }

    #[tokio::test]
    async fn middleware_respects_host_filter() {
        let mut rule = rule_for_path("r1", "^/api$", &[], 200, "ok");
        rule.location.host = Some("api.example.com".to_string());
        let rules = Arc::new(RwLock::new(vec![rule]));
        let mw = MockMiddleware::new(rules);

        let mut other = make_ctx("GET", "static.example.com", "http://static.example.com/api");
        assert_eq!(mw.on_request(&mut other).await, MiddlewareAction::Continue);

        let mut matched = make_ctx("GET", "api.example.com", "http://api.example.com/api");
        assert_eq!(
            mw.on_request(&mut matched).await,
            MiddlewareAction::StopAndReturn
        );
    }

    #[tokio::test]
    async fn first_matching_rule_wins_before_later_rules() {
        let first = rule_for_path("first", "^/api$", &[], 201, "first");
        let second = rule_for_path("second", "^/api$", &[], 202, "second");
        let rules = Arc::new(RwLock::new(vec![first, second]));
        let mw = MockMiddleware::new(rules);
        let mut ctx = make_ctx("GET", "example.com", "http://example.com/api");
        assert_eq!(
            mw.on_request(&mut ctx).await,
            MiddlewareAction::StopAndReturn
        );
        let mock = ctx.mock_response.as_ref().unwrap();
        assert_eq!(mock.status, 201);
        assert_eq!(&mock.body[..], b"first");
    }
}
