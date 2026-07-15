use async_trait::async_trait;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::forward::encode_grpc_frame;
use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{InterceptedResponse, Middleware, MiddlewareAction, RequestContext};
use crate::session::{FlowRuleKind, FlowShortCircuitReason, RequestFlowEvent};
use bytes::Bytes;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsFrameAction {
    pub opcode: u8,
    #[serde(default)]
    pub payload: String,
    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcMessageAction {
    #[serde(default)]
    pub compressed: bool,
    #[serde(default)]
    pub payload_base64: String,
    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelDecision {
    pub allow: bool,
    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MockBehavior {
    HttpResponse {
        responses: Vec<MockResponse>,
    },
    WebSocketScript {
        frames: Vec<WsFrameAction>,
    },
    GrpcScript {
        messages: Vec<GrpcMessageAction>,
        /// Optional gRPC trailers (e.g. grpc-status); surfaced as response
        /// headers in the Trailers-Only shape. Defaults to empty so assistant/UI
        /// payloads may omit it.
        #[serde(default)]
        trailers: crate::middleware::HeaderMap,
    },
    TunnelDecision {
        decision: TunnelDecision,
    },
}

impl MockBehavior {
    fn kind(&self) -> &'static str {
        match self {
            MockBehavior::HttpResponse { .. } => "http_response",
            MockBehavior::WebSocketScript { .. } => "websocket_script",
            MockBehavior::GrpcScript { .. } => "grpc_script",
            MockBehavior::TunnelDecision { .. } => "tunnel_decision",
        }
    }

    fn http_responses(&self) -> Option<&[MockResponse]> {
        match self {
            MockBehavior::HttpResponse { responses } => Some(responses),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockRule {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Full Location-based matching (host, path, port, protocol, query, methods, mode).
    pub location: Location,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<MockBehavior>,
    #[serde(default)]
    pub responses: Vec<MockResponse>,
    #[serde(default)]
    pub call_count: u64,
}

impl MockRule {
    pub fn matches(&self, ctx: &RequestContext) -> bool {
        self.enabled && self.location.matches(&MatchTarget::from_request(ctx))
    }

    pub fn current_response(&self) -> Option<&MockResponse> {
        let responses = self
            .behavior
            .as_ref()
            .and_then(MockBehavior::http_responses)
            .unwrap_or(&self.responses);
        if responses.is_empty() {
            return None;
        }
        let idx = (self.call_count as usize) % responses.len();
        responses.get(idx)
    }

    pub fn behavior_kind(&self) -> &'static str {
        self.behavior
            .as_ref()
            .map(MockBehavior::kind)
            .unwrap_or("http_response")
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
        let snapshots: Vec<MockRule> = {
            let rules = self.rules.read().await;
            rules.iter().filter(|r| r.enabled).cloned().collect()
        };

        for rule in snapshots {
            if !rule.matches(ctx) {
                continue;
            }
            ctx.push_flow(RequestFlowEvent::RuleMatched {
                rule_kind: FlowRuleKind::Mock,
                rule_id: rule.id.clone(),
                rule_name: rule.name.clone(),
            });

            if let Some(MockBehavior::GrpcScript { messages, trailers }) = rule.behavior.clone() {
                if !is_grpc_request(ctx) {
                    continue;
                }
                {
                    // Look the live rule up by id, not snapshot index: rules may
                    // have been edited/reordered concurrently (TOCTOU).
                    let mut rules = self.rules.write().await;
                    if let Some(rule_mut) = rules.iter_mut().find(|r| r.id == rule.id) {
                        rule_mut.call_count += 1;
                    }
                }
                let body = grpc_script_body(&messages).await;
                let mut headers = crate::middleware::HeaderMap::new();
                headers.insert("content-type".to_string(), "application/grpc".to_string());
                headers.insert("content-length".to_string(), body.len().to_string());
                // Mock responses are buffered, so configured trailers (e.g.
                // `grpc-status`) are surfaced as response headers — matching the
                // gRPC "Trailers-Only" shape clients already understand.
                for (name, value) in &trailers {
                    headers.insert(name.clone(), value.clone());
                }
                if !headers.contains_key("grpc-status") {
                    headers.insert("grpc-status".to_string(), "0".to_string());
                }
                ctx.mock_response = Some(InterceptedResponse {
                    status: 200,
                    headers,
                    body: Bytes::from(body),
                    tags: vec!["mock".to_string()],
                    served_mock: Some(crate::middleware::ServedMock {
                        rule_id: rule.id.clone(),
                        behavior: "grpc_script".to_string(),
                    }),
                });
                ctx.push_flow(RequestFlowEvent::ShortCircuited {
                    reason: FlowShortCircuitReason::MockResponse,
                    status: 200,
                    rule_id: Some(rule.id.clone()),
                    rule_name: Some(rule.name.clone()),
                });
                return MiddlewareAction::StopAndReturn;
            }

            let (resp, body, delay_ms) = {
                // Look the live rule up by id, not snapshot index: rules may have
                // been edited/reordered concurrently (TOCTOU). If the rule was
                // deleted since the snapshot, serve from the snapshot.
                let mut rules = self.rules.write().await;
                let resp = match rules.iter_mut().find(|r| r.id == rule.id) {
                    Some(rule_mut) => {
                        let resp = match rule_mut.current_response() {
                            Some(r) => r.clone(),
                            None => continue,
                        };
                        rule_mut.call_count += 1;
                        resp
                    }
                    None => match rule.current_response() {
                        Some(r) => r.clone(),
                        None => continue,
                    },
                };
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
                served_mock: Some(crate::middleware::ServedMock {
                    rule_id: rule.id.clone(),
                    behavior: rule.behavior_kind().to_string(),
                }),
            });
            ctx.push_flow(RequestFlowEvent::ShortCircuited {
                reason: FlowShortCircuitReason::MockResponse,
                status: resp.status,
                rule_id: Some(rule.id.clone()),
                rule_name: Some(rule.name.clone()),
            });
            return MiddlewareAction::StopAndReturn;
        }
        MiddlewareAction::Continue
    }
}

fn is_grpc_request(ctx: &RequestContext) -> bool {
    ctx.protocol_context
        .as_ref()
        .map(|protocol| protocol.application == crate::core::forward::ApplicationProtocol::Grpc)
        .unwrap_or(false)
        || ctx
            .headers
            .get("content-type")
            .map(|ct| ct.starts_with("application/grpc"))
            .unwrap_or(false)
}

async fn grpc_script_body(messages: &[GrpcMessageAction]) -> Vec<u8> {
    let mut body = Vec::new();
    for message in messages {
        if message.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(message.delay_ms)).await;
        }
        let payload = base64::engine::general_purpose::STANDARD
            .decode(message.payload_base64.as_bytes())
            .unwrap_or_default();
        body.extend_from_slice(&encode_grpc_frame(message.compressed, &payload));
    }
    body
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
            behavior: None,
            responses: vec![MockResponse {
                status,
                headers: HashMap::new(),
                body: body.to_string(),
                delay_ms: 0,
            }],
            call_count: 0,
        }
    }

    fn grpc_script_rule(id: &str, path: &str, payload_base64: &str) -> MockRule {
        MockRule {
            id: id.to_string(),
            name: id.to_string(),
            enabled: true,
            location: Location {
                path: Some(path.to_string()),
                mode: MatchMode::Regex,
                application_protocol: Some("grpc".to_string()),
                body_mode: Some("stream_messages".to_string()),
                ..Default::default()
            },
            behavior: Some(MockBehavior::GrpcScript {
                messages: vec![GrpcMessageAction {
                    compressed: false,
                    payload_base64: payload_base64.to_string(),
                    delay_ms: 0,
                }],
                trailers: crate::middleware::HeaderMap::new(),
            }),
            responses: Vec::new(),
            call_count: 0,
        }
    }

    fn make_grpc_ctx(uri: &str) -> RequestContext {
        let mut ctx = make_ctx("POST", "grpc.example.com", uri);
        ctx.headers.insert(
            "content-type".to_string(),
            "application/grpc+proto".to_string(),
        );
        ctx.protocol_context = Some(crate::core::forward::ProtocolContext {
            downstream: crate::core::forward::WireProtocol::Http2,
            upstream: None,
            application: crate::core::forward::ApplicationProtocol::Grpc,
            body_mode: crate::core::forward::BodyMode::StreamMessages,
            scheme: "https".to_string(),
            connection_id: None,
            stream_id: None,
        });
        ctx
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
    fn old_mock_json_defaults_to_http_response_behavior() {
        let rule: MockRule = serde_json::from_value(serde_json::json!({
            "id": "old",
            "name": "legacy",
            "enabled": true,
            "location": {},
            "responses": [{
                "status": 200,
                "headers": {},
                "body": "ok"
            }]
        }))
        .unwrap();

        assert!(rule.behavior.is_none());
        assert_eq!(rule.behavior_kind(), "http_response");
        assert_eq!(rule.current_response().unwrap().body, "ok");
    }

    #[test]
    fn http_behavior_variant_supplies_responses() {
        let rule: MockRule = serde_json::from_value(serde_json::json!({
            "id": "new",
            "name": "typed",
            "enabled": true,
            "location": {},
            "behavior": {
                "type": "http_response",
                "responses": [{
                    "status": 201,
                    "headers": {},
                    "body": "typed"
                }]
            }
        }))
        .unwrap();

        assert_eq!(rule.responses.len(), 0);
        assert_eq!(rule.behavior_kind(), "http_response");
        assert_eq!(rule.current_response().unwrap().status, 201);
        assert_eq!(rule.current_response().unwrap().body, "typed");
    }

    #[tokio::test]
    async fn grpc_script_returns_encoded_grpc_response() {
        let rules = Arc::new(RwLock::new(vec![grpc_script_rule(
            "grpc-script",
            r"/pkg\.Svc/Unary",
            "CgNuZXc=",
        )]));
        let mw = MockMiddleware::new(rules.clone());
        let mut ctx = make_grpc_ctx("/pkg.Svc/Unary");

        assert_eq!(
            mw.on_request(&mut ctx).await,
            MiddlewareAction::StopAndReturn
        );

        let response = ctx.mock_response.expect("grpc mock response");
        assert_eq!(response.status, 200);
        assert_eq!(
            response.headers.get("content-type").map(String::as_str),
            Some("application/grpc")
        );
        assert_eq!(
            response.body.as_ref(),
            &[0, 0, 0, 0, 5, 0x0a, 0x03, b'n', b'e', b'w']
        );
        assert_eq!(
            response.headers.get("grpc-status").map(String::as_str),
            Some("0"),
            "grpc script mocks default to grpc-status 0 (Trailers-Only shape)"
        );
        assert_eq!(response.served_mock.unwrap().behavior, "grpc_script");
        assert_eq!(rules.read().await[0].call_count, 1);
    }

    #[tokio::test]
    async fn grpc_script_does_not_match_non_grpc_request() {
        let rules = Arc::new(RwLock::new(vec![grpc_script_rule(
            "grpc-script",
            r"/pkg\.Svc/Unary",
            "CgNuZXc=",
        )]));
        let mw = MockMiddleware::new(rules.clone());
        let mut ctx = make_ctx("POST", "grpc.example.com", "/pkg.Svc/Unary");

        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
        assert!(ctx.mock_response.is_none());
        assert_eq!(rules.read().await[0].call_count, 0);
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
            behavior: None,
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
        assert_eq!(mock.served_mock.as_ref().unwrap().rule_id, "r1");
        assert_eq!(mock.served_mock.as_ref().unwrap().behavior, "http_response");
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
