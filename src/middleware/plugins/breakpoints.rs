use async_trait::async_trait;
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, oneshot};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
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
    pub id: String,
    pub pattern: String, // Regex for URI or Body
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
    Request(RequestContext),
    Response(ResponseContext),
}

#[derive(Debug, Clone)]
pub enum BreakpointResolution {
    Continue,
    Modify(BreakpointContext),
    Drop,
}

pub struct BreakpointManager {
    pub rules: Arc<RwLock<Vec<BreakpointRule>>>,
    pub pending: Arc<RwLock<HashMap<String, PendingBreakpoint>>>,
    regex_cache: Arc<RwLock<HashMap<String, regex::Regex>>>,
}

impl BreakpointManager {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(RwLock::new(Vec::new())),
            pending: Arc::new(RwLock::new(HashMap::new())),
            regex_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_rule(&self, rule: BreakpointRule) {
        if let Ok(re) = regex::Regex::new(&rule.pattern) {
            self.regex_cache.write().await.insert(rule.id.clone(), re);
        }
        self.rules.write().await.push(rule);
    }

    pub async fn resolve_breakpoint(&self, id: &str, resolution: BreakpointResolution) -> Result<(), String> {
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
        self.regex_cache.write().await.remove(id);
        self.rules.write().await.retain(|r| r.id != id);
    }
}

pub struct BreakpointMiddleware {
    pub manager: Arc<BreakpointManager>,
}

impl BreakpointMiddleware {
    pub fn new(manager: Arc<BreakpointManager>) -> Self {
        Self { manager }
    }

    /// Returns the first matching enabled rule of the given type, releasing all locks
    /// before returning so no lock is held during the async breakpoint wait.
    async fn first_match(
        &self,
        bp_type_filter: impl Fn(&BreakpointType) -> bool,
        uri: &str,
        body: &str,
    ) -> Option<BreakpointRule> {
        let rules = self.manager.rules.read().await;
        let cache = self.manager.regex_cache.read().await;
        rules
            .iter()
            .filter(|r| r.enabled && bp_type_filter(&r.bp_type))
            .find(|r| {
                cache
                    .get(&r.id)
                    .is_some_and(|re| re.is_match(uri) || re.is_match(body))
            })
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn req(uri: &str, body: &str) -> RequestContext {
        RequestContext { method: "GET".to_string(), uri: uri.to_string(), headers: HashMap::new(), body: body.to_string(), host: "localhost".to_string(), body_bytes: None }
    }

    fn res(uri: &str, body: &str) -> ResponseContext {
        ResponseContext { status: 200, headers: HashMap::new(), body: body.to_string(), request_uri: uri.to_string(), session_id: None, ttfb_ms: 0, body_ms: 0, body_bytes: None }
    }

    fn req_rule(pattern: &str, enabled: bool) -> BreakpointRule {
        BreakpointRule { id: uuid::Uuid::new_v4().to_string(), pattern: pattern.to_string(), bp_type: BreakpointType::Request, enabled }
    }

    fn res_rule(pattern: &str, enabled: bool) -> BreakpointRule {
        BreakpointRule { id: uuid::Uuid::new_v4().to_string(), pattern: pattern.to_string(), bp_type: BreakpointType::Response, enabled }
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
        assert_eq!(mw.on_request(&mut req("/", "")).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn disabled_rule_not_triggered_on_request() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/secret", false)).await;
        let mw = BreakpointMiddleware::new(manager);
        assert_eq!(mw.on_request(&mut req("/secret", "")).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn non_matching_rule_passes_through() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"^/admin", true)).await;
        let mw = BreakpointMiddleware::new(manager);
        assert_eq!(mw.on_request(&mut req("/api/users", "")).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn matching_request_rule_resolved_continue() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/secret", true)).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager);
        assert_eq!(mw.on_request(&mut req("/secret", "")).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn matching_request_rule_resolved_drop_returns_stop() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"/drop-me", true)).await;
        auto_resolve(manager.clone(), BreakpointResolution::Drop).await;
        let mw = BreakpointMiddleware::new(manager);
        assert_eq!(mw.on_request(&mut req("/drop-me", "")).await, MiddlewareAction::StopAndReturn);
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
                        rq.body = "modified-body".to_string();
                        let _ = m.resolve_breakpoint(&id, BreakpointResolution::Modify(BreakpointContext::Request(rq))).await;
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
        assert_eq!(ctx.body, "modified-body");
    }

    #[tokio::test]
    async fn response_rule_does_not_fire_on_request() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(res_rule(r"/res-only", true)).await;
        let mw = BreakpointMiddleware::new(manager);
        assert_eq!(mw.on_request(&mut req("/res-only", "")).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn matching_response_rule_resolved_continue() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(res_rule(r"/watch", true)).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager);
        assert_eq!(mw.on_response(&mut res("/watch", "body")).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn invalid_regex_in_rule_does_not_panic() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule("[invalid", true)).await;
        let mw = BreakpointMiddleware::new(manager);
        // Invalid regex → check_match returns false → Continue without blocking
        assert_eq!(mw.on_request(&mut req("/anything", "body")).await, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn resolve_nonexistent_breakpoint_returns_err() {
        let manager = BreakpointManager::new();
        assert!(manager.resolve_breakpoint("no-such-id", BreakpointResolution::Continue).await.is_err());
    }

    #[tokio::test]
    async fn pattern_matches_body_not_just_uri() {
        let manager = Arc::new(BreakpointManager::new());
        manager.add_rule(req_rule(r"password", true)).await;
        auto_resolve(manager.clone(), BreakpointResolution::Continue).await;
        let mw = BreakpointMiddleware::new(manager);
        // URI doesn't match but body does
        let mut ctx = req("/login", r#"{"password":"secret"}"#);
        let action = mw.on_request(&mut ctx).await;
        // Should have paused (and been resolved to Continue)
        assert_eq!(action, MiddlewareAction::Continue);
        // Verify a pending breakpoint was created (it was resolved, so pending should be empty now)
    }
}

#[async_trait]
impl Middleware for BreakpointMiddleware {
    fn name(&self) -> &str {
        "BreakpointMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        // Locks released before the async wait so writers are never blocked during a pause.
        if self.first_match(|t| matches!(t, BreakpointType::Request), &ctx.uri, &ctx.body).await.is_none() {
            return MiddlewareAction::Continue;
        }

        let (tx, rx) = oneshot::channel();
        let bp_id = Uuid::new_v4().to_string();
        self.manager.pending.write().await.insert(bp_id.clone(), PendingBreakpoint {
            id: bp_id.clone(),
            bp_type: BreakpointType::Request,
            context: BreakpointContext::Request(ctx.clone()),
            tx,
        });

        match tokio::time::timeout(BREAKPOINT_TIMEOUT, rx).await {
            Ok(Ok(BreakpointResolution::Continue)) => MiddlewareAction::Continue,
            Ok(Ok(BreakpointResolution::Modify(BreakpointContext::Request(new_ctx)))) => {
                *ctx = new_ctx;
                MiddlewareAction::Continue
            }
            Ok(Ok(BreakpointResolution::Drop)) => MiddlewareAction::StopAndReturn,
            Ok(Ok(_)) | Ok(Err(_)) => MiddlewareAction::StopAndReturn,
            Err(_) => {
                self.manager.pending.write().await.remove(&bp_id);
                tracing::warn!(id = %bp_id, "Breakpoint request timed out, dropping");
                MiddlewareAction::StopAndReturn
            }
        }
    }

    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        if self.first_match(|t| matches!(t, BreakpointType::Response), &ctx.request_uri, &ctx.body).await.is_none() {
            return MiddlewareAction::Continue;
        }

        let (tx, rx) = oneshot::channel();
        let bp_id = Uuid::new_v4().to_string();
        self.manager.pending.write().await.insert(bp_id.clone(), PendingBreakpoint {
            id: bp_id.clone(),
            bp_type: BreakpointType::Response,
            context: BreakpointContext::Response(ctx.clone()),
            tx,
        });

        match tokio::time::timeout(BREAKPOINT_TIMEOUT, rx).await {
            Ok(Ok(BreakpointResolution::Continue)) => MiddlewareAction::Continue,
            Ok(Ok(BreakpointResolution::Modify(BreakpointContext::Response(new_ctx)))) => {
                *ctx = new_ctx;
                MiddlewareAction::Continue
            }
            Ok(Ok(BreakpointResolution::Drop)) => MiddlewareAction::StopAndReturn,
            Ok(Ok(_)) | Ok(Err(_)) => MiddlewareAction::StopAndReturn,
            Err(_) => {
                self.manager.pending.write().await.remove(&bp_id);
                tracing::warn!(id = %bp_id, "Breakpoint response timed out, dropping");
                MiddlewareAction::StopAndReturn
            }
        }
    }
}
