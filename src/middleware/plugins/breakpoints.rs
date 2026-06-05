use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use async_trait::async_trait;
use bytes::Bytes;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointRule {
    #[serde(default)]
    pub id: String,
    /// Full Location-based matching (host, path, port, protocol, query, methods, mode).
    /// Leave all fields at default to match every request/response.
    pub location: Location,
    pub bp_type: BreakpointType,
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
}

impl BreakpointManager {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(RwLock::new(Vec::new())),
            pending: Arc::new(RwLock::new(HashMap::new())),
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
}

impl Default for BreakpointManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BreakpointMiddleware {
    pub manager: Arc<BreakpointManager>,
}

impl BreakpointMiddleware {
    pub fn new(manager: Arc<BreakpointManager>) -> Self {
        Self { manager }
    }

    /// Returns the first matching enabled rule of the given type, releasing all
    /// locks before returning so no lock is held during the async breakpoint wait.
    async fn first_match(
        &self,
        bp_type_filter: impl Fn(&BreakpointType) -> bool,
        target: &MatchTarget,
    ) -> Option<BreakpointRule> {
        let rules = self.manager.rules.read().await;
        rules
            .iter()
            .filter(|r| r.enabled && bp_type_filter(&r.bp_type))
            .find(|r| r.location.matches(target))
            .cloned()
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
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

    #[tokio::test]
    async fn no_rules_returns_continue() {
        let mw = BreakpointMiddleware::new(Arc::new(BreakpointManager::new()));
        assert_eq!(
            mw.on_request(&mut req("/", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn disabled_rule_not_triggered_on_request() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/secret", false)).await;
        let mw = BreakpointMiddleware::new(manager);
        assert_eq!(
            mw.on_request(&mut req("/secret", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn non_matching_rule_passes_through() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"^/admin", true)).await;
        let mw = BreakpointMiddleware::new(manager);
        assert_eq!(
            mw.on_request(&mut req("/api/users", "")).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn matching_request_rule_resolved_continue() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/secret", true)).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager);
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
        let mw = BreakpointMiddleware::new(manager);
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
        let mw = BreakpointMiddleware::new(manager);
        let mut ctx = req("/modify", "original");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert_eq!(ctx.body_text(), "modified-body");
    }

    #[tokio::test]
    async fn response_rule_does_not_fire_on_request() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(res_rule(r"/res-only", true)).await;
        let mw = BreakpointMiddleware::new(manager);
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
        let mw = BreakpointMiddleware::new(manager);
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
        let mw = BreakpointMiddleware::new(manager);
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
        let mw = BreakpointMiddleware::new(manager.clone());
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

        let (tx, rx) = oneshot::channel();
        let bp_id = Uuid::new_v4().to_string();
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
            Ok(Ok(BreakpointResolution::Continue)) => MiddlewareAction::Continue,
            Ok(Ok(BreakpointResolution::Modify(bc))) => {
                if let BreakpointContext::Request(new_ctx) = *bc {
                    *ctx = *new_ctx;
                    MiddlewareAction::Continue
                } else {
                    MiddlewareAction::StopAndReturn
                }
            }
            Ok(Ok(BreakpointResolution::Drop)) => MiddlewareAction::StopAndReturn,
            Ok(Err(_)) => MiddlewareAction::StopAndReturn,
            Err(_) => {
                self.manager.pending.write().await.remove(&bp_id);
                tracing::warn!(id = %bp_id, "Breakpoint request timed out, dropping");
                let mut headers = crate::middleware::HeaderMap::new();
                headers.insert("content-type".to_string(), "text/plain".to_string());
                ctx.mock_response = Some(crate::middleware::InterceptedResponse {
                    status: 504,
                    headers,
                    body: Bytes::from_static(b"Breakpoint timed out"),
                    tags: Vec::new(),
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

        let (tx, rx) = oneshot::channel();
        let bp_id = Uuid::new_v4().to_string();
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
            Ok(Ok(BreakpointResolution::Continue)) => MiddlewareAction::Continue,
            Ok(Ok(BreakpointResolution::Modify(bc))) => {
                if let BreakpointContext::Response(new_ctx) = *bc {
                    *ctx = *new_ctx;
                    MiddlewareAction::Continue
                } else {
                    MiddlewareAction::StopAndReturn
                }
            }
            Ok(Ok(BreakpointResolution::Drop)) => MiddlewareAction::StopAndReturn,
            Ok(Err(_)) => MiddlewareAction::StopAndReturn,
            Err(_) => {
                self.manager.pending.write().await.remove(&bp_id);
                tracing::warn!(id = %bp_id, "Breakpoint response timed out, dropping");
                MiddlewareAction::StopAndReturn
            }
        }
    }
}
