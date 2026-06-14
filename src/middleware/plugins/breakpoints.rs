use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, oneshot};
use uuid::Uuid;

// Breakpoints auto-drop after this long so no request handler leaks if the UI is closed.
const BREAKPOINT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BreakpointType {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BreakpointTier {
    /// Headers and metadata only. Safe for streams and tunnels.
    Head,
    /// Full request/response body editing. Compatible with buffered bodies only.
    #[default]
    Body,
    /// Frame/message-level pause for WebSocket frames or gRPC messages.
    Frame,
    /// SOCKS5/raw tunnel metadata pause only.
    Tunnel,
}

impl BreakpointTier {
    pub(crate) fn is_compatible_with_target(self, target: &MatchTarget) -> bool {
        match self {
            BreakpointTier::Head => true,
            BreakpointTier::Body => !matches!(
                target.body_mode.as_deref(),
                Some("tunnel" | "frames" | "stream_bytes" | "stream_messages")
            ),
            BreakpointTier::Frame => {
                matches!(
                    target.body_mode.as_deref(),
                    Some("frames" | "stream_messages")
                )
            }
            BreakpointTier::Tunnel => matches!(target.body_mode.as_deref(), Some("tunnel")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointDiagnostic {
    pub at: DateTime<Utc>,
    pub rule_id: String,
    pub bp_type: BreakpointType,
    pub tier: BreakpointTier,
    pub reason: String,
    pub method: String,
    pub host: String,
    pub path: String,
    pub wire_protocol: Option<String>,
    pub application_protocol: Option<String>,
    pub body_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointRule {
    #[serde(default)]
    pub id: String,
    /// Full Location-based matching (host, path, port, protocol, query, methods, mode).
    /// Leave all fields at default to match every request/response.
    pub location: Location,
    pub bp_type: BreakpointType,
    #[serde(default)]
    pub tier: BreakpointTier,
    pub enabled: bool,
}

pub struct PendingBreakpoint {
    pub id: String,
    pub bp_type: BreakpointType,
    pub context: BreakpointContext,
    pub tx: oneshot::Sender<BreakpointResolution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BreakpointContext {
    Request(Box<RequestContext>),
    Response(Box<ResponseContext>),
}

#[derive(Debug, Clone)]
pub enum BreakpointResolution {
    Continue,
    Modify(Box<BreakpointContext>),
    Drop,
}

pub struct BreakpointManager {
    pub rules: Arc<RwLock<Vec<BreakpointRule>>>,
    pub pending: Arc<RwLock<HashMap<String, PendingBreakpoint>>>,
    diagnostics: Arc<RwLock<Vec<BreakpointDiagnostic>>>,
}

impl BreakpointManager {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(RwLock::new(Vec::new())),
            pending: Arc::new(RwLock::new(HashMap::new())),
            diagnostics: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn add_rule(&self, rule: BreakpointRule) {
        self.rules.write().await.push(rule);
    }

    pub async fn resolve_breakpoint(
        &self,
        id: &str,
        resolution: BreakpointResolution,
    ) -> Result<(), String> {
        let mut pending = self.pending.write().await;
        if let Some(bp) = pending.remove(id) {
            let _ = bp.tx.send(resolution);
            Ok(())
        } else {
            Err("Breakpoint not found".to_string())
        }
    }

    pub async fn list_rules(&self) -> Vec<BreakpointRule> {
        self.rules.read().await.clone()
    }

    pub async fn list_diagnostics(&self) -> Vec<BreakpointDiagnostic> {
        self.diagnostics.read().await.clone()
    }

    async fn record_diagnostic(
        &self,
        rule: &BreakpointRule,
        target: &MatchTarget,
        reason: impl Into<String>,
    ) {
        let mut diagnostics = self.diagnostics.write().await;
        diagnostics.push(BreakpointDiagnostic {
            at: Utc::now(),
            rule_id: rule.id.clone(),
            bp_type: rule.bp_type.clone(),
            tier: rule.tier,
            reason: reason.into(),
            method: target.method.clone(),
            host: target.host.clone(),
            path: target.path.clone(),
            wire_protocol: target.wire_protocol.clone(),
            application_protocol: target.application_protocol.clone(),
            body_mode: target.body_mode.clone(),
        });
        const MAX_DIAGNOSTICS: usize = 50;
        if diagnostics.len() > MAX_DIAGNOSTICS {
            let drain = diagnostics.len() - MAX_DIAGNOSTICS;
            diagnostics.drain(0..drain);
        }
    }

    pub async fn delete_rule(&self, id: &str) {
        self.rules.write().await.retain(|r| r.id != id);
    }

    pub async fn update_rule(&self, id: &str, updated: BreakpointRule) -> bool {
        let mut rules = self.rules.write().await;
        let Some(rule) = rules.iter_mut().find(|rule| rule.id == id) else {
            return false;
        };
        let mut updated = updated;
        updated.id = id.to_string();
        *rule = updated;
        true
    }

    /// Returns the first matching enabled rule and records diagnostics for
    /// enabled rules whose Location matched but whose tier cannot run for the
    /// target protocol/body mode.
    pub async fn first_match(
        &self,
        bp_type_filter: impl Fn(&BreakpointType) -> bool,
        target: &MatchTarget,
    ) -> Option<BreakpointRule> {
        // Match under the read lock without cloning the whole rule set (this runs
        // per request); only matching/incompatible rules are cloned out, and
        // diagnostics are recorded after the lock is released.
        let (matched, incompatible) = {
            let rules = self.rules.read().await;
            let mut matched = None;
            let mut incompatible = Vec::new();
            for rule in rules
                .iter()
                .filter(|r| r.enabled && bp_type_filter(&r.bp_type))
            {
                if !rule.location.matches(target) {
                    continue;
                }
                if rule.tier.is_compatible_with_target(target) {
                    if matched.is_none() {
                        matched = Some(rule.clone());
                    }
                } else {
                    incompatible.push(rule.clone());
                }
            }
            (matched, incompatible)
        };
        for rule in &incompatible {
            self.record_diagnostic(
                rule,
                target,
                format!(
                    "{:?} breakpoint is incompatible with body mode {:?}",
                    rule.tier, target.body_mode
                ),
            )
            .await;
        }
        matched
    }

    /// Cheap fast path for per-frame callers: `true` when any enabled Frame-tier
    /// rule exists. Lets the WS relay and gRPC observer skip building a
    /// `MatchTarget` + breakpoint context for every frame when no frame
    /// breakpoints are configured (the overwhelmingly common case).
    pub async fn has_frame_rules(&self) -> bool {
        self.rules
            .read()
            .await
            .iter()
            .any(|r| r.enabled && r.tier == BreakpointTier::Frame)
    }

    pub async fn pause_frame(
        &self,
        session_manager: &crate::session::SharedSessionManager,
        session_id: &str,
        context: BreakpointContext,
    ) -> BreakpointResolution {
        let (target, bp_type) = match &context {
            BreakpointContext::Request(ctx) => {
                (MatchTarget::from_request(ctx), BreakpointType::Request)
            }
            BreakpointContext::Response(ctx) => {
                (MatchTarget::from_response(ctx), BreakpointType::Response)
            }
        };
        // Per-frame hot path: find the matching rule under the read lock and
        // clone only that rule (no whole-vec clone per frame).
        let rule = {
            let rules = self.rules.read().await;
            rules
                .iter()
                .find(|rule| {
                    rule.enabled
                        && std::mem::discriminant(&rule.bp_type) == std::mem::discriminant(&bp_type)
                        && rule.tier == BreakpointTier::Frame
                        && rule.tier.is_compatible_with_target(&target)
                        && rule.location.matches(&target)
                })
                .cloned()
        };
        let Some(rule) = rule else {
            return BreakpointResolution::Continue;
        };

        let bp_id = Uuid::new_v4().to_string();
        session_manager.mark_paused(session_id);
        session_manager.append_event(
            session_id,
            crate::session::SessionEvent::BreakpointPaused {
                breakpoint_id: bp_id.clone(),
            },
        );

        let (tx, rx) = oneshot::channel();
        self.pending.write().await.insert(
            bp_id.clone(),
            PendingBreakpoint {
                id: bp_id.clone(),
                bp_type: rule.bp_type,
                context,
                tx,
            },
        );

        let resolution = match tokio::time::timeout(BREAKPOINT_TIMEOUT, rx).await {
            Ok(Ok(resolution)) => resolution,
            Ok(Err(_)) | Err(_) => {
                self.pending.write().await.remove(&bp_id);
                tracing::warn!(id = %bp_id, "Frame breakpoint timed out, dropping frame");
                BreakpointResolution::Drop
            }
        };
        session_manager.clear_paused(session_id);
        resolution
    }
}

impl Default for BreakpointManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BreakpointMiddleware {
    pub manager: Arc<BreakpointManager>,
    pub session_manager: crate::session::SharedSessionManager,
}

impl BreakpointMiddleware {
    pub fn new(
        manager: Arc<BreakpointManager>,
        session_manager: crate::session::SharedSessionManager,
    ) -> Self {
        Self {
            manager,
            session_manager,
        }
    }

    /// Returns the first matching enabled rule of the given type.
    async fn first_match(
        &self,
        bp_type_filter: impl Fn(&BreakpointType) -> bool,
        target: &MatchTarget,
    ) -> Option<BreakpointRule> {
        self.manager.first_match(bp_type_filter, target).await
    }
}

#[async_trait]
impl Middleware for BreakpointMiddleware {
    fn name(&self) -> &str {
        "BreakpointMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let target = MatchTarget::from_request(ctx);
        if self
            .first_match(|t| matches!(t, BreakpointType::Request), &target)
            .await
            .is_none()
        {
            return MiddlewareAction::Continue;
        }

        let session_id = ctx
            .session_id
            .as_ref()
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // Write back so InspectionMiddleware reuses the same ID rather than
        // generating a fresh one and creating a duplicate ghost session.
        ctx.session_id = Some(session_id.clone());

        let bp_id = Uuid::new_v4().to_string();

        // Record the request immediately so it appears in the sessions list as paused
        self.session_manager.record_request_with_source(
            session_id.clone(),
            ctx.clone(),
            crate::session::SessionSource::Proxy,
        );
        self.session_manager.mark_paused(&session_id);
        self.session_manager.append_event(
            &session_id,
            crate::session::SessionEvent::BreakpointPaused {
                breakpoint_id: bp_id.clone(),
            },
        );

        let (tx, rx) = oneshot::channel();
        self.manager.pending.write().await.insert(
            bp_id.clone(),
            PendingBreakpoint {
                id: bp_id.clone(),
                bp_type: BreakpointType::Request,
                context: BreakpointContext::Request(Box::new(ctx.clone())),
                tx,
            },
        );

        match tokio::time::timeout(BREAKPOINT_TIMEOUT, rx).await {
            Ok(Ok(BreakpointResolution::Continue)) => {
                self.session_manager.clear_paused(&session_id);
                MiddlewareAction::Continue
            }
            Ok(Ok(BreakpointResolution::Modify(bc))) => {
                self.session_manager.clear_paused(&session_id);
                if let BreakpointContext::Request(new_ctx) = *bc {
                    *ctx = *new_ctx;
                    MiddlewareAction::Continue
                } else {
                    MiddlewareAction::StopAndReturn
                }
            }
            Ok(Ok(BreakpointResolution::Drop)) => {
                self.session_manager.clear_paused(&session_id);
                MiddlewareAction::StopAndReturn
            }
            Ok(Err(_)) => {
                self.session_manager.clear_paused(&session_id);
                MiddlewareAction::StopAndReturn
            }
            Err(_) => {
                self.manager.pending.write().await.remove(&bp_id);
                self.session_manager.clear_paused(&session_id);
                tracing::warn!(id = %bp_id, "Breakpoint request timed out, dropping");
                let mut headers = crate::middleware::HeaderMap::new();
                headers.insert("content-type".to_string(), "text/plain".to_string());
                ctx.mock_response = Some(crate::middleware::InterceptedResponse {
                    status: 504,
                    headers,
                    body: Bytes::from_static(b"Breakpoint timed out"),
                    tags: Vec::new(),
                    served_mock: None,
                });
                MiddlewareAction::StopAndReturn
            }
        }
    }

    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        let target = MatchTarget::from_response(ctx);
        if self
            .first_match(|t| matches!(t, BreakpointType::Response), &target)
            .await
            .is_none()
        {
            return MiddlewareAction::Continue;
        }

        let bp_id = Uuid::new_v4().to_string();

        // For response breakpoints, mark the session as paused if it exists
        if let Some(session_id) = &ctx.session_id {
            self.session_manager.mark_paused(session_id);
            self.session_manager.append_event(
                session_id,
                crate::session::SessionEvent::BreakpointPaused {
                    breakpoint_id: bp_id.clone(),
                },
            );
        }

        let (tx, rx) = oneshot::channel();
        self.manager.pending.write().await.insert(
            bp_id.clone(),
            PendingBreakpoint {
                id: bp_id.clone(),
                bp_type: BreakpointType::Response,
                context: BreakpointContext::Response(Box::new(ctx.clone())),
                tx,
            },
        );

        match tokio::time::timeout(BREAKPOINT_TIMEOUT, rx).await {
            Ok(Ok(BreakpointResolution::Continue)) => {
                if let Some(session_id) = &ctx.session_id {
                    self.session_manager.clear_paused(session_id);
                }
                MiddlewareAction::Continue
            }
            Ok(Ok(BreakpointResolution::Modify(bc))) => {
                if let Some(session_id) = &ctx.session_id {
                    self.session_manager.clear_paused(session_id);
                }
                if let BreakpointContext::Response(new_ctx) = *bc {
                    *ctx = *new_ctx;
                    MiddlewareAction::Continue
                } else {
                    MiddlewareAction::StopAndReturn
                }
            }
            Ok(Ok(BreakpointResolution::Drop)) => {
                if let Some(session_id) = &ctx.session_id {
                    self.session_manager.clear_paused(session_id);
                }
                MiddlewareAction::StopAndReturn
            }
            Ok(Err(_)) => {
                if let Some(session_id) = &ctx.session_id {
                    self.session_manager.clear_paused(session_id);
                }
                MiddlewareAction::StopAndReturn
            }
            Err(_) => {
                self.manager.pending.write().await.remove(&bp_id);
                if let Some(session_id) = &ctx.session_id {
                    self.session_manager.clear_paused(session_id);
                }
                tracing::warn!(id = %bp_id, "Breakpoint response timed out, dropping");
                MiddlewareAction::StopAndReturn
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::matcher::{Location, MatchMode};
    use crate::middleware::{
        HeaderMap, Middleware, MiddlewareAction, RequestContext, ResponseContext,
    };
    use std::sync::Arc;

    fn req(uri: &str, _body: &str) -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: uri.to_string(),
            headers: HeaderMap::new(),
            body: Bytes::from(_body.to_string()),
            host: "localhost".to_string(),
            ..Default::default()
        }
    }

    fn tunnel_req(uri: &str) -> RequestContext {
        let mut ctx = req(uri, "");
        ctx.protocol_context = Some(crate::core::forward::ProtocolContext {
            downstream: crate::core::forward::WireProtocol::Socks5,
            upstream: None,
            application: crate::core::forward::ApplicationProtocol::Binary,
            body_mode: crate::core::forward::BodyMode::Tunnel,
            scheme: "socks5".to_string(),
            connection_id: None,
            stream_id: None,
        });
        ctx
    }

    fn ws_frame_req(uri: &str, body: &str) -> RequestContext {
        let mut ctx = req(uri, body);
        ctx.method = "WS".to_string();
        ctx.protocol_context = Some(crate::core::forward::ProtocolContext {
            downstream: crate::core::forward::WireProtocol::WebSocket,
            upstream: None,
            application: crate::core::forward::ApplicationProtocol::Http,
            body_mode: crate::core::forward::BodyMode::Frames,
            scheme: "ws".to_string(),
            connection_id: None,
            stream_id: None,
        });
        ctx
    }

    fn grpc_message_req(uri: &str, body: &str) -> RequestContext {
        let mut ctx = req(uri, body);
        ctx.method = "POST".to_string();
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

    fn res(uri: &str, _body: &str) -> ResponseContext {
        ResponseContext {
            status: 200,
            headers: HeaderMap::new(),
            body: Bytes::from(_body.to_string()),
            request_uri: uri.to_string(),
            ..Default::default()
        }
    }

    /// Build a Request breakpoint rule matching a path regex.
    fn req_rule(path: &str, enabled: bool) -> BreakpointRule {
        BreakpointRule {
            id: uuid::Uuid::new_v4().to_string(),
            location: Location {
                path: if path.is_empty() || path == ".*" {
                    None
                } else {
                    Some(path.to_string())
                },
                mode: MatchMode::Regex,
                ..Default::default()
            },
            bp_type: BreakpointType::Request,
            tier: BreakpointTier::Body,
            enabled,
        }
    }

    /// Build a Response breakpoint rule matching a path regex.
    fn res_rule(path: &str, enabled: bool) -> BreakpointRule {
        BreakpointRule {
            id: uuid::Uuid::new_v4().to_string(),
            location: Location {
                path: if path.is_empty() || path == ".*" {
                    None
                } else {
                    Some(path.to_string())
                },
                mode: MatchMode::Regex,
                ..Default::default()
            },
            bp_type: BreakpointType::Response,
            tier: BreakpointTier::Body,
            enabled,
        }
    }

    /// Spawns a task that polls for the first pending breakpoint and resolves it.
    async fn auto_resolve(manager: Arc<BreakpointManager>, resolution: BreakpointResolution) {
        let m = manager.clone();
        tokio::spawn(async move {
            loop {
                let pending = m.pending.read().await;
                if let Some(id) = pending.keys().next().cloned() {
                    drop(pending);
                    let _ = m.resolve_breakpoint(&id, resolution).await;
                    return;
                }
                drop(pending);
                tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
            }
        });
    }

    fn session_manager_for_tests() -> crate::session::SharedSessionManager {
        Arc::new(crate::session::SessionManager::new(100))
    }

    #[tokio::test]
    async fn no_rules_returns_continue() {
        let mw = BreakpointMiddleware::new(
            Arc::new(BreakpointManager::new()),
            session_manager_for_tests(),
        );
        assert_eq!(
            mw.on_request(&mut req("/", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn disabled_rule_not_triggered_on_request() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/secret", false)).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_request(&mut req("/secret", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn non_matching_rule_passes_through() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"^/admin", true)).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_request(&mut req("/api/users", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn body_breakpoint_does_not_pause_raw_tunnel() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/tunnel", true)).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_request(&mut tunnel_req("/tunnel")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn incompatible_tier_records_diagnostic() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/tunnel", true)).await;
        let mw = BreakpointMiddleware::new(manager.clone(), session_manager_for_tests());

        assert_eq!(
            mw.on_request(&mut tunnel_req("/tunnel")).await,
            MiddlewareAction::Continue
        );

        let diagnostics = manager.list_diagnostics().await;
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].tier, BreakpointTier::Body);
        assert_eq!(diagnostics[0].body_mode.as_deref(), Some("tunnel"));
    }

    #[test]
    fn old_breakpoint_json_defaults_to_body_tier() {
        let rule: BreakpointRule = serde_json::from_value(serde_json::json!({
            "id": "old",
            "location": {},
            "bp_type": "Request",
            "enabled": true
        }))
        .unwrap();

        assert_eq!(rule.tier, BreakpointTier::Body);
    }

    #[tokio::test]
    async fn head_breakpoint_can_pause_raw_tunnel_metadata() {
        let manager = Arc::new(BreakpointManager::new());
        let mut rule = req_rule(r"/tunnel", true);
        rule.tier = BreakpointTier::Head;
        manager.add_rule(rule).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_request(&mut tunnel_req("/tunnel")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn tunnel_breakpoint_can_pause_raw_tunnel_metadata() {
        let manager = Arc::new(BreakpointManager::new());
        let mut rule = req_rule(r"/tunnel", true);
        rule.tier = BreakpointTier::Tunnel;
        manager.add_rule(rule).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager.clone(), session_manager_for_tests());

        assert_eq!(
            mw.on_request(&mut tunnel_req("/tunnel")).await,
            MiddlewareAction::Continue
        );
        assert!(manager.list_diagnostics().await.is_empty());
    }

    #[tokio::test]
    async fn head_breakpoint_can_pause_stream_message_metadata() {
        let manager = Arc::new(BreakpointManager::new());
        let mut rule = req_rule(r"/grpc.Service/Call", true);
        rule.tier = BreakpointTier::Head;
        manager.add_rule(rule).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager.clone(), session_manager_for_tests());

        assert_eq!(
            mw.on_request(&mut grpc_message_req("/grpc.Service/Call", "message"))
                .await,
            MiddlewareAction::Continue
        );
        assert!(manager.list_diagnostics().await.is_empty());
    }

    #[tokio::test]
    async fn frame_breakpoint_can_pause_frame_context() {
        let manager = Arc::new(BreakpointManager::new());
        let mut rule = req_rule(r"/socket", true);
        rule.tier = BreakpointTier::Frame;
        manager.add_rule(rule).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let sm = session_manager_for_tests();

        let resolution = manager
            .pause_frame(
                &sm,
                "sess-1",
                BreakpointContext::Request(Box::new(ws_frame_req("/socket", "hello"))),
            )
            .await;

        assert!(matches!(resolution, BreakpointResolution::Continue));
        assert!(manager.pending.read().await.is_empty());
    }

    #[tokio::test]
    async fn frame_breakpoint_drop_resolution_is_returned_to_frame_handler() {
        let manager = Arc::new(BreakpointManager::new());
        let mut rule = req_rule(r"/socket", true);
        rule.tier = BreakpointTier::Frame;
        manager.add_rule(rule).await;
        auto_resolve(manager.clone(), BreakpointResolution::Drop).await;
        let sm = session_manager_for_tests();

        let resolution = manager
            .pause_frame(
                &sm,
                "sess-drop",
                BreakpointContext::Request(Box::new(ws_frame_req("/socket", "drop me"))),
            )
            .await;

        assert!(matches!(resolution, BreakpointResolution::Drop));
        assert!(manager.pending.read().await.is_empty());
    }

    #[tokio::test]
    async fn frame_breakpoint_modify_resolution_preserves_modified_frame_context() {
        let manager = Arc::new(BreakpointManager::new());
        let mut rule = req_rule(r"/socket", true);
        rule.tier = BreakpointTier::Frame;
        manager.add_rule(rule).await;
        let resolver = manager.clone();
        tokio::spawn(async move {
            loop {
                let pending = resolver.pending.read().await;
                if let Some(id) = pending.keys().next().cloned() {
                    let context = pending.get(&id).unwrap().context.clone();
                    drop(pending);
                    if let BreakpointContext::Request(mut request) = context {
                        request.set_body_text("modified-frame");
                        let _ = resolver
                            .resolve_breakpoint(
                                &id,
                                BreakpointResolution::Modify(Box::new(BreakpointContext::Request(
                                    request,
                                ))),
                            )
                            .await;
                    }
                    return;
                }
                drop(pending);
                tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
            }
        });
        let sm = session_manager_for_tests();

        let resolution = manager
            .pause_frame(
                &sm,
                "sess-modify",
                BreakpointContext::Request(Box::new(ws_frame_req("/socket", "original-frame"))),
            )
            .await;

        match resolution {
            BreakpointResolution::Modify(context) => match *context {
                BreakpointContext::Request(request) => {
                    assert_eq!(request.body_text(), "modified-frame");
                }
                BreakpointContext::Response(_) => panic!("expected modified request frame"),
            },
            _ => panic!("expected modified frame resolution"),
        }
        assert!(manager.pending.read().await.is_empty());
    }

    #[tokio::test]
    async fn matching_request_rule_resolved_continue() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/secret", true)).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_request(&mut req("/secret", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn matching_request_rule_resolved_drop_returns_stop() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/drop-me", true)).await;
        auto_resolve(manager.clone(), BreakpointResolution::Drop).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_request(&mut req("/drop-me", "")).await,
            MiddlewareAction::StopAndReturn
        );
    }

    #[tokio::test]
    async fn matching_request_rule_resolved_modify_updates_context() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/modify", true)).await;
        let m = manager.clone();
        tokio::spawn(async move {
            loop {
                let pending = m.pending.read().await;
                if let Some(id) = pending.keys().next().cloned() {
                    let ctx = pending.get(&id).unwrap().context.clone();
                    drop(pending);
                    if let BreakpointContext::Request(mut rq) = ctx {
                        rq.set_body_text("modified-body");
                        let _ = m
                            .resolve_breakpoint(
                                &id,
                                BreakpointResolution::Modify(Box::new(BreakpointContext::Request(
                                    rq,
                                ))),
                            )
                            .await;
                    }
                    return;
                }
                drop(pending);
                tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
            }
        });
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        let mut ctx = req("/modify", "original");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert_eq!(ctx.body_text(), "modified-body");
    }

    #[tokio::test]
    async fn response_rule_does_not_fire_on_request() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(res_rule(r"/res-only", true)).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_request(&mut req("/res-only", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn matching_response_rule_resolved_continue() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(res_rule(r"/watch", true)).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_response(&mut res("/watch", "body")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn invalid_regex_path_does_not_panic() {
        let manager = Arc::new(BreakpointManager::new());
        // Invalid regex — Location::matches() returns false safely.
        manager.add_rule(req_rule("[invalid", true)).await;
        let mw = BreakpointMiddleware::new(manager, session_manager_for_tests());
        assert_eq!(
            mw.on_request(&mut req("/anything", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn resolve_nonexistent_breakpoint_returns_err() {
        let manager = BreakpointManager::new();
        assert!(
            manager
                .resolve_breakpoint("no-such-id", BreakpointResolution::Continue)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn pending_queue_contains_context_until_resolved() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/hold", true)).await;
        let mw = BreakpointMiddleware::new(manager.clone(), session_manager_for_tests());
        let mut ctx = req("/hold", "payload");

        let task = tokio::spawn(async move { mw.on_request(&mut ctx).await });

        let pending_id = loop {
            let pending = manager.pending.read().await;
            if let Some((id, bp)) = pending.iter().next() {
                assert!(matches!(bp.bp_type, BreakpointType::Request));
                match &bp.context {
                    BreakpointContext::Request(req) => {
                        assert_eq!(req.uri, "/hold");
                    }
                    _ => panic!("expected request breakpoint context"),
                }
                break id.clone();
            }
            drop(pending);
            tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
        };

        manager
            .resolve_breakpoint(&pending_id, BreakpointResolution::Continue)
            .await
            .unwrap();
        assert_eq!(task.await.unwrap(), MiddlewareAction::Continue);
        assert!(
            manager.pending.read().await.is_empty(),
            "resolved breakpoints must leave the pending queue"
        );
    }
}
