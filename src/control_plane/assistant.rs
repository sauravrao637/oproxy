use axum::{Json, extract::State, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::AppState;

use super::assistant_actions::{AssistantAction, execute_action_payload};
use super::assistant_context::build_assistant_context;
use super::assistant_contracts::grouped_tool_contract_info;
use super::assistant_prompt::build_initial_messages;
use super::assistant_provider::{AssistantProviderConfig, OpenAiCompatibleProviderClient};
use super::assistant_redaction::redact_value;
use super::assistant_registry::openai_tool_specs;
use super::assistant_tools::{ToolOutcome, execute_assistant_tool, tool_summary};

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

struct ProcessedToolCall {
    event: AssistantToolEvent,
    message: Value,
    proposed_action: Option<AssistantAction>,
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
    // All intent resolution flows through the model. Earlier builds short-cut
    // certain phrasings ("map A to B", "show failed requests") with keyword
    // regex before calling the provider; those heuristics misfired often
    // (hijacking questions, wrong host/status extraction) and are gone. The
    // model drives workspace navigation/filtering via the workspace_* tools and
    // mutations via the propose_* tools, all of which carry full context.
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
            let processed = process_tool_call(&state, &call).await;
            tool_events.push(processed.event);
            messages.push(processed.message);
            if let Some(action) = processed.proposed_action {
                proposed_actions.push(action);
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

    // The tool budget is spent and the model never produced a final text turn
    // (it kept calling read tools). Rather than discarding everything it
    // gathered behind a canned failure string, make one more completion with
    // tools disabled so it must answer from the context already in `messages`.
    let message = match provider_client.chat_completion_text_only(&messages).await {
        Ok(final_message) if !final_message.content.trim().is_empty() => final_message.content,
        _ => "I gathered the available context but could not compose a final answer within the tool budget. Try narrowing the request.".to_string(),
    };
    Ok(AssistantChatResponse {
        message,
        tool_events,
        proposed_actions,
    })
}

async fn process_tool_call(state: &Arc<AppState>, call: &Value) -> ProcessedToolCall {
    let call_id = call
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("tool_call");
    let name = call
        .pointer("/function/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let outcome = match parse_tool_arguments(call) {
        Ok(arguments) => execute_assistant_tool(state, name, arguments).await,
        Err(error) => Err(error),
    };
    let (event, content, proposed_action) = match outcome {
        Ok(ToolOutcome::Read(result)) => (
            AssistantToolEvent {
                name: name.to_string(),
                category: "read".to_string(),
                status: "ok".to_string(),
                summary: Some(tool_summary(&result)),
            },
            result,
            None,
        ),
        Ok(ToolOutcome::Workspace(result)) => {
            let content = serde_json::to_value(&result)
                .unwrap_or_else(|_| json!({ "ok": true, "message": result.message }));
            (
                AssistantToolEvent {
                    name: name.to_string(),
                    category: "ui".to_string(),
                    status: "ok".to_string(),
                    summary: Some(result.message),
                },
                content,
                None,
            )
        }
        Ok(ToolOutcome::Proposed(mut action)) => {
            register_pending_action(state, &mut action).await;
            let content = json!({
                "status": "needs_confirmation",
                "action_id": action.action_id,
                "summary": action.summary,
            });
            (
                AssistantToolEvent {
                    name: name.to_string(),
                    category: action.risk_category().to_string(),
                    status: "needs_confirmation".to_string(),
                    summary: Some(action.summary.clone()),
                },
                content,
                Some(action),
            )
        }
        Err(error) => (
            AssistantToolEvent {
                name: name.to_string(),
                category: "unknown".to_string(),
                status: "error".to_string(),
                summary: Some(error.clone()),
            },
            json!({ "error": error }),
            None,
        ),
    };
    ProcessedToolCall {
        event,
        message: json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": content.to_string(),
        }),
        proposed_action,
    }
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

/// Parse a tool call's `arguments` into a JSON object, tolerant of the shapes
/// real OpenAI-compatible providers actually emit:
///
/// - a JSON *string* (the OpenAI spec): parse it;
/// - an empty/whitespace string or absent/null: treat as `{}`;
/// - an already-decoded *object* (vLLM, llama.cpp, some Ollama models): use it.
///
/// This is spec-tolerant, not model-specific — no provider is special-cased.
pub(super) fn parse_tool_arguments(call: &Value) -> Result<Value, String> {
    let Some(args) = call.pointer("/function/arguments") else {
        return Ok(json!({}));
    };
    match args {
        Value::Null => Ok(json!({})),
        Value::Object(_) | Value::Array(_) => Ok(args.clone()),
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(json!({}));
            }
            serde_json::from_str(trimmed)
                .map_err(|e| format!("assistant tool arguments were invalid JSON: {e}"))
        }
        other => Err(format!(
            "assistant tool arguments must be a JSON object or string, got {other}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn object_form_tool_arguments_are_accepted() {
        // vLLM / llama.cpp / some Ollama models emit arguments as a decoded
        // object instead of a JSON string. We must accept it as-is.
        let args = parse_tool_arguments(&json!({
            "function": {
                "name": "list_sessions",
                "arguments": { "q": "host:api.test.com", "limit": 5 }
            }
        }))
        .expect("object arguments");

        assert_eq!(args, json!({ "q": "host:api.test.com", "limit": 5 }));
    }

    #[test]
    fn empty_and_null_tool_arguments_default_to_empty_object() {
        let from_empty = parse_tool_arguments(&json!({
            "function": { "name": "get_config", "arguments": "" }
        }))
        .expect("empty-string arguments");
        assert_eq!(from_empty, json!({}));

        let from_whitespace = parse_tool_arguments(&json!({
            "function": { "name": "get_config", "arguments": "  \n " }
        }))
        .expect("whitespace arguments");
        assert_eq!(from_whitespace, json!({}));

        let from_null = parse_tool_arguments(&json!({
            "function": { "name": "get_config", "arguments": Value::Null }
        }))
        .expect("null arguments");
        assert_eq!(from_null, json!({}));
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
