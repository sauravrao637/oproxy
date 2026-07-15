use std::sync::Arc;

use serde::Serialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::api::{SessionListFilter, SessionListOptions};

use super::assistant_redaction::{redact_uri, redact_value};
use super::workspace::{SessionsViewState, SortDirection};

#[derive(Debug, Clone, Serialize)]
pub(super) struct AssistantContext {
    pub(super) context_priority: AssistantContextPriority,
    pub(super) workspace: AssistantWorkspaceContext,
    pub(super) visible_sessions: AssistantVisibleSessionsContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) primary_subject: Option<AssistantPrimarySubject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) selected_session: Option<AssistantSessionSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) client_hints: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AssistantContextPriority {
    pub(super) primary: Option<String>,
    pub(super) secondary: Vec<String>,
    pub(super) rule: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AssistantPrimarySubject {
    pub(super) kind: String,
    pub(super) session_id: String,
    pub(super) instruction: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AssistantWorkspaceContext {
    pub(super) active_surface: String,
    pub(super) sessions_view: Value,
    pub(super) feature_views: Value,
    pub(super) assistant_context: Value,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AssistantVisibleSessionsContext {
    pub(super) total: usize,
    pub(super) filtered_total: usize,
    pub(super) limit: Option<usize>,
    pub(super) facets: Value,
    pub(super) selected_session_id: Option<String>,
    pub(super) selected_session_in_visible_results: bool,
    pub(super) sessions: Vec<AssistantSessionSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AssistantSessionSummary {
    pub(super) id: String,
    pub(super) timestamp: chrono::DateTime<chrono::Utc>,
    pub(super) method: String,
    pub(super) uri: String,
    pub(super) host: String,
    pub(super) status: Option<u16>,
    pub(super) source: crate::session::SessionSource,
    pub(super) tags: Vec<String>,
    pub(super) note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) request_flow: Vec<Value>,
}

pub(super) async fn build_assistant_context(
    state: &Arc<AppState>,
    client_context: Option<&Value>,
) -> AssistantContext {
    let workspace = state.workspace.read().await.clone();
    let attached_session_id = attached_session_id(client_context);
    let ignore_selected_session =
        attached_session_id.is_some() || has_explicit_attachment(client_context);
    let visible = state
        .api_handler
        .list_sessions(SessionListOptions {
            limit: Some(20),
            filter: session_filter_from_workspace_context(&workspace.sessions_view),
            ..SessionListOptions::default()
        })
        .await;
    let selected_session_id = attached_session_id.clone().or_else(|| {
        (!ignore_selected_session)
            .then(|| workspace.sessions_view.selected_session_id.clone())
            .flatten()
    });
    let selected_session_in_visible_results = selected_session_id
        .as_deref()
        .is_some_and(|id| visible.sessions.iter().any(|session| session.id == id));
    let selected_session = match selected_session_id.as_deref() {
        Some(id) => state
            .api_handler
            .get_session_details(id)
            .await
            .map(|detail| assistant_session_summary(detail.exchange)),
        None => None,
    };
    let primary_subject = attached_session_id
        .as_ref()
        .map(|id| primary_subject_for_attached_session(id));
    let sessions = visible
        .sessions
        .into_iter()
        .map(assistant_session_summary)
        .collect();

    AssistantContext {
        context_priority: context_priority(attached_session_id.as_deref()),
        workspace: AssistantWorkspaceContext {
            active_surface: serde_json::to_value(&workspace.active_surface)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "sessions".to_string()),
            sessions_view: redact_value(&sessions_view_context_value(
                &workspace.sessions_view,
                ignore_selected_session,
            )),
            feature_views: redact_value(&json!(workspace.feature_views)),
            assistant_context: redact_value(&json!(workspace.assistant_context)),
        },
        visible_sessions: AssistantVisibleSessionsContext {
            total: visible.total,
            filtered_total: visible.filtered_total,
            limit: visible.limit,
            facets: redact_value(&json!(visible.facets)),
            selected_session_id,
            selected_session_in_visible_results,
            sessions,
        },
        primary_subject,
        selected_session,
        client_hints: client_context.map(redact_value),
    }
}

fn primary_subject_for_attached_session(id: &str) -> AssistantPrimarySubject {
    AssistantPrimarySubject {
        kind: "attached_session".to_string(),
        session_id: id.to_string(),
        instruction: "This request was intentionally attached from Ask Assistant. Treat selected_session as the primary subject for the user's next message. Use secondary context only to fill gaps, answer broader follow-ups, or when the primary is insufficient.".to_string(),
    }
}

fn context_priority(attached_session_id: Option<&str>) -> AssistantContextPriority {
    AssistantContextPriority {
        primary: attached_session_id
            .map(|id| format!("primary_subject selected_session for attached session {id}")),
        secondary: vec![
            "workspace.sessions_view".to_string(),
            "visible_sessions".to_string(),
            "workspace.feature_views".to_string(),
            "client_hints.ui_state".to_string(),
        ],
        rule: "Use primary context first. Treat all workspace, visible Sessions, selected UI state, and client hints as secondary. Use secondary only when primary is absent or not sufficient to answer.".to_string(),
    }
}

fn has_explicit_attachment(client_context: Option<&Value>) -> bool {
    client_context
        .and_then(|context| context.get("attachment"))
        .is_some_and(|attachment| !attachment.is_null())
}

fn attached_session_id(client_context: Option<&Value>) -> Option<String> {
    let attachment = client_context?.get("attachment")?;
    if attachment.get("kind").and_then(Value::as_str) != Some("session") {
        return None;
    }
    let id = attachment.get("id").and_then(Value::as_str)?.trim();
    if id.is_empty() || id.len() > 256 {
        return None;
    }
    Some(id.to_string())
}

fn sessions_view_context_value(view: &SessionsViewState, ignore_selected_session: bool) -> Value {
    let mut value = json!(view);
    if ignore_selected_session && let Some(object) = value.as_object_mut() {
        object.insert("selected_session_id".to_string(), Value::Null);
        object.insert("selected_session_ignored".to_string(), json!(true));
    }
    value
}

fn assistant_session_summary(exchange: crate::session::Exchange) -> AssistantSessionSummary {
    AssistantSessionSummary {
        id: exchange.id,
        timestamp: exchange.timestamp,
        method: exchange.request.method,
        uri: redact_uri(&exchange.request.uri),
        host: exchange.request.host,
        status: exchange.response.as_ref().map(|response| response.status),
        source: exchange.source,
        tags: exchange.tags,
        note: exchange.note,
        request_flow: exchange
            .flow
            .into_iter()
            .filter_map(|event| serde_json::to_value(event).ok())
            .map(|event| redact_value(&event))
            .collect(),
    }
}

fn session_filter_from_workspace_context(view: &SessionsViewState) -> SessionListFilter {
    SessionListFilter {
        query: view.query.clone(),
        regex: view.regex,
        methods: Some(view.methods.clone()),
        status_buckets: Some(view.status_buckets.clone()),
        host_focus: view.host_focus.clone(),
        host_filter: view.host_filter.clone(),
        sort: crate::api::SessionSort {
            key: view.sort.key.clone(),
            dir: match view.sort.dir {
                SortDirection::Asc => crate::api::SessionSortDirection::Asc,
                SortDirection::Desc => crate::api::SessionSortDirection::Desc,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::workspace::{SessionsViewMode, WorkspaceSort};

    #[test]
    fn workspace_context_filter_preserves_backend_view_semantics() {
        let view = SessionsViewState {
            query: "host:api.test.com".to_string(),
            regex: true,
            methods: vec!["POST".to_string()],
            status_buckets: vec!["5".to_string()],
            host_focus: vec!["api.test.com".to_string()],
            host_filter: Some("api.test.com".to_string()),
            sort: WorkspaceSort {
                key: "status".to_string(),
                dir: SortDirection::Desc,
            },
            view_mode: SessionsViewMode::Sequence,
            selected_session_id: Some("s1".to_string()),
            app_filter: Default::default(),
            wire_filter: Default::default(),
        };

        let filter = session_filter_from_workspace_context(&view);

        assert_eq!(filter.query, "host:api.test.com");
        assert!(filter.regex);
        assert_eq!(filter.methods, Some(vec!["POST".to_string()]));
        assert_eq!(filter.status_buckets, Some(vec!["5".to_string()]));
        assert_eq!(filter.host_focus, vec!["api.test.com".to_string()]);
        assert_eq!(filter.host_filter.as_deref(), Some("api.test.com"));
        assert_eq!(filter.sort.key, "status");
        assert!(matches!(
            filter.sort.dir,
            crate::api::SessionSortDirection::Desc
        ));
    }

    #[test]
    fn explicit_client_attachment_suppresses_selected_session_in_view_context() {
        let view = SessionsViewState {
            selected_session_id: Some("selected-request".to_string()),
            ..SessionsViewState::default()
        };

        let value = sessions_view_context_value(&view, true);

        assert_eq!(value["selected_session_id"], Value::Null);
        assert_eq!(value["selected_session_ignored"], true);
    }

    #[test]
    fn null_client_attachment_does_not_suppress_selected_session() {
        assert!(!has_explicit_attachment(Some(
            &json!({ "attachment": null })
        )));
        assert!(has_explicit_attachment(Some(&json!({
            "attachment": { "kind": "session", "id": "right-clicked-request" }
        }))));
    }

    #[test]
    fn session_attachment_supplies_authoritative_session_id() {
        assert_eq!(
            attached_session_id(Some(&json!({
                "attachment": { "kind": "session", "id": "right-clicked-request" }
            }))),
            Some("right-clicked-request".to_string())
        );
        assert_eq!(
            attached_session_id(Some(&json!({
                "attachment": { "kind": "header", "id": "not-a-session" }
            }))),
            None
        );
    }

    #[test]
    fn session_attachment_builds_primary_subject_instruction() {
        let subject = primary_subject_for_attached_session("right-clicked-request");

        assert_eq!(subject.kind, "attached_session");
        assert_eq!(subject.session_id, "right-clicked-request");
        assert!(subject.instruction.contains("primary subject"));
        assert!(
            subject
                .instruction
                .contains("secondary context only to fill gaps")
        );
    }

    #[test]
    fn context_priority_marks_ui_state_as_secondary() {
        let with_primary = context_priority(Some("right-clicked-request"));

        assert_eq!(
            with_primary.primary.as_deref(),
            Some("primary_subject selected_session for attached session right-clicked-request")
        );
        assert!(
            with_primary
                .secondary
                .contains(&"workspace.sessions_view".to_string())
        );
        assert!(
            with_primary
                .secondary
                .contains(&"visible_sessions".to_string())
        );
        assert!(
            with_primary
                .secondary
                .contains(&"client_hints.ui_state".to_string())
        );
        assert!(with_primary.rule.contains("Use primary context first"));
        assert!(
            with_primary
                .rule
                .contains("secondary only when primary is absent or not sufficient")
        );

        let without_primary = context_priority(None);
        assert!(without_primary.primary.is_none());
    }
}
