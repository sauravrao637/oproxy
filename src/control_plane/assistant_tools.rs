use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::AppState;
use crate::api::{SessionListFilter, SessionListOptions};
use crate::middleware::matcher::{Location, MatchMode};
use crate::middleware::plugins::capture_filter::CaptureFilterConfig;
use crate::middleware::plugins::mock::MockResponse;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::storage;

use super::assistant_actions::{
    AssistantAction, action_state_precondition, propose_action, propose_map_remote_action,
};
use super::assistant_contracts::{
    AssistantToolExecutionKind, AssistantToolRisk, contract_for_tool,
};
use super::assistant_payload_repair::repair_assistant_payload;
use super::assistant_redaction::{redact_string, redact_uri, redact_value};
use super::assistant_registry::{read_feature_catalog, workspace_action_name_for_tool};
use super::workspace::{WorkspaceActionRequest, WorkspaceActionResult, apply_workspace_action};

pub(super) enum ToolOutcome {
    Read(Value),
    Workspace(Box<WorkspaceActionResult>),
    Proposed(AssistantAction),
}

#[derive(Debug, Clone, Copy)]
enum StaticAssistantTool {
    ListSessions,
    GetSession,
    GetConfig,
    GetFeatureCatalog,
    GetRules,
    GetProtocolMetrics,
    GetConnections,
    GetBreakpointDiagnostics,
    GetThrottling,
    GetDns,
    GetCaptureFilter,
    GetWebhooks,
    GetUpstreamProxy,
    ProposeMapRemote,
    ProposeDnsOverride,
    ProposeThrottling,
    ProposeRewriteRule,
    ProposeMockRule,
    ProposeAccessRule,
    ProposeCaptureFilter,
    ProposeUpstreamProxy,
    ProposeAction,
}

impl StaticAssistantTool {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "list_sessions" => Self::ListSessions,
            "get_session" => Self::GetSession,
            "get_config" => Self::GetConfig,
            "get_feature_catalog" => Self::GetFeatureCatalog,
            "get_rules" => Self::GetRules,
            "get_protocol_metrics" => Self::GetProtocolMetrics,
            "get_connections" => Self::GetConnections,
            "get_breakpoint_diagnostics" => Self::GetBreakpointDiagnostics,
            "get_throttling" => Self::GetThrottling,
            "get_dns" => Self::GetDns,
            "get_capture_filter" => Self::GetCaptureFilter,
            "get_webhooks" => Self::GetWebhooks,
            "get_upstream_proxy" => Self::GetUpstreamProxy,
            "propose_map_remote" => Self::ProposeMapRemote,
            "propose_dns_override" => Self::ProposeDnsOverride,
            "propose_throttling" => Self::ProposeThrottling,
            "propose_rewrite_rule" => Self::ProposeRewriteRule,
            "propose_mock_rule" => Self::ProposeMockRule,
            "propose_access_rule" => Self::ProposeAccessRule,
            "propose_capture_filter" => Self::ProposeCaptureFilter,
            "propose_upstream_proxy" => Self::ProposeUpstreamProxy,
            "propose_action" => Self::ProposeAction,
            _ => return None,
        })
    }
}

pub(super) async fn execute_assistant_tool(
    state: &Arc<AppState>,
    name: &str,
    args: Value,
) -> Result<ToolOutcome, String> {
    let Some(contract) = contract_for_tool(name) else {
        return Err(format!(
            "assistant tool '{name}' is not declared or allowlisted"
        ));
    };

    if let Some(action_type) = workspace_action_name_for_tool(name) {
        return apply_workspace_action(
            state,
            WorkspaceActionRequest {
                action_type,
                payload: args,
            },
        )
        .await
        .map(Box::new)
        .map(ToolOutcome::Workspace);
    }

    let Some(tool) = StaticAssistantTool::from_name(name) else {
        return Err(format!(
            "assistant tool '{name}' is declared as {} ({}) but has no executor",
            execution_kind_label(contract.execution_kind),
            risk_label(contract.risk)
        ));
    };

    match tool {
        StaticAssistantTool::ListSessions => {
            read_list_sessions(state, args).await.map(ToolOutcome::Read)
        }
        StaticAssistantTool::GetSession => {
            read_get_session(state, args).await.map(ToolOutcome::Read)
        }
        StaticAssistantTool::GetConfig => read_config(state).await.map(ToolOutcome::Read),
        StaticAssistantTool::GetFeatureCatalog => read_feature_catalog(args).map(ToolOutcome::Read),
        StaticAssistantTool::GetRules => read_rules(state, args).await.map(ToolOutcome::Read),
        StaticAssistantTool::GetProtocolMetrics => Ok(ToolOutcome::Read(json!(
            super::sessions::aggregate_protocol_metrics(state.session_manager.get_all_sessions())
        ))),
        StaticAssistantTool::GetConnections => read_connections(state, args).map(ToolOutcome::Read),
        StaticAssistantTool::GetBreakpointDiagnostics => Ok(ToolOutcome::Read(json!({
            "diagnostics": state.api_handler.list_breakpoint_diagnostics().await,
        }))),
        StaticAssistantTool::GetThrottling => Ok(ToolOutcome::Read(json!(
            state.throttling_config.read().await.clone()
        ))),
        StaticAssistantTool::GetDns => Ok(ToolOutcome::Read(json!(
            state.dns_overrides.read().await.clone()
        ))),
        StaticAssistantTool::GetCaptureFilter => Ok(ToolOutcome::Read(json!(
            state.capture_filter.read().await.clone()
        ))),
        StaticAssistantTool::GetWebhooks => Ok(ToolOutcome::Read(json!(redact_value(&json!(
            state.webhooks.read().await.clone()
        ))))),
        StaticAssistantTool::GetUpstreamProxy => Ok(ToolOutcome::Read(json!({
            "upstream_proxy": storage::load_upstream_proxy(&state.storage_path)
                .or_else(|| state.config.upstream_proxy.clone())
        }))),
        StaticAssistantTool::ProposeMapRemote => {
            propose_map_remote_action(args).map(ToolOutcome::Proposed)
        }
        StaticAssistantTool::ProposeDnsOverride => propose_dns_override_action(state, args)
            .await
            .map(ToolOutcome::Proposed),
        StaticAssistantTool::ProposeThrottling => propose_throttling_action(state, args)
            .await
            .map(ToolOutcome::Proposed),
        StaticAssistantTool::ProposeRewriteRule => {
            propose_rewrite_rule_action(args).map(ToolOutcome::Proposed)
        }
        StaticAssistantTool::ProposeMockRule => {
            propose_mock_rule_action(args).map(ToolOutcome::Proposed)
        }
        StaticAssistantTool::ProposeAccessRule => {
            propose_access_rule_action(args).map(ToolOutcome::Proposed)
        }
        StaticAssistantTool::ProposeCaptureFilter => propose_capture_filter_action(state, args)
            .await
            .map(ToolOutcome::Proposed),
        StaticAssistantTool::ProposeUpstreamProxy => propose_upstream_proxy_action(state, args)
            .await
            .map(ToolOutcome::Proposed),
        StaticAssistantTool::ProposeAction => propose_action(args).map(ToolOutcome::Proposed),
    }
}

fn execution_kind_label(kind: AssistantToolExecutionKind) -> &'static str {
    match kind {
        AssistantToolExecutionKind::Read => "read",
        AssistantToolExecutionKind::Workspace => "workspace",
        AssistantToolExecutionKind::Proposal => "proposal",
    }
}

fn risk_label(risk: AssistantToolRisk) -> &'static str {
    match risk {
        AssistantToolRisk::Read => "read",
        AssistantToolRisk::UiSafe => "ui_safe",
        AssistantToolRisk::UiSensitive => "ui_sensitive",
        AssistantToolRisk::Mutate => "mutate",
        AssistantToolRisk::Network => "network",
        AssistantToolRisk::Destructive => "destructive",
    }
}

async fn read_list_sessions(state: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let q = args.get("q").and_then(Value::as_str).unwrap_or_default();
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.min(50) as usize)
        .or(Some(20));
    let response = state
        .api_handler
        .list_sessions(SessionListOptions {
            limit,
            filter: SessionListFilter {
                query: q.to_string(),
                ..SessionListFilter::default()
            },
            ..SessionListOptions::default()
        })
        .await;
    let sessions: Vec<Value> = response
        .sessions
        .into_iter()
        .map(|exchange| {
            json!({
                "id": exchange.id,
                "timestamp": exchange.timestamp,
                "updated_at": exchange.updated_at,
                "method": exchange.request.method,
                "uri": redact_uri(&exchange.request.uri),
                "host": exchange.request.host,
                "status": exchange.response.as_ref().map(|r| r.status),
                "source": exchange.source,
                "tags": exchange.tags,
                "note": exchange.note,
                "metrics": exchange.metrics,
                "downstream_protocol": exchange.downstream_protocol,
                "connection_id": exchange.connection_id,
                "stream_id": exchange.stream_id,
            })
        })
        .collect();
    Ok(json!({
        "sessions": sessions,
        "total": response.total,
        "limit": response.limit,
        "offset": response.offset,
    }))
}

async fn read_get_session(state: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "get_session requires id".to_string())?;
    let Some(detail) = state.api_handler.get_session_details(id).await else {
        return Err("session not found".to_string());
    };
    let exchange = detail.exchange;
    Ok(json!({
        "id": exchange.id,
        "timestamp": exchange.timestamp,
        "updated_at": exchange.updated_at,
        "request": {
            "method": exchange.request.method,
            "uri": redact_uri(&exchange.request.uri),
            "host": exchange.request.host,
            "headers": redact_value(&json!(exchange.request.headers)),
            "body_bytes": exchange.request.body.len(),
            "body_preview": redact_string(&exchange.request.body_text().chars().take(500).collect::<String>()),
        },
        "response": exchange.response.map(|response| json!({
            "status": response.status,
            "headers": redact_value(&json!(response.headers)),
            "body_bytes": response.body.len(),
            "body_preview": redact_string(&response.body_text().chars().take(500).collect::<String>()),
        })),
        "metrics": exchange.metrics,
        "source": exchange.source,
        "tags": exchange.tags,
        "note": exchange.note,
        "inspector_data": redact_value(&json!(exchange.inspector_data)),
        "downstream_protocol": exchange.downstream_protocol,
        "connection_id": exchange.connection_id,
        "stream_id": exchange.stream_id,
        "protocol_context": exchange.protocol_context,
        "ws_frame_count": exchange.ws_frames.len(),
        "event_count": exchange.events.len(),
        "paused_at": exchange.paused_at,
    }))
}

/// `get_connections`: connection → stream tree built from recorded exchanges
/// (h2/h3 multiplexing view). Trimmed to the most recently active connections
/// so the payload stays small enough for model context.
fn read_connections(state: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, 20) as usize)
        .unwrap_or(10);
    let mut connections =
        super::sessions::aggregate_connections(state.session_manager.get_all_sessions());
    let total = connections.len();
    connections.truncate(limit);
    Ok(json!({
        "connections": redact_value(&json!(connections)),
        "total": total,
        "limit": limit,
    }))
}

async fn read_config(state: &Arc<AppState>) -> Result<Value, String> {
    Ok(json!({
        "port": state.config.port,
        "bind_host": state.config.bind_host,
        "mitm_enabled": state.config.mitm.enabled,
        "https_port": state.config.https_port,
        "socks5_port": state.config.socks5_port,
        "http3_enabled": state.config.http3_enabled,
        "http3_port": state.config.http3_port,
        "otel_enabled": state.config.otel_enabled,
        "max_body_bytes": state.proxy_engine.max_body_bytes(),
        "max_sessions": state.config.max_sessions,
        "max_connections": state.config.max_connections,
        "inspect_ws_frames": state.config.inspect_ws_frames,
        "allow_remote_admin": state.config.allow_remote_admin,
        "allow_private_admin_egress": state.config.allow_private_admin_egress,
        "admin_auth_enabled": state.config.admin_token.as_deref().is_some_and(|token| !token.trim().is_empty()),
        "storage_path": state.storage_path.display().to_string(),
        "uptime_secs": state.started_at.elapsed().as_secs(),
    }))
}

async fn read_rules(state: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let resource = args
        .get("resource")
        .and_then(Value::as_str)
        .unwrap_or("all");
    let value = match resource {
        "rule_sets" => json!(state.rule_sets.read().await.clone()),
        "map_remote" => json!(state.map_remote_rules.read().await.clone()),
        "map_local" => json!(state.map_local_rules.read().await.clone()),
        "access" => json!(state.access_rules.read().await.clone()),
        "breakpoints" => json!(state.api_handler.list_breakpoint_rules().await),
        "mock" => json!(state.mock_rules.read().await.clone()),
        "scripts" => json!(redact_value(&json!(state.lua_scripts.read().await.clone()))),
        "all" => json!({
            "rule_sets": state.rule_sets.read().await.clone(),
            "map_remote": state.map_remote_rules.read().await.clone(),
            "map_local": state.map_local_rules.read().await.clone(),
            "access": state.access_rules.read().await.clone(),
            "breakpoints": state.api_handler.list_breakpoint_rules().await,
            "mock": state.mock_rules.read().await.clone(),
            "scripts": redact_value(&json!(state.lua_scripts.read().await.clone())),
        }),
        _ => return Err("unknown rules resource".to_string()),
    };
    Ok(value)
}

async fn propose_dns_override_action(
    state: &Arc<AppState>,
    args: Value,
) -> Result<AssistantAction, String> {
    let current = state.dns_overrides.read().await.clone();
    propose_dns_override_action_from_map(current, args)
}

fn propose_dns_override_action_from_map(
    current: crate::middleware::plugins::dns_override::DnsOverrides,
    args: Value,
) -> Result<AssistantAction, String> {
    let current_state = json!(current);
    let operation = args
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("set")
        .trim()
        .to_ascii_lowercase();
    let host = normalize_dns_host(
        args.get("host")
            .and_then(Value::as_str)
            .ok_or_else(|| "propose_dns_override requires host".to_string())?,
    )?;

    match operation.as_str() {
        "set" | "update" => {
            let ip =
                normalize_dns_ip(args.get("ip").and_then(Value::as_str).ok_or_else(|| {
                    "propose_dns_override operation=set requires ip".to_string()
                })?)?;
            let mut next: crate::middleware::plugins::dns_override::DnsOverrides =
                serde_json::from_value(current_state.clone())
                    .map_err(|e| format!("invalid current DNS state: {e}"))?;
            next.insert(
                host.clone(),
                crate::middleware::plugins::dns_override::DnsEntry {
                    ip: ip.clone(),
                    enabled: true,
                },
            );
            let mut action = propose_action(json!({
                "method": "POST",
                "endpoint": "/admin/dns",
                "summary": format!("Resolve {host} to {ip}"),
                "payload": next,
            }))?;
            action.preconditions.push(action_state_precondition(
                "dns",
                &current_state,
                "DNS overrides changed since this action was prepared; refresh the assistant action before applying.",
            ));
            Ok(action)
        }
        "delete" | "remove" | "clear" => {
            let mut action = propose_action(json!({
                "method": "DELETE",
                "endpoint": format!("/admin/dns/{host}"),
                "summary": format!("Delete DNS override for {host}"),
                "payload": Value::Null,
            }))?;
            action.preconditions.push(action_state_precondition(
                "dns",
                &current_state,
                "DNS overrides changed since this action was prepared; refresh the assistant action before applying.",
            ));
            Ok(action)
        }
        _ => Err("propose_dns_override operation must be set or delete".to_string()),
    }
}

fn normalize_dns_host(raw: &str) -> Result<String, String> {
    let host = raw
        .trim()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '<' | '>' | ',' | ';' | '.'))
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err("DNS override host is required".to_string());
    }
    if host.len() > 255
        || host.contains('/')
        || host.contains('?')
        || host.contains('#')
        || host.chars().any(char::is_whitespace)
    {
        return Err("DNS override host must be a clean hostname".to_string());
    }
    Ok(host)
}

fn normalize_dns_ip(raw: &str) -> Result<String, String> {
    let ip = raw.trim();
    ip.parse::<IpAddr>()
        .map(|addr| addr.to_string())
        .map_err(|_| "DNS override ip must be a valid IPv4 or IPv6 address".to_string())
}

async fn propose_throttling_action(
    state: &Arc<AppState>,
    args: Value,
) -> Result<AssistantAction, String> {
    let current = state.throttling_config.read().await.clone();
    propose_throttling_action_from_config(current, args)
}

fn propose_throttling_action_from_config(
    current: ThrottlingConfig,
    mut args: Value,
) -> Result<AssistantAction, String> {
    let current_state = json!(current);
    let current: ThrottlingConfig = serde_json::from_value(current_state.clone())
        .map_err(|e| format!("invalid current throttling state: {e}"))?;
    repair_assistant_payload(&mut args);
    let enabled = args.get("enabled").and_then(Value::as_bool);
    let latency_ms = optional_u64_arg(&args, "latency_ms")?;
    let bandwidth_limit_kbps = optional_u64_arg(&args, "bandwidth_limit_kbps")?;

    if enabled.is_none() && latency_ms.is_none() && bandwidth_limit_kbps.is_none() {
        return Err(
            "propose_throttling requires enabled, latency_ms, or bandwidth_limit_kbps".to_string(),
        );
    }

    let next = ThrottlingConfig {
        enabled: enabled.unwrap_or_else(|| {
            if latency_ms.is_some() || bandwidth_limit_kbps.is_some() {
                true
            } else {
                current.enabled
            }
        }),
        latency_ms: latency_ms.unwrap_or(current.latency_ms),
        bandwidth_limit_kbps: bandwidth_limit_kbps.unwrap_or(current.bandwidth_limit_kbps),
    };
    let summary = if !next.enabled {
        "Disable throttling".to_string()
    } else {
        format!(
            "Set throttling to {} ms latency and {} kbps bandwidth",
            next.latency_ms, next.bandwidth_limit_kbps
        )
    };

    let mut action = propose_action(json!({
        "method": "POST",
        "endpoint": "/admin/throttling",
        "summary": summary,
        "payload": next,
    }))?;
    action.preconditions.push(action_state_precondition(
        "throttling",
        &current_state,
        "Throttling config changed since this action was prepared; refresh the assistant action before applying.",
    ));
    Ok(action)
}

fn optional_u64_arg(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
        Some(_) => Err(format!("{key} must be a non-negative integer")),
    }
}

fn propose_rewrite_rule_action(mut args: Value) -> Result<AssistantAction, String> {
    repair_assistant_payload(&mut args);
    let actions = args
        .get("actions")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .cloned()
        .ok_or_else(|| "propose_rewrite_rule requires at least one action".to_string())?;
    let location = location_from_args(&args)?;
    let applies_to = args
        .get("applies_to")
        .and_then(Value::as_str)
        .unwrap_or("both")
        .to_ascii_lowercase();
    if !matches!(applies_to.as_str(), "request" | "response" | "both") {
        return Err(
            "propose_rewrite_rule applies_to must be request, response, or both".to_string(),
        );
    }
    let name = non_empty_string_arg(&args, "name").unwrap_or_else(|| {
        format!(
            "Rewrite {}",
            location_summary(
                location.host.as_deref(),
                location.path.as_deref(),
                location.methods.as_slice()
            )
        )
    });
    let payload = json!({
        "id": "",
        "name": name,
        "enabled": args.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        "location": location,
        "applies_to": applies_to,
        "actions": actions,
    });

    propose_action(json!({
        "method": "POST",
        "endpoint": "/admin/rule-sets",
        "summary": payload["name"].as_str().unwrap_or("Create rewrite rule"),
        "payload": payload,
    }))
}

fn propose_mock_rule_action(mut args: Value) -> Result<AssistantAction, String> {
    repair_assistant_payload(&mut args);
    let location = location_from_args(&args)?;
    let responses = mock_responses_from_args(&args)?;
    let name = non_empty_string_arg(&args, "name").unwrap_or_else(|| {
        format!(
            "Mock {}",
            location_summary(
                location.host.as_deref(),
                location.path.as_deref(),
                location.methods.as_slice()
            )
        )
    });
    let payload = json!({
        "id": "",
        "name": name,
        "enabled": args.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        "location": location,
        "responses": responses,
        "call_count": 0,
    });

    propose_action(json!({
        "method": "POST",
        "endpoint": "/admin/mock/rules",
        "summary": payload["name"].as_str().unwrap_or("Create mock rule"),
        "payload": payload,
    }))
}

fn propose_access_rule_action(mut args: Value) -> Result<AssistantAction, String> {
    repair_assistant_payload(&mut args);
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("block")
        .to_ascii_lowercase();
    if !matches!(action.as_str(), "block" | "allow") {
        return Err("propose_access_rule action must be block or allow".to_string());
    }
    let location = location_from_args(&args)?;
    let name = non_empty_string_arg(&args, "name").unwrap_or_else(|| {
        format!(
            "{} {}",
            title_case(&action),
            location_summary(
                location.host.as_deref(),
                location.path.as_deref(),
                location.methods.as_slice()
            )
        )
    });
    let payload = json!({
        "id": "",
        "name": name,
        "enabled": args.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        "location": location,
        "action": action,
    });

    propose_action(json!({
        "method": "POST",
        "endpoint": "/admin/access-rules",
        "summary": payload["name"].as_str().unwrap_or("Create access rule"),
        "payload": payload,
    }))
}

async fn propose_capture_filter_action(
    state: &Arc<AppState>,
    args: Value,
) -> Result<AssistantAction, String> {
    let current = state.capture_filter.read().await.clone();
    propose_capture_filter_action_from_config(current, args)
}

fn propose_capture_filter_action_from_config(
    current: CaptureFilterConfig,
    mut args: Value,
) -> Result<AssistantAction, String> {
    repair_assistant_payload(&mut args);
    let current_state = json!(current);
    let mode = args
        .get("mode")
        .and_then(Value::as_str)
        .map(|mode| mode.to_ascii_lowercase())
        .unwrap_or_else(|| "disabled".to_string());
    if !matches!(mode.as_str(), "disabled" | "allowlist" | "denylist") {
        return Err(
            "propose_capture_filter mode must be disabled, allowlist, or denylist".to_string(),
        );
    }
    let hosts = args
        .get("hosts")
        .and_then(parse_string_array_arg)
        .unwrap_or_default();
    let payload = json!({
        "mode": mode,
        "hosts": if mode == "disabled" { Vec::<String>::new() } else { hosts },
    });
    let summary = match payload["mode"].as_str().unwrap_or("disabled") {
        "allowlist" => "Set capture filter to allowlist",
        "denylist" => "Set capture filter to denylist",
        _ => "Disable capture filter",
    };

    let mut action = propose_action(json!({
        "method": "POST",
        "endpoint": "/admin/capture-filter",
        "summary": summary,
        "payload": payload,
    }))?;
    action.preconditions.push(action_state_precondition(
        "capture_filter",
        &current_state,
        "Capture filter changed since this action was prepared; refresh the assistant action before applying.",
    ));
    Ok(action)
}

async fn propose_upstream_proxy_action(
    state: &Arc<AppState>,
    args: Value,
) -> Result<AssistantAction, String> {
    let current = json!({
        "upstream_proxy": storage::load_upstream_proxy(&state.storage_path)
            .or_else(|| state.config.upstream_proxy.clone())
    });
    propose_upstream_proxy_action_from_state(current, args)
}

fn propose_upstream_proxy_action_from_state(
    current_state: Value,
    args: Value,
) -> Result<AssistantAction, String> {
    let proxy = args
        .get("upstream_proxy")
        .or_else(|| args.get("proxy"))
        .and_then(|value| {
            if value.is_null() {
                Some(None)
            } else {
                value.as_str().map(|raw| raw.trim().to_string()).map(Some)
            }
        })
        .ok_or_else(|| "propose_upstream_proxy requires upstream_proxy or proxy".to_string())?;
    let proxy = proxy.filter(|value| !value.is_empty());
    let payload = json!({ "upstream_proxy": proxy });
    let summary = payload["upstream_proxy"]
        .as_str()
        .map(|value| format!("Set upstream proxy to {value}"))
        .unwrap_or_else(|| "Clear upstream proxy".to_string());

    let mut action = propose_action(json!({
        "method": "POST",
        "endpoint": "/admin/upstream-proxy",
        "summary": summary,
        "payload": payload,
    }))?;
    action.preconditions.push(action_state_precondition(
        "upstream_proxy",
        &current_state,
        "Upstream proxy changed since this action was prepared; refresh the assistant action before applying.",
    ));
    Ok(action)
}

fn location_from_args(args: &Value) -> Result<Location, String> {
    let mut location = args
        .get("location")
        .filter(|value| value.is_object())
        .cloned()
        .map(serde_json::from_value::<Location>)
        .transpose()
        .map_err(|e| format!("invalid location: {e}"))?
        .unwrap_or_default();

    if let Some(host) = non_empty_string_arg(args, "host") {
        location.host = Some(host);
    }
    if let Some(path) = non_empty_string_arg(args, "path") {
        location.path = Some(path);
    }
    if let Some(protocol) = non_empty_string_arg(args, "protocol") {
        location.protocol = Some(protocol.to_ascii_lowercase());
    }
    if let Some(wire_protocol) = non_empty_string_arg(args, "wire_protocol") {
        location.wire_protocol = Some(wire_protocol.to_ascii_lowercase());
    }
    if let Some(application_protocol) = non_empty_string_arg(args, "application_protocol") {
        location.application_protocol = Some(application_protocol.to_ascii_lowercase());
    }
    if let Some(body_mode) = non_empty_string_arg(args, "body_mode") {
        location.body_mode = Some(body_mode.to_ascii_lowercase());
    }
    if let Some(query) = non_empty_string_arg(args, "query") {
        location.query = Some(query);
    }
    if let Some(port) = optional_u64_arg(args, "port")? {
        location.port =
            Some(u16::try_from(port).map_err(|_| "port must be between 0 and 65535".to_string())?);
    }
    if let Some(methods) = args.get("methods").and_then(parse_string_array_arg) {
        location.methods = methods
            .into_iter()
            .map(|method| method.to_ascii_uppercase())
            .collect();
    }
    if let Some(mode) = args.get("mode").and_then(Value::as_str) {
        location.mode = match mode.to_ascii_lowercase().as_str() {
            "glob" => MatchMode::Glob,
            "regex" => MatchMode::Regex,
            _ => return Err("location mode must be glob or regex".to_string()),
        };
    }
    Ok(location)
}

fn mock_responses_from_args(args: &Value) -> Result<Vec<MockResponse>, String> {
    if let Some(responses) = args.get("responses").and_then(Value::as_array) {
        if responses.is_empty() {
            return Err("propose_mock_rule responses must not be empty".to_string());
        }
        return responses
            .iter()
            .cloned()
            .map(|response| {
                serde_json::from_value::<MockResponse>(response)
                    .map_err(|e| format!("invalid mock response: {e}"))
            })
            .collect();
    }

    let status = args
        .get("status")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .try_into()
        .map_err(|_| "mock status must be between 100 and 599".to_string())?;
    if !(100..=599).contains(&status) {
        return Err("mock status must be between 100 and 599".to_string());
    }
    let body = args
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut headers = args
        .get("headers")
        .cloned()
        .map(serde_json::from_value::<HashMap<String, String>>)
        .transpose()
        .map_err(|e| format!("invalid mock headers: {e}"))?
        .unwrap_or_default();
    let content_type = non_empty_string_arg(args, "content_type")
        .unwrap_or_else(|| "application/json".to_string());
    headers
        .entry("content-type".to_string())
        .or_insert(content_type);
    let delay_ms = optional_u64_arg(args, "delay_ms")?.unwrap_or(0);

    Ok(vec![MockResponse {
        status,
        headers,
        body,
        delay_ms,
    }])
}

fn non_empty_string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_string_array_arg(value: &Value) -> Option<Vec<String>> {
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn location_summary(host: Option<&str>, path: Option<&str>, methods: &[String]) -> String {
    let mut parts = Vec::new();
    if !methods.is_empty() {
        parts.push(methods.join(","));
    }
    if let Some(host) = host {
        parts.push(host.to_string());
    }
    if let Some(path) = path {
        parts.push(path.to_string());
    }
    if parts.is_empty() {
        "matching traffic".to_string()
    } else {
        parts.join(" ")
    }
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

pub(super) fn tool_summary(value: &Value) -> String {
    match value {
        Value::Object(map) => format!("{} fields returned", map.len()),
        Value::Array(items) => format!("{} items returned", items.len()),
        _ => "tool result returned".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct AssistantEvalSuite {
        cases: Vec<AssistantEvalCase>,
    }

    #[derive(Debug, Deserialize)]
    struct AssistantEvalCase {
        id: String,
        prompt: String,
        expected_tool: String,
        #[serde(default)]
        current_dns: HashMap<String, crate::middleware::plugins::dns_override::DnsValue>,
        #[serde(default)]
        current_capture_filter: Option<CaptureFilterConfig>,
        #[serde(default)]
        current_throttling: Option<ThrottlingConfig>,
        #[serde(default)]
        current_upstream_proxy: Value,
        args: Value,
        expected_action: ExpectedAction,
    }

    #[derive(Debug, Deserialize)]
    struct ExpectedAction {
        method: String,
        endpoint: String,
        #[serde(default)]
        payload: Value,
    }

    /// Executor coverage: every tool exposed to the model must dispatch to a
    /// real executor. `contract_for_tool` already gates unknown names, and a
    /// separate test guarantees contracts == exposed tools, but neither catches
    /// a tool that has a contract yet falls through `execute_assistant_tool` to
    /// the "no executor" arm. This locks the match arms against the registry so
    /// adding a tool to the manifest without wiring an executor fails in CI.
    #[test]
    fn every_model_tool_has_an_executor() {
        use std::collections::BTreeSet;
        let handled: BTreeSet<&str> = [
            "list_sessions",
            "get_session",
            "get_config",
            "get_feature_catalog",
            "get_rules",
            "get_protocol_metrics",
            "get_connections",
            "get_breakpoint_diagnostics",
            "get_throttling",
            "get_dns",
            "get_capture_filter",
            "get_webhooks",
            "get_upstream_proxy",
            "propose_map_remote",
            "propose_dns_override",
            "propose_throttling",
            "propose_rewrite_rule",
            "propose_mock_rule",
            "propose_access_rule",
            "propose_capture_filter",
            "propose_upstream_proxy",
            "propose_action",
        ]
        .into_iter()
        .collect();

        for name in super::super::assistant_registry::openai_tool_names() {
            if name.starts_with("workspace_") {
                assert!(
                    workspace_action_name_for_tool(&name).is_some(),
                    "workspace tool '{name}' has no backing workspace action executor"
                );
            } else {
                assert!(
                    handled.contains(name.as_str()),
                    "model tool '{name}' is exposed but has no executor arm in execute_assistant_tool"
                );
            }
        }
    }

    #[test]
    fn tool_summary_describes_common_payload_shapes() {
        assert_eq!(
            tool_summary(&json!({ "a": 1, "b": 2 })),
            "2 fields returned"
        );
        assert_eq!(tool_summary(&json!([1, 2, 3])), "3 items returned");
        assert_eq!(tool_summary(&json!(true)), "tool result returned");
    }

    #[test]
    fn dns_override_proposal_merges_with_existing_overrides() {
        use crate::middleware::plugins::dns_override::{DnsEntry, DnsOverrides};
        let existing: DnsOverrides = HashMap::from([(
            "old.test.com".to_string(),
            DnsEntry {
                ip: "10.0.0.1".to_string(),
                enabled: true,
            },
        )]);
        let action = propose_dns_override_action_from_map(
            existing,
            json!({
                "operation": "set",
                "host": "API.TEST.COM",
                "ip": "10.0.0.5",
            }),
        )
        .expect("dns proposal");

        assert_eq!(action.method, "POST");
        assert_eq!(action.endpoint, "/admin/dns");
        assert_eq!(action.payload["old.test.com"]["ip"], "10.0.0.1");
        assert_eq!(action.payload["api.test.com"]["ip"], "10.0.0.5");
        assert_eq!(action.preconditions.len(), 1);
        assert_eq!(action.preconditions[0].resource, "dns");
    }

    #[test]
    fn dns_override_delete_proposes_item_delete() {
        use crate::middleware::plugins::dns_override::DnsOverrides;
        let action = propose_dns_override_action_from_map(
            DnsOverrides::new(),
            json!({
                "operation": "delete",
                "host": "api.test.com",
            }),
        )
        .expect("dns delete proposal");

        assert_eq!(action.method, "DELETE");
        assert_eq!(action.endpoint, "/admin/dns/api.test.com");
        assert!(action.payload.is_null());
    }

    #[test]
    fn throttling_proposal_preserves_unspecified_fields() {
        let action = propose_throttling_action_from_config(
            ThrottlingConfig {
                enabled: false,
                latency_ms: 50,
                bandwidth_limit_kbps: 128,
            },
            json!({ "latency_ms": "250" }),
        )
        .expect("throttling proposal");

        assert_eq!(action.endpoint, "/admin/throttling");
        assert_eq!(action.payload["enabled"], true);
        assert_eq!(action.payload["latency_ms"], 250);
        assert_eq!(action.payload["bandwidth_limit_kbps"], 128);
        assert_eq!(action.preconditions.len(), 1);
        assert_eq!(action.preconditions[0].resource, "throttling");
    }

    #[test]
    fn rewrite_rule_proposal_builds_ui_payload() {
        let action = propose_rewrite_rule_action(json!({
            "host": "api.test.com",
            "path": "/v1/*",
            "methods": ["GET"],
            "applies_to": "request",
            "actions": [{ "type": "set_header", "name": "x-debug", "value": "yes" }]
        }))
        .expect("rewrite proposal");

        assert_eq!(action.endpoint, "/admin/rule-sets");
        assert_eq!(action.payload["location"]["host"], "api.test.com");
        assert_eq!(action.payload["location"]["path"], "/v1/*");
        assert_eq!(action.payload["location"]["methods"], json!(["GET"]));
        assert_eq!(action.payload["applies_to"], "request");
        assert_eq!(action.payload["actions"][0]["type"], "set_header");
    }

    #[test]
    fn rewrite_rule_proposal_preserves_protocol_matcher_fields() {
        let action = propose_rewrite_rule_action(json!({
            "host": "api.test.com",
            "wire_protocol": "HTTP2",
            "application_protocol": "GRPC",
            "body_mode": "stream_messages",
            "actions": [{ "type": "set_header", "name": "x-debug", "value": "yes" }]
        }))
        .expect("rewrite proposal");

        assert_eq!(action.payload["location"]["wire_protocol"], "http2");
        assert_eq!(action.payload["location"]["application_protocol"], "grpc");
        assert_eq!(action.payload["location"]["body_mode"], "stream_messages");
    }

    #[test]
    fn mock_rule_proposal_builds_single_response_payload() {
        let action = propose_mock_rule_action(json!({
            "path": "/users",
            "methods": ["GET"],
            "status": "200",
            "body": "[]",
            "content_type": "application/json",
        }))
        .expect("mock proposal");

        assert_eq!(action.endpoint, "/admin/mock/rules");
        assert_eq!(action.payload["location"]["path"], "/users");
        assert_eq!(action.payload["responses"][0]["status"], 200);
        assert_eq!(
            action.payload["responses"][0]["headers"]["content-type"],
            "application/json"
        );
        assert_eq!(action.payload["responses"][0]["body"], "[]");
        assert_eq!(action.payload["call_count"], 0);
    }

    #[test]
    fn access_rule_proposal_builds_block_rule_payload() {
        let action = propose_access_rule_action(json!({
            "action": "block",
            "host": "analytics.example.com",
        }))
        .expect("access proposal");

        assert_eq!(action.endpoint, "/admin/access-rules");
        assert_eq!(action.payload["action"], "block");
        assert_eq!(action.payload["location"]["host"], "analytics.example.com");
    }

    #[test]
    fn capture_filter_proposal_adds_state_precondition() {
        let action = propose_capture_filter_action_from_config(
            CaptureFilterConfig {
                mode: Default::default(),
                hosts: Vec::new(),
            },
            json!({ "mode": "denylist", "hosts": "analytics.example.com" }),
        )
        .expect("capture proposal");

        assert_eq!(action.endpoint, "/admin/capture-filter");
        assert_eq!(action.payload["mode"], "denylist");
        assert_eq!(action.payload["hosts"], json!(["analytics.example.com"]));
        assert_eq!(action.preconditions.len(), 1);
        assert_eq!(action.preconditions[0].resource, "capture_filter");
    }

    #[test]
    fn upstream_proxy_proposal_adds_state_precondition() {
        let action = propose_upstream_proxy_action_from_state(
            json!({ "upstream_proxy": Value::Null }),
            json!({ "upstream_proxy": "http://proxy.local:3128" }),
        )
        .expect("upstream proposal");

        assert_eq!(action.endpoint, "/admin/upstream-proxy");
        assert_eq!(action.payload["upstream_proxy"], "http://proxy.local:3128");
        assert_eq!(action.preconditions.len(), 1);
        assert_eq!(action.preconditions[0].resource, "upstream_proxy");
    }

    #[test]
    fn golden_conversation_eval_cases_produce_expected_actions() {
        let suite: AssistantEvalSuite =
            serde_yaml::from_str(include_str!("assistant_eval_cases.yaml"))
                .expect("assistant eval cases should parse");
        assert!(!suite.cases.is_empty());

        for case in suite.cases {
            let action = run_eval_case(&case).unwrap_or_else(|error| {
                panic!(
                    "assistant eval case '{}' failed for prompt '{}': {error}",
                    case.id, case.prompt
                )
            });

            assert_eq!(
                action.method, case.expected_action.method,
                "case {} method",
                case.id
            );
            assert_eq!(
                action.endpoint, case.expected_action.endpoint,
                "case {} endpoint",
                case.id
            );
            assert_value_contains(
                &action.payload,
                &case.expected_action.payload,
                &format!("case {} payload", case.id),
            );
        }
    }

    fn run_eval_case(case: &AssistantEvalCase) -> Result<AssistantAction, String> {
        match case.expected_tool.as_str() {
            "propose_dns_override" => propose_dns_override_action_from_map(
                case.current_dns
                    .clone()
                    .into_iter()
                    .map(|(k, v)| (k, v.into_entry()))
                    .collect(),
                case.args.clone(),
            ),
            "propose_throttling" => propose_throttling_action_from_config(
                case.current_throttling.clone().unwrap_or(ThrottlingConfig {
                    enabled: false,
                    latency_ms: 0,
                    bandwidth_limit_kbps: 0,
                }),
                case.args.clone(),
            ),
            "propose_map_remote" => propose_map_remote_action(case.args.clone()),
            "propose_rewrite_rule" => propose_rewrite_rule_action(case.args.clone()),
            "propose_mock_rule" => propose_mock_rule_action(case.args.clone()),
            "propose_access_rule" => propose_access_rule_action(case.args.clone()),
            "propose_capture_filter" => propose_capture_filter_action_from_config(
                case.current_capture_filter
                    .clone()
                    .unwrap_or(CaptureFilterConfig {
                        mode: Default::default(),
                        hosts: Vec::new(),
                    }),
                case.args.clone(),
            ),
            "propose_upstream_proxy" => {
                let current = if case.current_upstream_proxy.is_null() {
                    json!({ "upstream_proxy": Value::Null })
                } else {
                    case.current_upstream_proxy.clone()
                };
                propose_upstream_proxy_action_from_state(current, case.args.clone())
            }
            other => Err(format!("unsupported eval expected_tool '{other}'")),
        }
    }

    fn assert_value_contains(actual: &Value, expected: &Value, path: &str) {
        match expected {
            Value::Object(expected_map) => {
                let actual_map = actual
                    .as_object()
                    .unwrap_or_else(|| panic!("{path}: expected object, got {actual:?}"));
                for (key, expected_value) in expected_map {
                    let actual_value = actual_map
                        .get(key)
                        .unwrap_or_else(|| panic!("{path}: missing key '{key}'"));
                    assert_value_contains(actual_value, expected_value, &format!("{path}.{key}"));
                }
            }
            Value::Array(expected_items) => {
                let actual_items = actual
                    .as_array()
                    .unwrap_or_else(|| panic!("{path}: expected array, got {actual:?}"));
                assert!(
                    actual_items.len() >= expected_items.len(),
                    "{path}: expected at least {} items, got {}",
                    expected_items.len(),
                    actual_items.len()
                );
                for (idx, expected_item) in expected_items.iter().enumerate() {
                    assert_value_contains(
                        &actual_items[idx],
                        expected_item,
                        &format!("{path}[{idx}]"),
                    );
                }
            }
            _ => assert_eq!(actual, expected, "{path}"),
        }
    }
}
