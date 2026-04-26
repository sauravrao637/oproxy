use crate::core::playback::PlaybackEngine;
use crate::middleware::plugins::breakpoints::{
    BreakpointContext, BreakpointManager, BreakpointResolution, BreakpointRule, BreakpointType,
};
use crate::middleware::plugins::rewrite::{RewriteMiddleware, RewriteRule};
use crate::session::Exchange;
use crate::session::SharedSessionManager;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct SessionFileRequest {
    pub path: String,
}

#[derive(Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<Exchange>,
    pub total: usize,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Serialize)]
pub struct SessionDetailResponse {
    pub exchange: Exchange,
}

#[derive(Serialize)]
pub struct PendingBreakpointInfo {
    pub id: String,
    pub bp_type: BreakpointType,
    pub context: BreakpointContext,
}

pub struct ApiHandler {
    session_manager: SharedSessionManager,
    rewrite_middleware: Arc<RewriteMiddleware>,
    breakpoint_manager: Arc<BreakpointManager>,
    playback_engine: PlaybackEngine,
}

impl ApiHandler {
    pub fn new(
        session_manager: SharedSessionManager,
        rewrite_middleware: Arc<RewriteMiddleware>,
        breakpoint_manager: Arc<BreakpointManager>,
    ) -> Self {
        let playback_engine = PlaybackEngine::new(session_manager.clone());
        Self {
            session_manager,
            rewrite_middleware,
            breakpoint_manager,
            playback_engine,
        }
    }

    pub async fn save_session(&self, path: String) -> Result<(), String> {
        self.session_manager
            .save_to_file(path)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn load_session(&self, path: String) -> Result<(), String> {
        self.session_manager
            .load_from_file(path)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn start_playback(&self) {
        let sessions = self.session_manager.get_all_sessions();
        self.playback_engine.replay(sessions).await;
    }

    /// List sessions with optional timestamp filter and pagination.
    /// `since` — return only sessions newer than this timestamp (incremental poll).
    /// `limit` / `offset` — page through results (applied after `since` filtering).
    pub async fn list_sessions(
        &self,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> SessionListResponse {
        let all = self.session_manager.get_all_sessions();
        let total = all.len();
        let mut sessions: Vec<_> = if let Some(since_dt) = since {
            all.into_iter()
                .filter(|e| e.timestamp > since_dt || e.response.is_none())
                .collect()
        } else {
            all
        };
        sessions.sort_unstable_by(|a, b| b.timestamp.cmp(&a.timestamp));

        let off = offset.unwrap_or(0);
        let paged: Vec<_> = if let Some(lim) = limit {
            sessions.into_iter().skip(off).take(lim).collect()
        } else {
            sessions.into_iter().skip(off).collect()
        };

        SessionListResponse {
            sessions: paged,
            total,
            limit,
            offset,
        }
    }

    pub async fn get_session_details(&self, id: &str) -> Option<SessionDetailResponse> {
        self.session_manager
            .get_session(id)
            .map(|exchange| SessionDetailResponse { exchange })
    }

    pub async fn clear_sessions(&self) {
        self.session_manager.clear_sessions();
    }

    pub async fn get_all_metrics(&self) -> Vec<crate::session::InspectionMetrics> {
        self.session_manager
            .get_all_sessions()
            .into_iter()
            .filter_map(|e| e.metrics)
            .collect()
    }

    pub async fn add_rewrite_rule(&self, rule: RewriteRule) {
        let mut rules = self.rewrite_middleware.rules.write().await;
        rules.push(rule);
    }

    pub async fn list_rewrite_rules(&self) -> Vec<RewriteRule> {
        self.rewrite_middleware.rules.read().await.clone()
    }

    pub async fn delete_rewrite_rule(&self, index: usize) {
        let mut rules = self.rewrite_middleware.rules.write().await;
        if index < rules.len() {
            rules.remove(index);
        }
    }

    pub async fn list_breakpoints(&self) -> Vec<(String, BreakpointContext)> {
        let pending = self.breakpoint_manager.pending.read().await;
        pending
            .iter()
            .map(|(id, bp)| (id.clone(), bp.context.clone()))
            .collect()
    }

    pub async fn resolve_breakpoint(
        &self,
        id: String,
        resolution: BreakpointResolution,
    ) -> Result<(), String> {
        self.breakpoint_manager
            .resolve_breakpoint(&id, resolution)
            .await
    }

    pub async fn list_breakpoint_rules(&self) -> Vec<BreakpointRule> {
        self.breakpoint_manager.list_rules().await
    }

    pub async fn add_breakpoint_rule(&self, rule: BreakpointRule) {
        self.breakpoint_manager.add_rule(rule).await;
    }

    pub async fn delete_breakpoint_rule(&self, id: &str) {
        self.breakpoint_manager.delete_rule(id).await;
    }

    pub async fn list_pending(&self) -> Vec<PendingBreakpointInfo> {
        let pending = self.breakpoint_manager.pending.read().await;
        pending
            .values()
            .map(|bp| PendingBreakpointInfo {
                id: bp.id.clone(),
                bp_type: bp.bp_type.clone(),
                context: bp.context.clone(),
            })
            .collect()
    }
}

/// Pretty-print a body string based on its content-type.
/// Returns the original string unchanged if it cannot be pretty-printed.
pub fn pretty_body(body: &str, content_type: &str) -> String {
    if content_type.contains("application/json") || content_type.contains("/json") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            if let Ok(s) = serde_json::to_string_pretty(&v) {
                return s;
            }
        }
    }
    body.to_string()
}
