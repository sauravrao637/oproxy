use axum::{Json, extract::State, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::AppState;

use super::assistant_actions::{
    AssistantAction, deterministic_action_from_messages, execute_action_payload,
};
use super::assistant_context::build_assistant_context;
use super::assistant_contracts::grouped_tool_contract_info;
use super::assistant_prompt::build_initial_messages;
use super::assistant_provider::{AssistantProviderConfig, OpenAiCompatibleProviderClient};
use super::assistant_redaction::redact_value;
use super::assistant_registry::{openai_tool_specs, workspace_tool_name_for_action};
use super::assistant_tools::{ToolOutcome, execute_assistant_tool, tool_summary};
use super::workspace::{
    WorkspaceActionRequest, apply_workspace_action, deterministic_workspace_action_from_text,
};

const ASSISTANT_ACTION_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_TOOL_LOOPS: usize = 4;
const MAX_PENDING_ACTIONS: usize = 50;

#[derive(Default)]
pub(crate) struct AssistantState {
    pending_actions: RwLock<HashMap<String, PendingAssistantAction>>,
}

pub(crate) type SharedAssistantState = Arc<AssistantState>;

pub(crate) fn new_assistant_state() -> SharedAssistantState {
    Arc::new(AssistantState::default())
}

#[derive(Clone)]
struct PendingAssistantAction {
    action: AssistantAction,
    confirmation_token: String,
    expires_at: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AssistantMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AssistantChatRequest {
    pub messages: Vec<AssistantMessage>,
    pub provider: AssistantProviderConfig,
    pub api_key: String,
    #[serde(default)]
    pub client_context: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AssistantChatResponse {
    pub message: String,
    pub tool_events: Vec<AssistantToolEvent>,
    pub proposed_actions: Vec<AssistantAction>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AssistantToolEvent {
    pub name: String,
    pub category: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExecuteAssistantActionRequest {
    pub action_id: String,
    pub confirmation_token: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ExecuteAssistantActionResponse {
    pub ok: bool,
    pub result: Value,
    pub refreshed_resources: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelAssistantActionResponse {
    pub ok: bool,
}

pub(super) async fn get_assistant_tools() -> impl IntoResponse {
    Json(grouped_tool_contract_info())
}

pub(super) async fn chat_assistant(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AssistantChatRequest>,
) -> impl IntoResponse {
    match run_assistant_chat(state, req).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

pub(super) async fn execute_assistant_action(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecuteAssistantActionRequest>,
) -> impl IntoResponse {
    let pending = {
        let mut pending = state.assistant.pending_actions.write().await;
        prune_expired_actions(&mut pending);
        let Some(pending_action) = pending.get(&req.action_id).cloned() else {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({ "error": "assistant action not found or expired" })),
            )
                .into_response();
        };
        if !constant_time_eq(&pending_action.confirmation_token, &req.confirmation_token) {
            return (
                axum::http::StatusCode::FORBIDDEN,
                Json(json!({ "error": "assistant action confirmation failed" })),
            )
                .into_response();
        }
        pending.remove(&req.action_id);
        pending_action
    };

    match execute_action_payload(&state, &pending.action).await {
        Ok((result, refreshed_resources)) => Json(ExecuteAssistantActionResponse {
            ok: true,
            result,
            refreshed_resources,
        })
        .into_response(),
        Err(e) => {
            let status = if e.starts_with("assistant action precondition failed") {
                axum::http::StatusCode::CONFLICT
            } else {
                axum::http::StatusCode::UNPROCESSABLE_ENTITY
            };
            (status, Json(json!({ "error": e }))).into_response()
        }
    }
}

pub(super) async fn cancel_assistant_action(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecuteAssistantActionRequest>,
) -> impl IntoResponse {
    let mut pending = state.assistant.pending_actions.write().await;
    prune_expired_actions(&mut pending);
    let Some(pending_action) = pending.get(&req.action_id) else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({ "error": "assistant action not found or expired" })),
        )
            .into_response();
    };
    if !constant_time_eq(&pending_action.confirmation_token, &req.confirmation_token) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({ "error": "assistant action confirmation failed" })),
        )
            .into_response();
    }
    pending.remove(&req.action_id);

    Json(CancelAssistantActionResponse { ok: true }).into_response()
}

async fn run_assistant_chat(
    state: Arc<AppState>,
    req: AssistantChatRequest,
) -> Result<AssistantChatResponse, String> {
    if let Some(workspace_request) = deterministic_workspace_action_from_messages(&req.messages) {
        let result = apply_workspace_action(&state, workspace_request).await?;
        return Ok(AssistantChatResponse {
            message: format!(
                "{}. I updated the UI so you can inspect it there.",
                result.message
            ),
            tool_events: vec![AssistantToolEvent {
                name: workspace_tool_name_for_action(&result.action_type)
                    .unwrap_or_else(|| result.action_type.clone()),
                category: "ui".to_string(),
                status: "ok".to_string(),
                summary: Some(result.message),
            }],
            proposed_actions: vec![],
        });
    }

    if let Some(mut action) = deterministic_action_from_messages(&req.messages)? {
        register_pending_action(&state, &mut action).await;
        return Ok(AssistantChatResponse {
            message: "I prepared the map-remote rule below. Review it and click Apply when you want me to run it.".to_string(),
            tool_events: vec![AssistantToolEvent {
                name: "propose_map_remote".to_string(),
                category: action.risk_category().to_string(),
                status: "needs_confirmation".to_string(),
                summary: Some(action.summary.clone()),
            }],
            proposed_actions: vec![action],
        });
    }

    let provider_client =
        OpenAiCompatibleProviderClient::new(req.provider.clone(), req.api_key.clone())?;

    let assistant_context = build_assistant_context(&state, req.client_context.as_ref()).await;
    let mut tool_events = Vec::new();
    let mut proposed_actions = Vec::new();
    let mut messages = build_initial_messages(&req, &assistant_context);
    let tools = openai_tool_specs();

    for _ in 0..MAX_TOOL_LOOPS {
        let provider_message = provider_client.chat_completion(&messages, &tools).await?;
        let assistant_content = provider_message.content;
        let tool_calls = provider_message.tool_calls;

        if tool_calls.is_empty() {
            return Ok(AssistantChatResponse {
                message: assistant_content,
                tool_events,
                proposed_actions,
            });
        }

        messages.push(Value::Object(provider_message.raw_message));
        for call in tool_calls {
            let call_id = call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("tool_call")
                .to_string();
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tool_result = match parse_tool_arguments(&call) {
                Ok(args) => execute_assistant_tool(&state, &name, args).await,
                Err(e) => Err(e),
            };
            match tool_result {
                Ok(ToolOutcome::Read(result)) => {
                    tool_events.push(AssistantToolEvent {
                        name: name.clone(),
                        category: "read".to_string(),
                        status: "ok".to_string(),
                        summary: Some(tool_summary(&result)),
                    });
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": result.to_string(),
                    }));
                }
                Ok(ToolOutcome::Workspace(result)) => {
                    let content = serde_json::to_value(&result)
                        .unwrap_or_else(|_| json!({ "ok": true, "message": result.message }));
                    tool_events.push(AssistantToolEvent {
                        name: name.clone(),
                        category: "ui".to_string(),
                        status: "ok".to_string(),
                        summary: Some(result.message.clone()),
                    });
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": content.to_string(),
                    }));
                }
                Ok(ToolOutcome::Proposed(mut action)) => {
                    register_pending_action(&state, &mut action).await;
                    tool_events.push(AssistantToolEvent {
                        name: name.clone(),
                        category: action.risk_category().to_string(),
                        status: "needs_confirmation".to_string(),
                        summary: Some(action.summary.clone()),
                    });
                    proposed_actions.push(action.clone());
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": json!({
                            "status": "needs_confirmation",
                            "action_id": action.action_id,
                            "summary": action.summary,
                        }).to_string(),
                    }));
                }
                Err(e) => {
                    tool_events.push(AssistantToolEvent {
                        name: name.clone(),
                        category: "unknown".to_string(),
                        status: "error".to_string(),
                        summary: Some(e.clone()),
                    });
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": json!({ "error": e }).to_string(),
                    }));
                }
            }
        }

        if !proposed_actions.is_empty() {
            let message = if assistant_content.trim().is_empty() {
                "I prepared the action below. Review it and click Apply when you want me to run it."
                    .to_string()
            } else {
                assistant_content
            };
            return Ok(AssistantChatResponse {
                message,
                tool_events,
                proposed_actions,
            });
        }
    }

    Ok(AssistantChatResponse {
        message: "I gathered the available context, but the provider did not finish within the tool loop limit.".to_string(),
        tool_events,
        proposed_actions,
    })
}

async fn register_pending_action(state: &Arc<AppState>, action: &mut AssistantAction) {
    action.action_id = uuid::Uuid::new_v4().to_string();
    action.confirmation_token = uuid::Uuid::new_v4().to_string();
    let raw_action = action.clone();
    let pending = PendingAssistantAction {
        action: raw_action,
        confirmation_token: action.confirmation_token.clone(),
        expires_at: Instant::now() + ASSISTANT_ACTION_TTL,
    };
    let mut map = state.assistant.pending_actions.write().await;
    prune_expired_actions(&mut map);
    // Hard cap: drop the oldest-inserted entry if we're at the limit.
    // This prevents unbounded growth when the user fires many chat requests
    // without confirming or dismissing the resulting pending actions.
    if map.len() >= MAX_PENDING_ACTIONS
        && let Some(oldest_key) = map.keys().next().cloned()
    {
        map.remove(&oldest_key);
    }
    map.insert(action.action_id.clone(), pending);
    action.payload = redact_value(&action.payload);
}

fn prune_expired_actions(pending: &mut HashMap<String, PendingAssistantAction>) {
    let now = Instant::now();
    pending.retain(|_, action| action.expires_at > now);
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |diff, (a, b)| diff | (*a ^ *b))
        == 0
}

fn parse_tool_arguments(call: &Value) -> Result<Value, String> {
    let Some(args) = call.pointer("/function/arguments") else {
        return Ok(json!({}));
    };
    let Some(args) = args.as_str() else {
        return Err("assistant tool arguments must be a JSON string".to_string());
    };
    serde_json::from_str(args)
        .map_err(|e| format!("assistant tool arguments were invalid JSON: {e}"))
}

fn deterministic_workspace_action_from_messages(
    messages: &[AssistantMessage],
) -> Option<WorkspaceActionRequest> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .and_then(|message| deterministic_workspace_action_from_text(&message.content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_workspace_intent_is_detected_before_provider_use() {
        let action = deterministic_workspace_action_from_messages(&[AssistantMessage {
            role: "user".to_string(),
            content: "show failed requests from api.test.com".to_string(),
        }])
        .expect("workspace action");

        assert_eq!(action.action_type, "sessions.apply_filter");
        assert_eq!(action.payload["status_buckets"], json!(["4", "5"]));
        assert_eq!(action.payload["host_focus"], json!(["api.test.com"]));
    }

    #[test]
    fn confirmation_compare_is_constant_time_style() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }

    #[test]
    fn malformed_tool_arguments_return_explicit_error() {
        let err = parse_tool_arguments(&json!({
            "function": {
                "name": "list_sessions",
                "arguments": "{not json"
            }
        }))
        .expect_err("invalid JSON should be reported");

        assert!(err.contains("invalid JSON"));
    }

    #[test]
    fn missing_tool_arguments_default_to_empty_object() {
        let args = parse_tool_arguments(&json!({
            "function": { "name": "get_config" }
        }))
        .expect("missing arguments");

        assert_eq!(args, json!({}));
    }

    #[test]
    fn prune_expired_actions_removes_stale_pending_items() {
        let mut pending = HashMap::from([(
            "a".to_string(),
            PendingAssistantAction {
                action: AssistantAction {
                    action_id: "a".to_string(),
                    confirmation_token: "token".to_string(),
                    kind: "test".to_string(),
                    summary: "test".to_string(),
                    risk: super::super::assistant_action_contracts::AssistantActionRisk::Mutate,
                    endpoint: "/admin/throttling".to_string(),
                    method: "POST".to_string(),
                    payload: json!({}),
                    requires_confirmation: true,
                    preconditions: Vec::new(),
                },
                confirmation_token: "token".to_string(),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        )]);

        prune_expired_actions(&mut pending);

        assert!(pending.is_empty());
    }
}
