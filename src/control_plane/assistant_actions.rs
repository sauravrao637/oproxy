use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn null_as_empty_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::plugins::access_control::AccessRule;
use crate::middleware::plugins::breakpoints::BreakpointRule;
use crate::middleware::plugins::capture_filter::CaptureFilterConfig;
use crate::middleware::plugins::lua_engine::LuaScript;
use crate::middleware::plugins::map_local::MapLocalRule;
use crate::middleware::plugins::map_remote::MapRemoteRule;
use crate::middleware::plugins::mock::MockRule;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::middleware::plugins::rules::RewriteRuleSet;
use crate::security::{AdminEgressPolicy, enforce_admin_egress_policy};
use crate::storage;
use crate::webhooks::{WebhookConfig, sanitize_webhook_events};

use super::assistant::AssistantMessage;
use super::assistant_action_contracts::{
    AssistantActionRisk, AssistantActionRouteContract, action_route_contract,
    id_backed_collection_bases, refreshed_resources_for_action,
};
use super::assistant_payload_repair::repair_assistant_payload;
use super::assistant_redaction::{redact_string, redact_value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AssistantAction {
    pub action_id: String,
    pub confirmation_token: String,
    pub kind: String,
    pub summary: String,
    pub risk: AssistantActionRisk,
    pub endpoint: String,
    pub method: String,
    #[serde(default)]
    pub payload: Value,
    pub requires_confirmation: bool,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_as_empty_vec"
    )]
    pub preconditions: Vec<AssistantActionPrecondition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AssistantActionPrecondition {
    pub resource: String,
    pub expected_hash: String,
    pub message: String,
}

pub(super) fn propose_action(args: Value) -> Result<AssistantAction, String> {
    let method = args
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_uppercase();
    let endpoint = args
        .get("endpoint")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let payload = args.get("payload").cloned().unwrap_or(Value::Null);
    let summary = args
        .get("summary")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Apply assistant-proposed oproxy change")
        .to_string();
    let route = validate_action_route(&method, &endpoint, &payload)?;
    let payload = normalize_action_payload(&method, &endpoint, payload, &summary)?;
    validate_action_payload_shape(&method, &endpoint, &payload)?;
    Ok(AssistantAction {
        action_id: String::new(),
        confirmation_token: String::new(),
        kind: route.kind,
        summary,
        risk: route.risk,
        endpoint,
        method,
        payload,
        requires_confirmation: true,
        preconditions: Vec::new(),
    })
}

pub(super) fn action_state_precondition(
    resource: &str,
    value: &Value,
    message: impl Into<String>,
) -> AssistantActionPrecondition {
    AssistantActionPrecondition {
        resource: resource.to_string(),
        expected_hash: stable_value_hash(value),
        message: message.into(),
    }
}

fn normalize_action_payload(
    method: &str,
    endpoint: &str,
    payload: Value,
    summary: &str,
) -> Result<Value, String> {
    if method == "DELETE" || payload.is_null() {
        return Ok(payload);
    }
    let mut payload = payload;
    repair_assistant_payload(&mut payload);
    let Some(map) = payload.as_object_mut() else {
        return Err("assistant action payload must be a JSON object".to_string());
    };

    for prefix in id_backed_collection_bases() {
        if endpoint == prefix && method == "POST" {
            ensure_object_field(map, "id", json!(""));
            ensure_object_field(map, "enabled", json!(true));
            ensure_rule_name(map, summary);
            ensure_location(map);
            return Ok(payload);
        }
        if let Some(id) = path_id(endpoint, prefix)
            && method == "PUT"
        {
            ensure_object_field(map, "id", json!(id));
            ensure_object_field(map, "enabled", json!(true));
            ensure_rule_name(map, summary);
            ensure_location(map);
            return Ok(payload);
        }
    }

    Ok(payload)
}

fn ensure_object_field(map: &mut serde_json::Map<String, Value>, key: &str, value: Value) {
    if !map.contains_key(key) {
        map.insert(key.to_string(), value);
    }
}

fn ensure_rule_name(map: &mut serde_json::Map<String, Value>, summary: &str) {
    if map.contains_key("name") || is_breakpoint_payload(map) {
        return;
    }
    let name = summary.trim();
    map.insert(
        "name".to_string(),
        json!(if name.is_empty() {
            "Assistant generated rule"
        } else {
            name
        }),
    );
}

fn ensure_location(map: &mut serde_json::Map<String, Value>) {
    if !map.contains_key("location") && !map.contains_key("url") && !map.contains_key("code") {
        map.insert("location".to_string(), json!({}));
    }
}

fn is_breakpoint_payload(map: &serde_json::Map<String, Value>) -> bool {
    map.contains_key("bp_type")
}

fn validate_action_payload_shape(
    method: &str,
    endpoint: &str,
    payload: &Value,
) -> Result<(), String> {
    if method == "DELETE" {
        return Ok(());
    }
    match (method, endpoint) {
        ("POST", "/admin/rule-sets") => require_payload::<RewriteRuleSet>(payload),
        ("POST", "/admin/map-remote-rules") => require_payload::<MapRemoteRule>(payload),
        ("POST", "/admin/map-local-rules") => require_payload::<MapLocalRule>(payload),
        ("POST", "/admin/access-rules") => require_payload::<AccessRule>(payload),
        ("POST", "/admin/breakpoints") => require_payload::<BreakpointRule>(payload),
        ("POST", "/admin/mock/rules") => require_payload::<MockRule>(payload),
        ("POST", "/admin/scripts") => require_payload::<LuaScript>(payload),
        ("POST", "/admin/webhooks") => require_payload::<WebhookConfig>(payload),
        ("POST", "/admin/throttling") => require_payload::<ThrottlingConfig>(payload),
        ("POST", "/admin/capture-filter") => require_payload::<CaptureFilterConfig>(payload),
        ("POST", "/admin/dns") => require_payload::<HashMap<String, String>>(payload),
        ("POST", "/admin/forward") => require_payload::<AssistantForwardReq>(payload),
        ("POST", "/admin/upstream-proxy") => {
            if !payload
                .get("upstream_proxy")
                .is_some_and(|value| value.is_string() || value.is_null())
            {
                return Err(
                    "invalid assistant action payload: upstream_proxy must be a string or null"
                        .to_string(),
                );
            }
            Ok(())
        }
        _ if method == "PUT" && path_id(endpoint, "/admin/rule-sets").is_some() => {
            require_payload::<RewriteRuleSet>(payload)
        }
        _ if method == "PUT" && path_id(endpoint, "/admin/map-remote-rules").is_some() => {
            require_payload::<MapRemoteRule>(payload)
        }
        _ if method == "PUT" && path_id(endpoint, "/admin/map-local-rules").is_some() => {
            require_payload::<MapLocalRule>(payload)
        }
        _ if method == "PUT" && path_id(endpoint, "/admin/access-rules").is_some() => {
            require_payload::<AccessRule>(payload)
        }
        _ if method == "PUT" && path_id(endpoint, "/admin/breakpoints").is_some() => {
            require_payload::<BreakpointRule>(payload)
        }
        _ if method == "PUT" && path_id(endpoint, "/admin/mock/rules").is_some() => {
            require_payload::<MockRule>(payload)
        }
        _ if method == "PUT" && path_id(endpoint, "/admin/scripts").is_some() => {
            require_payload::<LuaScript>(payload)
        }
        _ if method == "PUT" && path_id(endpoint, "/admin/webhooks").is_some() => {
            require_payload::<WebhookConfig>(payload)
        }
        _ => Ok(()),
    }
}

fn require_payload<T: for<'de> Deserialize<'de>>(payload: &Value) -> Result<(), String> {
    serde_json::from_value::<T>(payload.clone())
        .map(|_| ())
        .map_err(|e| format!("invalid assistant action payload: {e}"))
}

pub(super) fn propose_map_remote_action(args: Value) -> Result<AssistantAction, String> {
    let source = args
        .get("source_host")
        .and_then(Value::as_str)
        .ok_or_else(|| "propose_map_remote requires source_host".to_string())?;
    let destination = args
        .get("destination")
        .and_then(Value::as_str)
        .ok_or_else(|| "propose_map_remote requires destination".to_string())?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    build_map_remote_action(source, destination, name)
}

pub(super) fn deterministic_action_from_messages(
    messages: &[AssistantMessage],
) -> Result<Option<AssistantAction>, String> {
    let Some(text) = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
    else {
        return Ok(None);
    };
    deterministic_map_remote_action(text)
}

fn deterministic_map_remote_action(text: &str) -> Result<Option<AssistantAction>, String> {
    let re = regex::RegexBuilder::new(
        r"\b(?:map|route|proxy|forward)\b(?:\s+all\s+traffic|\s+traffic)?\s+from\s+(\S+)\s+to\s+(\S+)",
    )
    .case_insensitive(true)
    .build()
    .map_err(|e| format!("assistant map-remote parser failed: {e}"))?;
    let Some(caps) = re.captures(text) else {
        return Ok(None);
    };
    let source = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
    let destination = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
    build_map_remote_action(source, destination, None).map(Some)
}

fn build_map_remote_action(
    source_host: &str,
    destination: &str,
    name: Option<String>,
) -> Result<AssistantAction, String> {
    let source = normalize_source_host(source_host)?;
    let destination = normalize_destination_origin(destination)?;
    let (host, port) = split_host_port(&source);
    let summary = format!("Map all traffic from {source} to {destination}");
    let mut location = json!({
        "host": host,
        "mode": "glob"
    });
    if let Some(port) = port {
        location["port"] = json!(port);
    }
    let payload = json!({
        "id": "",
        "name": name.unwrap_or_else(|| format!("Map {source} to {destination}")),
        "enabled": true,
        "location": location,
        "destination": destination
    });

    let route = validate_action_route("POST", "/admin/map-remote-rules", &payload)?;
    Ok(AssistantAction {
        action_id: String::new(),
        confirmation_token: String::new(),
        kind: route.kind,
        summary,
        risk: route.risk,
        endpoint: "/admin/map-remote-rules".to_string(),
        method: "POST".to_string(),
        payload,
        requires_confirmation: true,
        preconditions: Vec::new(),
    })
}

fn normalize_source_host(input: &str) -> Result<String, String> {
    let cleaned = clean_host_token(input);
    if cleaned.is_empty() {
        return Err("source host is required".to_string());
    }
    if cleaned.starts_with("http://") || cleaned.starts_with("https://") {
        let url = reqwest::Url::parse(&cleaned).map_err(|e| format!("invalid source URL: {e}"))?;
        let host = url
            .host_str()
            .ok_or_else(|| "source URL must include a host".to_string())?;
        return Ok(match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        });
    }
    if cleaned.contains('/') {
        return normalize_source_host(&format!("http://{cleaned}"));
    }
    Ok(cleaned)
}

fn normalize_destination_origin(input: &str) -> Result<String, String> {
    let cleaned = clean_host_token(input);
    if cleaned.is_empty() {
        return Err("destination is required".to_string());
    }
    let candidate = if cleaned.starts_with("http://") || cleaned.starts_with("https://") {
        cleaned
    } else {
        format!("https://{cleaned}")
    };
    let url =
        reqwest::Url::parse(&candidate).map_err(|e| format!("invalid destination URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("destination must use http or https".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "destination URL must include a host".to_string())?;
    Ok(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

fn clean_host_token(input: &str) -> String {
    input
        .trim()
        .trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | ',' | ';' | '.' | '?'
            )
        })
        .to_string()
}

fn split_host_port(source: &str) -> (String, Option<u16>) {
    if let Some((host, port)) = source.rsplit_once(':')
        && !host.contains(':')
        && let Ok(port) = port.parse::<u16>()
    {
        return (host.to_string(), Some(port));
    }
    (source.to_string(), None)
}

pub(super) async fn execute_action_payload(
    state: &Arc<AppState>,
    action: &AssistantAction,
) -> Result<(Value, Vec<String>), String> {
    validate_action_route(&action.method, &action.endpoint, &action.payload)?;
    validate_action_preconditions(state, action).await?;
    let mut action = action.clone();
    action.payload = normalize_action_payload(
        &action.method,
        &action.endpoint,
        action.payload.clone(),
        &action.summary,
    )?;
    validate_action_payload_shape(&action.method, &action.endpoint, &action.payload)?;
    match (action.method.as_str(), action.endpoint.as_str()) {
        ("POST", "/admin/throttling") => {
            let cfg: ThrottlingConfig = from_payload(&action.payload)?;
            *state.throttling_config.write().await = cfg;
            let snapshot = state.throttling_config.read().await.clone();
            storage::save_throttle(&state.storage_path, &snapshot)
                .await
                .map_err(|e| e.to_string())?;
            Ok((
                json!({ "ok": true }),
                action_refresh_resources(&action, &["throttling"]),
            ))
        }
        ("POST", "/admin/capture-filter") => {
            let cfg: CaptureFilterConfig = from_payload(&action.payload)?;
            *state.capture_filter.write().await = cfg;
            let snapshot = state.capture_filter.read().await.clone();
            storage::save_capture_filter(&state.storage_path, &snapshot)
                .await
                .map_err(|e| e.to_string())?;
            Ok((
                json!({ "ok": true }),
                action_refresh_resources(&action, &["capture_filter"]),
            ))
        }
        ("POST", "/admin/dns") => {
            let map: HashMap<String, String> = from_payload(&action.payload)?;
            *state.dns_overrides.write().await = map;
            let snapshot = state.dns_overrides.read().await.clone();
            storage::save_dns_overrides(&state.storage_path, &snapshot)
                .await
                .map_err(|e| e.to_string())?;
            Ok((
                json!({ "ok": true }),
                action_refresh_resources(&action, &["dns"]),
            ))
        }
        ("POST", "/admin/upstream-proxy") => {
            let url = action
                .payload
                .get("upstream_proxy")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if let Some(ref candidate) = url
                && reqwest::Proxy::all(candidate).is_err()
            {
                return Err("invalid proxy URL".to_string());
            }
            storage::save_upstream_proxy(&state.storage_path, &url)
                .await
                .map_err(|e| e.to_string())?;
            state.proxy_engine.set_upstream_proxy(url.as_deref()).await;
            Ok((
                json!({ "ok": true, "upstream_proxy": url }),
                action_refresh_resources(&action, &["upstream_proxy"]),
            ))
        }
        ("DELETE", "/admin/sessions") => {
            state.api_handler.clear_sessions().await;
            Ok((
                json!({ "ok": true }),
                action_refresh_resources(&action, &["sessions"]),
            ))
        }
        ("POST", "/admin/playback") => {
            state.api_handler.start_playback().await;
            Ok((
                json!({ "ok": true }),
                action_refresh_resources(&action, &["sessions"]),
            ))
        }
        ("POST", "/admin/forward") => execute_forward_action(state, &action).await,
        _ => execute_collection_action(state, &action).await,
    }
}

async fn execute_collection_action(
    state: &Arc<AppState>,
    action: &AssistantAction,
) -> Result<(Value, Vec<String>), String> {
    if let Some(id) = action.endpoint.strip_prefix("/admin/dns/") {
        if action.method != "DELETE" {
            return Err("DNS item endpoint supports DELETE only".to_string());
        }
        let mut overrides = state.dns_overrides.write().await;
        if overrides.remove(id).is_none() {
            return Err("DNS override not found".to_string());
        }
        storage::save_dns_overrides(&state.storage_path, &overrides)
            .await
            .map_err(|e| e.to_string())?;
        return Ok((
            json!({ "ok": true }),
            action_refresh_resources(action, &["dns"]),
        ));
    }

    macro_rules! collection {
        ($prefix:literal, $field:ident, $ty:ty, $save:ident, $resource:literal, $new_id:path) => {{
            if action.endpoint == $prefix && action.method == "POST" {
                let mut item: $ty = from_payload(&action.payload)?;
                item.id = $new_id();
                let saved = item.clone();
                let mut items = state.$field.write().await;
                items.push(item);
                storage::$save(&state.storage_path, &items)
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok((json!(saved), action_refresh_resources(action, &[$resource])));
            }
            if let Some(id) = path_id(&action.endpoint, $prefix) {
                if action.method == "PUT" {
                    let mut item: $ty = from_payload(&action.payload)?;
                    item.id = id.to_string();
                    let mut items = state.$field.write().await;
                    let Some(slot) = items.iter_mut().find(|r| r.id == id) else {
                        return Err(format!("{} item not found", $resource));
                    };
                    *slot = item;
                    storage::$save(&state.storage_path, &items)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok((json!({ "ok": true }), action_refresh_resources(action, &[$resource])));
                }
                if action.method == "DELETE" {
                    let mut items = state.$field.write().await;
                    let before = items.len();
                    items.retain(|r| r.id != id);
                    if items.len() == before {
                        return Err(format!("{} item not found", $resource));
                    }
                    storage::$save(&state.storage_path, &items)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok((json!({ "ok": true }), action_refresh_resources(action, &[$resource])));
                }
            }
        }};
    }

    collection!(
        "/admin/rule-sets",
        rule_sets,
        RewriteRuleSet,
        save_rule_sets,
        "rule_sets",
        RewriteRuleSet::new_id
    );
    collection!(
        "/admin/map-remote-rules",
        map_remote_rules,
        MapRemoteRule,
        save_map_remote_rules,
        "map_remote",
        MapRemoteRule::new_id
    );
    collection!(
        "/admin/map-local-rules",
        map_local_rules,
        MapLocalRule,
        save_map_local_rules,
        "map_local",
        MapLocalRule::new_id
    );
    collection!(
        "/admin/access-rules",
        access_rules,
        AccessRule,
        save_access_rules,
        "access",
        AccessRule::new_id
    );

    execute_breakpoint_mock_script_or_webhook(state, action).await
}

async fn validate_action_preconditions(
    state: &Arc<AppState>,
    action: &AssistantAction,
) -> Result<(), String> {
    for precondition in &action.preconditions {
        let current = match precondition.resource.as_str() {
            "capture_filter" => json!(state.capture_filter.read().await.clone()),
            "dns" => json!(state.dns_overrides.read().await.clone()),
            "throttling" => json!(state.throttling_config.read().await.clone()),
            "upstream_proxy" => json!({
                "upstream_proxy": storage::load_upstream_proxy(&state.storage_path)
                    .or_else(|| state.config.upstream_proxy.clone())
            }),
            other => {
                return Err(format!(
                    "assistant action precondition references unsupported resource '{other}'"
                ));
            }
        };
        let current_hash = stable_value_hash(&current);
        if current_hash != precondition.expected_hash {
            return Err(format!(
                "assistant action precondition failed: {}",
                precondition.message
            ));
        }
    }
    Ok(())
}

async fn execute_breakpoint_mock_script_or_webhook(
    state: &Arc<AppState>,
    action: &AssistantAction,
) -> Result<(Value, Vec<String>), String> {
    if action.endpoint == "/admin/breakpoints" && action.method == "POST" {
        let mut rule: BreakpointRule = from_payload(&action.payload)?;
        rule.id = uuid::Uuid::new_v4().to_string();
        state.api_handler.add_breakpoint_rule(rule.clone()).await;
        storage::save_breakpoints(
            &state.storage_path,
            &state.api_handler.list_breakpoint_rules().await,
        )
        .await
        .map_err(|e| e.to_string())?;
        return Ok((
            json!(rule),
            action_refresh_resources(action, &["breakpoints"]),
        ));
    }
    if let Some(id) = path_id(&action.endpoint, "/admin/breakpoints") {
        if action.method == "PUT" {
            let mut rule: BreakpointRule = from_payload(&action.payload)?;
            rule.id = id.to_string();
            if !state.api_handler.update_breakpoint_rule(id, rule).await {
                return Err("breakpoint not found".to_string());
            }
        } else if action.method == "DELETE" {
            state.api_handler.delete_breakpoint_rule(id).await;
        } else {
            return Err("unsupported breakpoint method".to_string());
        }
        storage::save_breakpoints(
            &state.storage_path,
            &state.api_handler.list_breakpoint_rules().await,
        )
        .await
        .map_err(|e| e.to_string())?;
        return Ok((
            json!({ "ok": true }),
            action_refresh_resources(action, &["breakpoints"]),
        ));
    }

    if action.endpoint == "/admin/mock/rules" && action.method == "POST" {
        let mut rule: MockRule = from_payload(&action.payload)?;
        if rule.id.is_empty() {
            rule.id = uuid::Uuid::new_v4().to_string();
        }
        let saved = rule.clone();
        let mut rules = state.mock_rules.write().await;
        rules.push(rule);
        storage::save_mock_rules(&state.storage_path, &rules)
            .await
            .map_err(|e| e.to_string())?;
        return Ok((json!(saved), action_refresh_resources(action, &["mock"])));
    }
    if let Some(id) = path_id(&action.endpoint, "/admin/mock/rules") {
        let mut rules = state.mock_rules.write().await;
        if action.method == "PUT" {
            let updated: MockRule = from_payload(&action.payload)?;
            let Some(slot) = rules.iter_mut().find(|r| r.id == id) else {
                return Err("mock rule not found".to_string());
            };
            let call_count = slot.call_count;
            *slot = updated;
            slot.id = id.to_string();
            slot.call_count = call_count;
        } else if action.method == "DELETE" {
            let before = rules.len();
            rules.retain(|r| r.id != id);
            if rules.len() == before {
                return Err("mock rule not found".to_string());
            }
        } else {
            return Err("unsupported mock rule method".to_string());
        }
        storage::save_mock_rules(&state.storage_path, &rules)
            .await
            .map_err(|e| e.to_string())?;
        return Ok((
            json!({ "ok": true }),
            action_refresh_resources(action, &["mock"]),
        ));
    }

    if action.endpoint == "/admin/scripts" && action.method == "POST" {
        let mut script: LuaScript = from_payload(&action.payload)?;
        if script.id.is_empty() {
            script.id = uuid::Uuid::new_v4().to_string();
        }
        let saved = script.clone();
        let mut scripts = state.lua_scripts.write().await;
        scripts.push(script);
        storage::save_lua_scripts(&state.storage_path, &scripts)
            .await
            .map_err(|e| e.to_string())?;
        return Ok((
            json!(redact_value(&json!(saved))),
            action_refresh_resources(action, &["scripts"]),
        ));
    }
    if let Some(id) = path_id(&action.endpoint, "/admin/scripts") {
        let mut scripts = state.lua_scripts.write().await;
        if action.method == "PUT" {
            let mut updated: LuaScript = from_payload(&action.payload)?;
            updated.id = id.to_string();
            let Some(slot) = scripts.iter_mut().find(|s| s.id == id) else {
                return Err("script not found".to_string());
            };
            *slot = updated;
        } else if action.method == "DELETE" {
            let before = scripts.len();
            scripts.retain(|s| s.id != id);
            if scripts.len() == before {
                return Err("script not found".to_string());
            }
        } else {
            return Err("unsupported script method".to_string());
        }
        storage::save_lua_scripts(&state.storage_path, &scripts)
            .await
            .map_err(|e| e.to_string())?;
        return Ok((
            json!({ "ok": true }),
            action_refresh_resources(action, &["scripts"]),
        ));
    }

    execute_webhook_action(state, action).await
}

async fn execute_webhook_action(
    state: &Arc<AppState>,
    action: &AssistantAction,
) -> Result<(Value, Vec<String>), String> {
    if action.endpoint == "/admin/webhooks" && action.method == "POST" {
        let mut hook: WebhookConfig = from_payload(&action.payload)?;
        validate_webhook(state, &mut hook).await?;
        if hook.id.is_empty() {
            hook.id = uuid::Uuid::new_v4().to_string();
        }
        let saved = hook.clone();
        let mut hooks = state.webhooks.write().await;
        hooks.push(hook);
        storage::save_webhooks(&state.storage_path, &hooks)
            .await
            .map_err(|e| e.to_string())?;
        return Ok((
            json!(redact_value(&json!(saved))),
            action_refresh_resources(action, &["webhooks"]),
        ));
    }
    if let Some(id) = path_id(&action.endpoint, "/admin/webhooks") {
        let mut hooks = state.webhooks.write().await;
        if action.method == "PUT" {
            let mut updated: WebhookConfig = from_payload(&action.payload)?;
            validate_webhook(state, &mut updated).await?;
            updated.id = id.to_string();
            let Some(slot) = hooks.iter_mut().find(|h| h.id == id) else {
                return Err("webhook not found".to_string());
            };
            *slot = updated;
        } else if action.method == "DELETE" {
            let before = hooks.len();
            hooks.retain(|h| h.id != id);
            if hooks.len() == before {
                return Err("webhook not found".to_string());
            }
        } else {
            return Err("unsupported webhook method".to_string());
        }
        storage::save_webhooks(&state.storage_path, &hooks)
            .await
            .map_err(|e| e.to_string())?;
        return Ok((
            json!({ "ok": true }),
            action_refresh_resources(action, &["webhooks"]),
        ));
    }
    Err("assistant action endpoint is not executable".to_string())
}

async fn validate_webhook(state: &Arc<AppState>, hook: &mut WebhookConfig) -> Result<(), String> {
    sanitize_webhook_events(&mut hook.events);
    if hook.events.is_empty() {
        return Err("webhook must include request_captured or response_captured".to_string());
    }
    let url = reqwest::Url::parse(&hook.url).map_err(|e| format!("invalid webhook URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("unsupported webhook URL scheme: {}", url.scheme()));
    }
    enforce_admin_egress_policy(&url, AdminEgressPolicy::from_config(&state.config)).await
}

#[derive(Deserialize)]
struct AssistantForwardReq {
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

async fn execute_forward_action(
    state: &Arc<AppState>,
    action: &AssistantAction,
) -> Result<(Value, Vec<String>), String> {
    let req: AssistantForwardReq = from_payload(&action.payload)?;
    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|_| format!("invalid HTTP method: {}", req.method))?;
    let url = reqwest::Url::parse(&req.url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("unsupported URL scheme: {}", url.scheme()));
    }
    enforce_admin_egress_policy(&url, AdminEgressPolicy::from_config(&state.config)).await?;

    let mut builder = state
        .proxy_engine
        .http_client()
        .await
        .request(method, &req.url);
    for (name, value) in &req.headers {
        builder = builder.header(name, value);
    }
    if let Some(body) = req.body {
        builder = builder.body(body);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| format!("forward request failed: {}", redact_string(&e.to_string())))?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect::<HashMap<_, _>>();
    let bytes = response.bytes().await.unwrap_or_default();
    Ok((
        json!({
            "status": status,
            "headers": redact_value(&json!(headers)),
            "body_bytes": bytes.len(),
            "body_preview": redact_string(&String::from_utf8_lossy(&bytes).chars().take(500).collect::<String>()),
        }),
        action_refresh_resources(action, &["sessions"]),
    ))
}

fn validate_action_route(
    method: &str,
    endpoint: &str,
    payload: &Value,
) -> Result<AssistantActionRouteContract, String> {
    let route = action_route_contract(method, endpoint)?;
    if route.method != "DELETE" && payload.is_null() {
        return Err("assistant action payload is required".to_string());
    }
    Ok(route)
}

fn action_refresh_resources(action: &AssistantAction, fallback: &[&str]) -> Vec<String> {
    let resources = refreshed_resources_for_action(&action.method, &action.endpoint);
    if resources.is_empty() {
        fallback
            .iter()
            .map(|resource| (*resource).to_string())
            .collect()
    } else {
        resources
    }
}

#[cfg(test)]
fn classify_action_risk(method: &str, endpoint: &str) -> AssistantActionRisk {
    action_route_contract(method, endpoint)
        .map(|contract| contract.risk)
        .unwrap_or(AssistantActionRisk::Mutate)
}

impl AssistantAction {
    pub(super) fn risk_category(&self) -> &'static str {
        match self.risk {
            AssistantActionRisk::Mutate => "mutate",
            AssistantActionRisk::Network => "network",
            AssistantActionRisk::Destructive => "destructive",
        }
    }
}

fn from_payload<T: for<'de> Deserialize<'de>>(payload: &Value) -> Result<T, String> {
    serde_json::from_value(payload.clone())
        .map_err(|e| format!("invalid assistant action payload: {e}"))
}

fn stable_value_hash(value: &Value) -> String {
    let canonical = canonicalize_value(value);
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn canonicalize_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), canonicalize_value(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_value).collect()),
        _ => value.clone(),
    }
}

fn path_id<'a>(endpoint: &'a str, prefix: &str) -> Option<&'a str> {
    endpoint
        .strip_prefix(prefix)?
        .strip_prefix('/')
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_route_validation_is_allowlisted() {
        assert!(validate_action_route("POST", "/admin/rule-sets", &json!({})).is_ok());
        assert!(validate_action_route("DELETE", "/admin/rule-sets/abc", &Value::Null).is_ok());
        assert!(validate_action_route("POST", "/admin/unknown", &json!({})).is_err());
        assert!(validate_action_route("GET", "/admin/rule-sets", &json!({})).is_err());
    }

    #[test]
    fn propose_action_normalizes_create_payload_to_ui_shape() {
        let action = propose_action(json!({
            "method": "POST",
            "endpoint": "/admin/map-remote-rules",
            "summary": "Map api.test.com to Google",
            "payload": {
                "name": "Map api.test.com to Google",
                "enabled": true,
                "location": { "host": "api.test.com", "mode": "glob" },
                "destination": "https://google.com"
            }
        }))
        .expect("normalized action");

        assert_eq!(action.payload["id"], "");
        assert_eq!(action.payload["location"]["host"], "api.test.com");
        assert!(require_payload::<MapRemoteRule>(&action.payload).is_ok());
    }

    #[test]
    fn propose_action_rejects_payload_before_confirmation_when_ui_shape_is_invalid() {
        let err = propose_action(json!({
            "method": "POST",
            "endpoint": "/admin/map-remote-rules",
            "summary": "Map api.test.com",
            "payload": {
                "location": { "host": "api.test.com", "mode": "glob" }
            }
        }))
        .expect_err("invalid payload");

        assert!(err.contains("missing field `destination`"));
    }

    #[test]
    fn propose_action_coerces_known_numeric_strings_before_validation() {
        let throttling = propose_action(json!({
            "method": "POST",
            "endpoint": "/admin/throttling",
            "summary": "Set latency to 500 ms",
            "payload": {
                "enabled": true,
                "latency_ms": "500",
                "bandwidth_limit_kbps": "64"
            }
        }))
        .expect("numeric throttling payload");

        assert_eq!(throttling.payload["latency_ms"], 500);
        assert_eq!(throttling.payload["bandwidth_limit_kbps"], 64);
        assert!(require_payload::<ThrottlingConfig>(&throttling.payload).is_ok());

        let rule = propose_action(json!({
            "method": "POST",
            "endpoint": "/admin/rule-sets",
            "summary": "Set response status",
            "payload": {
                "id": "",
                "name": "Assistant status rule",
                "enabled": true,
                "location": { "path": "/api", "mode": "glob", "port": "443" },
                "applies_to": "response",
                "actions": [{ "type": "set_status", "code": "500" }]
            }
        }))
        .expect("numeric rule payload");

        assert_eq!(rule.payload["location"]["port"], 443);
        assert_eq!(rule.payload["actions"][0]["code"], 500);
        assert!(require_payload::<RewriteRuleSet>(&rule.payload).is_ok());
    }

    #[test]
    fn propose_action_coerces_known_boolean_strings_before_validation() {
        let rule = propose_action(json!({
            "method": "POST",
            "endpoint": "/admin/map-remote-rules",
            "summary": "Disable map remote rule",
            "payload": {
                "id": "",
                "name": "Disabled assistant map",
                "enabled": "false",
                "location": { "host": "api.test.com", "mode": "glob" },
                "destination": "https://example.com"
            }
        }))
        .expect("boolean map remote payload");

        assert_eq!(rule.payload["enabled"], false);
        assert!(require_payload::<MapRemoteRule>(&rule.payload).is_ok());

        let throttling = propose_action(json!({
            "method": "POST",
            "endpoint": "/admin/throttling",
            "summary": "Enable throttling",
            "payload": {
                "enabled": "true",
                "latency_ms": "250",
                "bandwidth_limit_kbps": "0"
            }
        }))
        .expect("boolean throttling payload");

        assert_eq!(throttling.payload["enabled"], true);
        assert!(require_payload::<ThrottlingConfig>(&throttling.payload).is_ok());
    }

    #[test]
    fn propose_action_repairs_single_rewrite_action_object_before_validation() {
        let rule = propose_action(json!({
            "method": "POST",
            "endpoint": "/admin/rule-sets",
            "summary": "Rewrite x-request-id for omniful.com",
            "payload": {
                "id": "",
                "name": "Rewrite x-request-id for omniful.com",
                "enabled": "true",
                "location": { "host": "omniful.com", "mode": "glob" },
                "applies_to": "request",
                "actions": {
                    "type": "set_header",
                    "name": "x-request-id",
                    "value": "1233"
                }
            }
        }))
        .expect("single action object should repair to action list");

        assert!(rule.payload["actions"].is_array());
        assert_eq!(rule.payload["actions"][0]["type"], "set_header");
        assert_eq!(rule.payload["actions"][0]["name"], "x-request-id");
        assert_eq!(rule.payload["actions"][0]["value"], "1233");
        assert!(require_payload::<RewriteRuleSet>(&rule.payload).is_ok());
    }

    #[test]
    fn deterministic_map_remote_request_builds_confirmed_action() {
        let action = deterministic_map_remote_action(
            "can you map all traffic from api.test.com to google.com",
        )
        .unwrap()
        .expect("map remote action");

        assert_eq!(action.method, "POST");
        assert_eq!(action.endpoint, "/admin/map-remote-rules");
        assert_eq!(action.payload["location"]["host"], "api.test.com");
        assert_eq!(action.payload["location"]["mode"], "glob");
        assert_eq!(action.payload["destination"], "https://google.com");
        assert!(action.requires_confirmation);
    }

    #[test]
    fn map_remote_parser_handles_ports_and_existing_scheme() {
        let action = deterministic_map_remote_action(
            "route traffic from http://api.test.com:8080/v1 to http://localhost:3000",
        )
        .unwrap()
        .expect("map remote action");

        assert_eq!(action.payload["location"]["host"], "api.test.com");
        assert_eq!(action.payload["location"]["port"], 8080);
        assert_eq!(action.payload["destination"], "http://localhost:3000");
    }

    #[test]
    fn action_risk_classification_marks_dangerous_work() {
        assert!(matches!(
            classify_action_risk("POST", "/admin/rule-sets"),
            AssistantActionRisk::Mutate
        ));
        assert!(matches!(
            classify_action_risk("POST", "/admin/forward"),
            AssistantActionRisk::Network
        ));
        assert!(matches!(
            classify_action_risk("DELETE", "/admin/rule-sets/abc"),
            AssistantActionRisk::Destructive
        ));
    }

    #[test]
    fn stable_value_hash_is_independent_of_object_key_order() {
        let left = json!({ "b": 2, "a": { "d": 4, "c": 3 } });
        let right = json!({ "a": { "c": 3, "d": 4 }, "b": 2 });

        assert_eq!(stable_value_hash(&left), stable_value_hash(&right));
    }
}
