//! Model compatibility checker for the assistant.
//!
//! Real OpenAI-compatible providers (Ollama, vLLM, llama.cpp, hosted APIs)
//! vary in how completely they implement chat-completions + tool-calling. This
//! module measures how well a user-supplied model works with oproxy by:
//!
//!   1. running a few capability gates (text completion, tool calling), then
//!   2. replaying the golden eval suite (`assistant_eval_cases.yaml`) against
//!      the live model and scoring how often it selects the correct tool for a
//!      prompt — reported as a compatibility percentage.
//!
//! The probes are generic: no provider or model is special-cased.
//!
//! Side-effect free by construction: every probe only sends requests to the
//! configured provider and *inspects* the tool calls it returns. Tools are
//! never executed, so the checker creates no sessions, mutates no workspace or
//! config state, and registers no pending actions.

use std::time::Instant;

use axum::{Json, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::assistant::parse_tool_arguments;
use super::assistant_provider::{AssistantProviderConfig, OpenAiCompatibleProviderClient};
use super::assistant_registry::openai_tool_specs;

#[derive(Debug, Deserialize)]
pub(crate) struct AssistantCompatRequest {
    pub provider: AssistantProviderConfig,
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AssistantCompatReport {
    /// True when every *required* capability gate passes (the model can drive
    /// the assistant at all). Independent of the eval score.
    pub compatible: bool,
    /// Share of golden eval cases for which the model selected the correct
    /// tool, 0–100. `None` when the eval phase was skipped (gates failed).
    pub compatibility_percent: Option<u8>,
    pub evals_passed: usize,
    pub evals_total: usize,
    /// Mean round-trip latency across the eval prompts, in milliseconds. A model
    /// can be fully correct yet too slow to be usable, so this is reported
    /// alongside the score. `None` when the eval phase was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_eval_latency_ms: Option<u128>,
    pub model: String,
    pub base_url: String,
    pub summary: String,
    pub checks: Vec<CompatCheck>,
    pub evals: Vec<EvalResult>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CompatCheck {
    pub id: String,
    pub label: String,
    /// Required checks gate `compatible`; recommended ones only inform.
    pub required: bool,
    pub ok: bool,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u128>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EvalResult {
    pub id: String,
    pub prompt: String,
    pub expected_tool: String,
    pub called_tool: Option<String>,
    /// True only when the case scored 100 (right tool *and* all payload fields).
    pub ok: bool,
    /// Weighted 0–100 score: tool selection plus payload-field correctness.
    pub score_percent: u8,
    pub fields_matched: u32,
    pub fields_total: u32,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u128>,
}

impl CompatCheck {
    fn new(id: &str, label: &str, required: bool) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            required,
            ok: false,
            detail: String::new(),
            latency_ms: None,
        }
    }

    fn pass(mut self, detail: impl Into<String>, started: Instant) -> Self {
        self.ok = true;
        self.detail = detail.into();
        self.latency_ms = Some(started.elapsed().as_millis());
        self
    }

    fn fail(mut self, detail: impl Into<String>, started: Option<Instant>) -> Self {
        self.ok = false;
        self.detail = detail.into();
        self.latency_ms = started.map(|s| s.elapsed().as_millis());
        self
    }

    fn pass_config(mut self) -> Self {
        self.ok = true;
        self.detail = "Base URL, model, and credentials accepted.".to_string();
        self
    }
}

// ── Golden eval suite (shared with the offline executor test) ──────────────

#[derive(Debug, Deserialize)]
struct EvalSuite {
    cases: Vec<EvalCaseSpec>,
}

#[derive(Debug, Deserialize)]
struct EvalCaseSpec {
    id: String,
    prompt: String,
    expected_tool: String,
    /// Canonical tool inputs the model should produce for this prompt. Used to
    /// score payload-field correctness when the expected specialized tool is
    /// called.
    #[serde(default)]
    args: Value,
    #[serde(default)]
    expected_action: ExpectedActionSpec,
}

#[derive(Debug, Default, Deserialize)]
struct ExpectedActionSpec {
    #[serde(default)]
    endpoint: String,
    /// Normalized final payload; used to score field correctness when the model
    /// reaches the endpoint via the generic propose_action tool.
    #[serde(default)]
    payload: Value,
}

/// Share of a case's score awarded for selecting the correct tool; the rest is
/// earned by producing the correct payload fields.
const TOOL_SELECTION_WEIGHT: f64 = 0.5;

fn load_eval_suite() -> EvalSuite {
    serde_yaml::from_str(include_str!("assistant_eval_cases.yaml"))
        .expect("assistant eval cases manifest is valid YAML")
}

const PROBE_TOOL_NAME: &str = "oproxy_probe_echo";

fn probe_tool_spec() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": PROBE_TOOL_NAME,
            "description": "Compatibility probe. Echo the provided message back verbatim.",
            "parameters": {
                "type": "object",
                "required": ["message"],
                "properties": {
                    "message": { "type": "string", "description": "Text to echo back." }
                }
            }
        }
    })
}

pub(super) async fn check_assistant_compatibility(
    Json(req): Json<AssistantCompatRequest>,
) -> impl IntoResponse {
    Json(run_compatibility_probes(req).await)
}

async fn run_compatibility_probes(req: AssistantCompatRequest) -> AssistantCompatReport {
    let model = req.provider.model.clone();
    let base_url = req.provider.base_url.clone();
    let mut checks = Vec::new();

    // 1. Provider configuration — construct the same client the assistant uses.
    let config_check = CompatCheck::new("provider_config", "Provider configuration", true);
    let client = match OpenAiCompatibleProviderClient::new(req.provider, req.api_key) {
        Ok(client) => {
            checks.push(config_check.pass_config());
            client
        }
        Err(e) => {
            checks.push(config_check.fail(e, None));
            return finish(model, base_url, checks, Vec::new(), false);
        }
    };

    // 2. Plain text completion (tools disabled) — the forced-final-answer path.
    let started = Instant::now();
    let text_check = CompatCheck::new("text_completion", "Plain text completion", true);
    match client.chat_completion_text_only(&plain_messages()).await {
        Ok(message) if !message.content.trim().is_empty() => {
            checks.push(text_check.pass(
                format!(
                    "Model returned text ({} chars).",
                    message.content.trim().len()
                ),
                started,
            ));
        }
        Ok(_) => {
            checks.push(text_check.fail(
                "Model accepted the request but returned empty content.",
                Some(started),
            ));
            return finish(model, base_url, checks, Vec::new(), false);
        }
        Err(e) => {
            checks.push(text_check.fail(e, Some(started)));
            return finish(model, base_url, checks, Vec::new(), false);
        }
    }

    let primary_check =
        CompatCheck::new("context_primary_priority", "Primary context priority", true);
    let started = Instant::now();
    match client
        .chat_completion_text_only(&context_primary_priority_messages())
        .await
    {
        Ok(message) if primary_priority_response_ok(&message.content) => {
            checks.push(primary_check.pass(
                "Model answered from primary context despite conflicting secondary UI state.",
                started,
            ));
        }
        Ok(message) => {
            checks.push(primary_check.fail(
                format!(
                    "Model did not clearly prioritize primary context. Response: {}",
                    compact_probe_response(&message.content)
                ),
                Some(started),
            ));
            return finish(model, base_url, checks, Vec::new(), false);
        }
        Err(e) => {
            checks.push(primary_check.fail(e, Some(started)));
            return finish(model, base_url, checks, Vec::new(), false);
        }
    }

    let fallback_check = CompatCheck::new(
        "context_secondary_fallback",
        "Secondary context fallback",
        true,
    );
    let started = Instant::now();
    match client
        .chat_completion_text_only(&context_secondary_fallback_messages())
        .await
    {
        Ok(message) if secondary_fallback_response_ok(&message.content) => {
            checks.push(fallback_check.pass(
                "Model used secondary context when primary context was insufficient.",
                started,
            ));
        }
        Ok(message) => {
            checks.push(fallback_check.fail(
                format!(
                    "Model did not clearly use secondary context as fallback. Response: {}",
                    compact_probe_response(&message.content)
                ),
                Some(started),
            ));
            return finish(model, base_url, checks, Vec::new(), false);
        }
        Err(e) => {
            checks.push(fallback_check.fail(e, Some(started)));
            return finish(model, base_url, checks, Vec::new(), false);
        }
    }

    // 3. Tool calling — does the model emit a tool call at all?
    let started = Instant::now();
    let tool_check = CompatCheck::new("tool_calling", "Tool calling", true);
    let probe_tools = [probe_tool_spec()];
    let tool_calling_ok = match client
        .chat_completion(&tool_request_messages(), &probe_tools)
        .await
    {
        Ok(message) if first_named_tool_call(&message.tool_calls, PROBE_TOOL_NAME).is_some() => {
            checks.push(tool_check.pass("Model emitted a tool call.", started));
            true
        }
        Ok(_) => {
            checks.push(tool_check.fail(
                "Model did not emit a tool call when explicitly instructed; it cannot drive read/action tools.",
                Some(started),
            ));
            false
        }
        Err(e) => {
            checks.push(tool_check.fail(
                format!("Tool-calling request failed (model may not support `tools`): {e}"),
                Some(started),
            ));
            false
        }
    };

    if !tool_calling_ok {
        // Eval would score 0; skip it and report the gate failure.
        return finish(model, base_url, checks, Vec::new(), false);
    }

    // 4. Golden eval suite — replay each prompt and score tool selection.
    let evals = run_eval_suite(&client).await;
    finish(model, base_url, checks, evals, true)
}

async fn run_eval_suite(client: &OpenAiCompatibleProviderClient) -> Vec<EvalResult> {
    let suite = load_eval_suite();
    let tools = openai_tool_specs();
    let mut results = Vec::with_capacity(suite.cases.len());

    for case in suite.cases {
        let started = Instant::now();
        let messages = eval_messages(&case.prompt);
        let result = match client.chat_completion(&messages, &tools).await {
            Ok(message) => score_eval_case(&case, &message.tool_calls, started),
            Err(e) => EvalResult {
                id: case.id,
                prompt: case.prompt,
                expected_tool: case.expected_tool,
                called_tool: None,
                ok: false,
                score_percent: 0,
                fields_matched: 0,
                fields_total: 0,
                detail: format!("Provider request failed: {e}"),
                latency_ms: Some(started.elapsed().as_millis()),
            },
        };
        results.push(result);
    }
    results
}

/// Score a case on two axes: did the model pick the right tool, and did it fill
/// in the right payload fields. The correct specialized tool (or the generic
/// `propose_action` aimed at the right endpoint) earns `TOOL_SELECTION_WEIGHT`;
/// the remainder is the fraction of expected payload leaves the model produced.
/// A wrong tool scores 0 — its arguments are meaningless.
fn score_eval_case(case: &EvalCaseSpec, tool_calls: &[Value], started: Instant) -> EvalResult {
    let latency = Some(started.elapsed().as_millis());
    let called_tool = tool_calls
        .first()
        .and_then(|call| call.pointer("/function/name").and_then(Value::as_str))
        .map(str::to_string);

    // Resolve which (expected fields, actual fields) to compare, and whether the
    // tool selection itself was correct.
    let (tool_ok, expected_fields, actual_fields, tool_detail) = match &called_tool {
        None => (
            false,
            Value::Null,
            Value::Null,
            "Model returned no tool call.".to_string(),
        ),
        Some(name) if name == &case.expected_tool => {
            let actual = tool_calls
                .first()
                .and_then(|call| parse_tool_arguments(call).ok())
                .unwrap_or(Value::Null);
            (
                true,
                case.args.clone(),
                actual,
                format!("Called expected tool `{name}`."),
            )
        }
        Some(name) if name == "propose_action" && !case.expected_action.endpoint.is_empty() => {
            let args = first_named_tool_call(tool_calls, "propose_action")
                .and_then(|call| parse_tool_arguments(&call).ok())
                .unwrap_or(Value::Null);
            let endpoint = args
                .get("endpoint")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if endpoint == case.expected_action.endpoint {
                let actual_payload = args.get("payload").cloned().unwrap_or(Value::Null);
                (
                    true,
                    case.expected_action.payload.clone(),
                    actual_payload,
                    format!("Called propose_action targeting {endpoint} (equivalent)."),
                )
            } else {
                (
                    false,
                    Value::Null,
                    Value::Null,
                    format!(
                        "Called propose_action targeting `{endpoint}`, expected `{}` (or tool `{}`).",
                        case.expected_action.endpoint, case.expected_tool
                    ),
                )
            }
        }
        Some(name) => (
            false,
            Value::Null,
            Value::Null,
            format!("Called `{name}`, expected `{}`.", case.expected_tool),
        ),
    };

    let (fields_matched, fields_total) = if tool_ok {
        field_score(&expected_fields, &actual_fields)
    } else {
        (0, 0)
    };
    let field_ratio = if fields_total > 0 {
        fields_matched as f64 / fields_total as f64
    } else {
        1.0 // nothing to fill in — full field credit
    };
    let score = if tool_ok {
        TOOL_SELECTION_WEIGHT + (1.0 - TOOL_SELECTION_WEIGHT) * field_ratio
    } else {
        0.0
    };
    let score_percent = (score * 100.0).round() as u8;

    let detail = if !tool_ok {
        tool_detail
    } else if fields_total == 0 {
        format!("{tool_detail} No payload fields to check.")
    } else {
        format!("{tool_detail} Payload fields {fields_matched}/{fields_total} correct.")
    };

    EvalResult {
        id: case.id.clone(),
        prompt: case.prompt.clone(),
        expected_tool: case.expected_tool.clone(),
        called_tool,
        ok: score_percent == 100,
        score_percent,
        fields_matched,
        fields_total,
        detail,
        latency_ms: latency,
    }
}

/// Count matched vs total *leaf* fields of `expected`, looked up by the same
/// path in `actual` (expected ⊆ actual). Values compare leniently so that
/// formatting differences the executor would normalize anyway (case, number vs
/// numeric string, boolean synonyms like `yes`/`true`) still count as correct.
fn field_score(expected: &Value, actual: &Value) -> (u32, u32) {
    match expected {
        Value::Object(map) => {
            let mut matched = 0;
            let mut total = 0;
            for (key, expected_value) in map {
                let actual_value = actual.get(key).cloned().unwrap_or(Value::Null);
                let (m, t) = field_score(expected_value, &actual_value);
                matched += m;
                total += t;
            }
            (matched, total)
        }
        Value::Array(items) => {
            let mut matched = 0;
            let mut total = 0;
            for (idx, expected_value) in items.iter().enumerate() {
                let actual_value = actual.get(idx).cloned().unwrap_or(Value::Null);
                let (m, t) = field_score(expected_value, &actual_value);
                matched += m;
                total += t;
            }
            (matched, total)
        }
        _ => {
            if leaf_equal(expected, actual) {
                (1, 1)
            } else {
                (0, 1)
            }
        }
    }
}

fn leaf_equal(expected: &Value, actual: &Value) -> bool {
    let norm = |value: &Value| -> String {
        match value {
            Value::String(s) => s.trim().to_ascii_lowercase(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Null => "null".to_string(),
            other => other.to_string(),
        }
    };
    let e = norm(expected);
    let a = norm(actual);
    if e == a {
        return true;
    }
    let truthy = |s: &str| matches!(s, "true" | "yes" | "on" | "1");
    let falsy = |s: &str| matches!(s, "false" | "no" | "off" | "0");
    (truthy(&e) && truthy(&a)) || (falsy(&e) && falsy(&a))
}

fn finish(
    model: String,
    base_url: String,
    checks: Vec<CompatCheck>,
    evals: Vec<EvalResult>,
    ran_evals: bool,
) -> AssistantCompatReport {
    let compatible = checks
        .iter()
        .filter(|check| check.required)
        .all(|check| check.ok);

    let evals_total = evals.len();
    let evals_passed = evals.iter().filter(|eval| eval.ok).count();
    let latencies: Vec<u128> = evals.iter().filter_map(|eval| eval.latency_ms).collect();
    let avg_eval_latency_ms = if latencies.is_empty() {
        None
    } else {
        Some(latencies.iter().sum::<u128>() / latencies.len() as u128)
    };
    // Compatibility is the mean weighted score across cases (tool selection +
    // payload-field correctness), not a simple right-tool pass rate.
    let compatibility_percent = if ran_evals && evals_total > 0 {
        let sum: u32 = evals.iter().map(|eval| eval.score_percent as u32).sum();
        Some((sum as f64 / evals_total as f64).round() as u8)
    } else if ran_evals {
        Some(100)
    } else {
        None
    };

    let summary = if !compatible {
        let failed = checks
            .iter()
            .filter(|check| check.required && !check.ok)
            .map(|check| check.label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("Not compatible. Failing required checks: {failed}.")
    } else if let Some(percent) = compatibility_percent {
        let latency = avg_eval_latency_ms
            .map(|ms| format!(" Avg latency {ms} ms/prompt."))
            .unwrap_or_default();
        format!(
            "{percent}% compatible — weighted across tool selection and payload-field correctness; {evals_passed} of {evals_total} eval cases fully correct.{latency}"
        )
    } else {
        "Required capability checks passed.".to_string()
    };

    AssistantCompatReport {
        compatible,
        compatibility_percent,
        evals_passed,
        evals_total,
        avg_eval_latency_ms,
        model,
        base_url,
        summary,
        checks,
        evals,
    }
}

fn first_named_tool_call(tool_calls: &[Value], name: &str) -> Option<Value> {
    tool_calls
        .iter()
        .find(|call| call.pointer("/function/name").and_then(Value::as_str) == Some(name))
        .cloned()
}

fn plain_messages() -> Vec<Value> {
    vec![
        json!({ "role": "system", "content": "You are a connectivity probe. Answer in one short sentence." }),
        json!({ "role": "user", "content": "Reply with a brief confirmation that you are reachable." }),
    ]
}

fn tool_request_messages() -> Vec<Value> {
    vec![
        json!({
            "role": "system",
            "content": "You are a tool-use probe. When a tool is available, call it instead of answering in prose."
        }),
        json!({
            "role": "user",
            "content": format!("Call the {PROBE_TOOL_NAME} function with message set to the word ping.")
        }),
    ]
}

fn context_priority_system_prompt() -> &'static str {
    "You are an oproxy assistant compatibility probe. Follow this context policy exactly: \
     primary context is the user-intentionally attached request. Secondary context is UI/workspace \
     state, visible sessions, and browser hints. If primary context is sufficient, answer only from \
     primary even when secondary conflicts. If primary context is absent or insufficient, use \
     secondary context as fallback. Answer with only the requested host and path."
}

fn context_primary_priority_messages() -> Vec<Value> {
    vec![
        json!({ "role": "system", "content": context_priority_system_prompt() }),
        json!({
            "role": "user",
            "content": "Primary context: attached request host primary.example.com path /primary. \
            Secondary UI state: selected request host secondary.example.com path /secondary. \
            The user asks: explain this request. Which host and path should be treated as this request?"
        }),
    ]
}

fn context_secondary_fallback_messages() -> Vec<Value> {
    vec![
        json!({ "role": "system", "content": context_priority_system_prompt() }),
        json!({
            "role": "user",
            "content": "Primary context: attached request id req-1, but no host, path, method, or flow details are available. \
            Secondary UI state: visible selected request host fallback.example.com path /secondary. \
            The user asks: explain this request. Primary is insufficient. Which host and path can be used as fallback?"
        }),
    ]
}

fn primary_priority_response_ok(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    normalized.contains("primary.example.com")
        && normalized.contains("/primary")
        && !normalized.contains("secondary.example.com")
}

fn secondary_fallback_response_ok(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    normalized.contains("fallback.example.com") && normalized.contains("/secondary")
}

fn compact_probe_response(content: &str) -> String {
    let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated = compact.chars().take(160).collect::<String>();
    if compact.chars().count() > 160 {
        format!("{truncated}...")
    } else {
        compact
    }
}

/// Eval system prompt mirrors the real assistant's tool-routing guidance so the
/// score reflects production tool selection. It is deliberately generic.
fn eval_messages(prompt: &str) -> Vec<Value> {
    vec![
        json!({
            "role": "system",
            "content": "You are the oproxy assistant. Select the single most appropriate tool for the user's request and call it. \
        Prefer specialized tools over the generic propose_action: propose_map_remote for host-to-host mapping, propose_dns_override for DNS, \
        propose_throttling for throttling, propose_rewrite_rule for header/query/path/body/status rewrites, propose_mock_rule for mock responses, \
        propose_access_rule for block/allow, propose_capture_filter for capture filtering, and propose_upstream_proxy for upstream proxy changes. \
        Call exactly one tool; do not answer in prose."
        }),
        json!({ "role": "user", "content": prompt }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(id: &str, expected: &str, called: Option<&str>, score_percent: u8) -> EvalResult {
        EvalResult {
            id: id.into(),
            prompt: "p".into(),
            expected_tool: expected.into(),
            called_tool: called.map(str::to_string),
            ok: score_percent == 100,
            score_percent,
            fields_matched: 0,
            fields_total: 0,
            detail: String::new(),
            latency_ms: Some(1),
        }
    }

    #[test]
    fn eval_cases_yaml_loads_and_is_nonempty() {
        let suite = load_eval_suite();
        assert!(!suite.cases.is_empty());
        assert!(
            suite
                .cases
                .iter()
                .all(|case| !case.prompt.is_empty() && !case.expected_tool.is_empty())
        );
    }

    #[test]
    fn context_priority_probe_messages_exercise_primary_and_fallback() {
        let primary_messages = context_primary_priority_messages();
        let primary_user = primary_messages[1]["content"].as_str().unwrap();
        assert!(primary_user.contains("primary.example.com"));
        assert!(primary_user.contains("secondary.example.com"));
        assert!(primary_priority_response_ok("primary.example.com /primary"));
        assert!(!primary_priority_response_ok(
            "secondary.example.com /secondary"
        ));

        let fallback_messages = context_secondary_fallback_messages();
        let fallback_user = fallback_messages[1]["content"].as_str().unwrap();
        assert!(fallback_user.contains("Primary is insufficient"));
        assert!(fallback_user.contains("fallback.example.com"));
        assert!(secondary_fallback_response_ok(
            "fallback.example.com /secondary"
        ));
        assert!(!secondary_fallback_response_ok(
            "primary.example.com /primary"
        ));
    }

    fn dns_case() -> EvalCaseSpec {
        EvalCaseSpec {
            id: "x".into(),
            prompt: "resolve api.test.com to 10.0.0.5".into(),
            expected_tool: "propose_dns_override".into(),
            args: json!({ "operation": "set", "host": "api.test.com", "ip": "10.0.0.5" }),
            expected_action: ExpectedActionSpec {
                endpoint: "/admin/dns".into(),
                payload: Value::Null,
            },
        }
    }

    #[test]
    fn right_tool_all_fields_scores_100() {
        let calls = vec![json!({
            "function": {
                "name": "propose_dns_override",
                "arguments": "{\"operation\":\"set\",\"host\":\"api.test.com\",\"ip\":\"10.0.0.5\"}"
            }
        })];
        let result = score_eval_case(&dns_case(), &calls, Instant::now());
        assert_eq!(result.score_percent, 100, "{}", result.detail);
        assert!(result.ok);
        assert_eq!(result.fields_matched, 3);
        assert_eq!(result.fields_total, 3);
    }

    #[test]
    fn right_tool_partial_fields_scores_between_weight_and_100() {
        // Right tool (0.5) + 1 of 3 fields correct (host only) => 0.5 + 0.5*(1/3) ≈ 0.667.
        let calls = vec![json!({
            "function": {
                "name": "propose_dns_override",
                "arguments": "{\"host\":\"api.test.com\"}"
            }
        })];
        let result = score_eval_case(&dns_case(), &calls, Instant::now());
        assert!(!result.ok);
        assert_eq!(result.fields_matched, 1);
        assert_eq!(result.fields_total, 3);
        assert!(
            result.score_percent > 50 && result.score_percent < 100,
            "score was {}",
            result.score_percent
        );
    }

    #[test]
    fn right_tool_no_fields_scores_only_tool_weight() {
        // Right tool, zero matching fields => exactly the tool-selection weight.
        let calls = vec![json!({
            "function": { "name": "propose_dns_override", "arguments": "{}" }
        })];
        let result = score_eval_case(&dns_case(), &calls, Instant::now());
        assert_eq!(result.score_percent, 50, "{}", result.detail);
    }

    #[test]
    fn lenient_leaf_compare_handles_case_numbers_and_booleans() {
        assert!(leaf_equal(&json!("API.TEST.COM"), &json!("api.test.com")));
        assert!(leaf_equal(&json!(500), &json!("500")));
        assert!(leaf_equal(&json!(true), &json!("yes")));
        assert!(!leaf_equal(&json!("get"), &json!("post")));
    }

    #[test]
    fn score_accepts_propose_action_with_matching_endpoint() {
        let case = EvalCaseSpec {
            id: "x".into(),
            prompt: "resolve a to b".into(),
            expected_tool: "propose_dns_override".into(),
            args: Value::Null,
            expected_action: ExpectedActionSpec {
                endpoint: "/admin/dns".into(),
                payload: Value::Null,
            },
        };
        let calls = vec![json!({
            "function": { "name": "propose_action", "arguments": "{\"endpoint\":\"/admin/dns\"}" }
        })];
        let result = score_eval_case(&case, &calls, Instant::now());
        assert!(result.ok, "{}", result.detail);
    }

    #[test]
    fn score_fails_on_wrong_tool() {
        let calls =
            vec![json!({ "function": { "name": "propose_throttling", "arguments": "{}" } })];
        let result = score_eval_case(&dns_case(), &calls, Instant::now());
        assert_eq!(result.score_percent, 0);
        assert!(!result.ok);
        assert_eq!(result.called_tool.as_deref(), Some("propose_throttling"));
    }

    #[test]
    fn percentage_and_summary_reflect_eval_pass_rate() {
        let checks = vec![CompatCheck {
            id: "tool_calling".into(),
            label: "Tool calling".into(),
            required: true,
            ok: true,
            detail: String::new(),
            latency_ms: Some(1),
        }];
        // Mean of weighted scores: (100 + 100 + 0 + 50) / 4 = 62.5 -> 63.
        let evals = vec![
            eval("a", "t", Some("t"), 100),
            eval("b", "t", Some("t"), 100),
            eval("c", "t", Some("x"), 0),
            eval("d", "t", Some("t"), 50),
        ];
        let report = finish("m".into(), "u".into(), checks, evals, true);
        assert!(report.compatible);
        assert_eq!(report.compatibility_percent, Some(63));
        assert_eq!(report.evals_passed, 2); // only the two 100s are "fully correct"
        assert_eq!(report.avg_eval_latency_ms, Some(1)); // eval() stubs latency_ms = 1
        assert!(report.summary.contains("63%"));
        assert!(report.summary.contains("Avg latency"));
    }

    #[test]
    fn percentage_is_none_when_gate_fails() {
        let checks = vec![CompatCheck {
            id: "text_completion".into(),
            label: "Plain text completion".into(),
            required: true,
            ok: false,
            detail: "boom".into(),
            latency_ms: Some(1),
        }];
        let report = finish("m".into(), "u".into(), checks, Vec::new(), false);
        assert!(!report.compatible);
        assert_eq!(report.compatibility_percent, None);
        assert!(report.summary.contains("Not compatible"));
    }
}
