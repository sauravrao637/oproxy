use axum::{extract::State, response::IntoResponse};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::plugins::access_control::AccessRule;
use crate::middleware::plugins::capture_filter::CaptureFilterConfig;
use crate::middleware::plugins::map_local::MapLocalRule;
use crate::middleware::plugins::map_remote::MapRemoteRule;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::middleware::plugins::rules::RewriteRuleSet;
use crate::storage;

use super::storage_error_response;

// ── Access control rules ───────────────────────────────────────────────────────

pub(super) async fn list_access_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.access_rules.read().await.clone())
}

pub(super) async fn create_access_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<AccessRule>,
) -> impl IntoResponse {
    rule.id = AccessRule::new_id();
    let saved = rule.clone();
    state.access_rules.write().await.push(rule);
    let rules = state.access_rules.read().await.clone();
    if let Err(e) = storage::save_access_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn update_access_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<AccessRule>,
) -> impl IntoResponse {
    rule.id = id.clone();
    {
        let mut rules = state.access_rules.write().await;
        match rules.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = rule,
            None => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    let rules = state.access_rules.read().await.clone();
    if let Err(e) = storage::save_access_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_access_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut rules = state.access_rules.write().await;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }
    let rules = state.access_rules.read().await.clone();
    if let Err(e) = storage::save_access_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Throttling ─────────────────────────────────────────────────────────────────

pub(super) async fn get_throttling(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.throttling_config.read().await.clone())
}

pub(super) async fn update_throttling(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_config): axum::extract::Json<ThrottlingConfig>,
) -> impl IntoResponse {
    let mut config = state.throttling_config.write().await;
    *config = new_config;
    if let Err(e) = storage::save_throttle(&state.storage_path, &config).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Unified rule sets ──────────────────────────────────────────────────────────

pub(super) async fn list_rule_sets(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.rule_sets.read().await.clone())
}

/// Reorder rule sets by accepting an ordered list of IDs.
/// Rules not in the list are appended at the end in their original order.
pub(super) async fn reorder_rule_sets(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> impl IntoResponse {
    let ids: Vec<String> = match body.get("ids").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    }) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({ "error": "ids array required" })),
            )
                .into_response();
        }
    };
    {
        let mut rules = state.rule_sets.write().await;
        let mut reordered: Vec<RewriteRuleSet> = ids
            .iter()
            .filter_map(|id| rules.iter().find(|r| &r.id == id).cloned())
            .collect();
        // Append any rules not mentioned in ids (preserve them at the end)
        for r in rules.iter() {
            if !ids.contains(&r.id) {
                reordered.push(r.clone());
            }
        }
        *rules = reordered;
    }
    let rules = state.rule_sets.read().await.clone();
    if let Err(e) = storage::save_rule_sets(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::Json(rules).into_response()
}

pub(super) async fn create_rule_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<RewriteRuleSet>,
) -> impl IntoResponse {
    rule.id = RewriteRuleSet::new_id();
    let saved = rule.clone();
    state.rule_sets.write().await.push(rule);
    let rules = state.rule_sets.read().await.clone();
    if let Err(e) = storage::save_rule_sets(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn get_rule_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let rules = state.rule_sets.read().await;
    match rules.iter().find(|r| r.id == id) {
        Some(r) => axum::Json(r.clone()).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn update_rule_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<RewriteRuleSet>,
) -> impl IntoResponse {
    rule.id = id.clone();
    {
        let mut rules = state.rule_sets.write().await;
        match rules.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = rule,
            None => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    let rules = state.rule_sets.read().await.clone();
    if let Err(e) = storage::save_rule_sets(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_rule_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut rules = state.rule_sets.write().await;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }
    let rules = state.rule_sets.read().await.clone();
    if let Err(e) = storage::save_rule_sets(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Map Local rules ────────────────────────────────────────────────────────────

pub(super) async fn list_map_local_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.map_local_rules.read().await.clone())
}

/// Directory where UI-uploaded Map Local fixtures are stored.
fn map_local_fixtures_dir(state: &AppState) -> std::path::PathBuf {
    state.storage_path.join("map-local")
}

/// Validate that a MapLocalRule's file_path exists and is accessible.
/// Returns an error response if it doesn't so operators discover path issues
/// at rule-save time instead of silently at request time.
///
/// Resolution mirrors the middleware exactly (mounted base path, then the
/// managed `storage/map-local/` fixtures directory).
fn validate_map_local_path(
    rule: &MapLocalRule,
    base_path: &Option<std::path::PathBuf>,
    fixtures_dir: &std::path::Path,
) -> Option<axum::response::Response> {
    if rule.file_path.trim().is_empty() {
        return Some(
            (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "file_path is required — set a file/directory path, upload a file, or paste fixture content"
                })),
            )
                .into_response(),
        );
    }
    // Validate the resolved path at rule creation so invalid rules return 422
    // instead of failing when matching traffic arrives.
    let path = crate::middleware::plugins::map_local::resolve_map_local_path(
        &rule.file_path,
        base_path.as_deref(),
        Some(fixtures_dir),
    );

    if !path.exists() {
        let hint = if base_path.is_some() && !rule.file_path.starts_with('/') {
            format!(
                "relative paths resolve from base path '{}' or the managed fixtures dir '{}'",
                base_path.as_ref().unwrap().display(),
                fixtures_dir.display()
            )
        } else if !rule.file_path.starts_with('/') {
            format!(
                "relative paths resolve from the managed fixtures dir '{}' — upload a file first",
                fixtures_dir.display()
            )
        } else {
            "In containerized deployments ensure the path is mounted inside the container."
                .to_string()
        };
        return Some(
            (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": format!(
                        "file_path '{}' does not exist or is not accessible from this process. {}",
                        rule.file_path, hint
                    )
                })),
            )
                .into_response(),
        );
    }
    None
}

/// Reduce a user-supplied fixture name to a single safe path component.
/// Rejects empty names, absolute paths, `..`, and any nested path so uploads
/// can never escape `storage/map-local/`.
fn sanitize_fixture_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut components = std::path::Path::new(trimmed).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(c)), None) => c.to_str().map(|s| s.to_string()),
        _ => None,
    }
}

/// If a rule was submitted with inline fixture content, write it to the managed
/// `storage/map-local/` directory under `file_path` (treated as a single file
/// name) and rewrite `file_path` to that name. Clears `inline_body` so it is
/// never persisted. Returns an error response if the name is unsafe or the write
/// fails. A no-op when `inline_body` is absent.
async fn materialize_inline_fixture(
    rule: &mut MapLocalRule,
    fixtures_dir: &std::path::Path,
) -> Result<(), axum::response::Response> {
    let Some(body) = rule.inline_body.take() else {
        return Ok(());
    };
    let Some(safe) = sanitize_fixture_name(&rule.file_path) else {
        return Err((
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({
                "error": "inline_body requires file_path to be a single fixture file name (no path separators or '..')"
            })),
        )
            .into_response());
    };
    if let Err(e) = tokio::fs::create_dir_all(fixtures_dir).await {
        return Err(storage_error_response(e));
    }
    if let Err(e) = tokio::fs::write(fixtures_dir.join(&safe), body.as_bytes()).await {
        return Err(storage_error_response(e));
    }
    rule.file_path = safe;
    Ok(())
}

#[derive(serde::Serialize)]
struct FixtureInfo {
    name: String,
    size: u64,
}

/// List managed Map Local fixtures available in `storage/map-local/`.
pub(super) async fn list_map_local_fixtures(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let dir = map_local_fixtures_dir(&state);
    let mut out: Vec<FixtureInfo> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let is_file = entry
                .file_type()
                .await
                .map(|ft| ft.is_file())
                .unwrap_or(false);
            if !is_file {
                continue;
            }
            let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
            if let Some(name) = entry.file_name().to_str() {
                out.push(FixtureInfo {
                    name: name.to_string(),
                    size,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    axum::Json(out).into_response()
}

/// Return the raw contents of a managed Map Local fixture so the UI can
/// repopulate the paste editor when editing a rule that references it.
pub(super) async fn get_map_local_fixture(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(safe) = sanitize_fixture_name(&name) else {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    };
    let path = map_local_fixtures_dir(&state).join(&safe);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        Err(e) => storage_error_response(e),
    }
}

/// Upload (create or overwrite) a managed Map Local fixture. The request body is
/// the raw file contents. The stored name is returned so the caller can set it as
/// a rule's `file_path` (a relative name resolved from `storage/map-local/`).
pub(super) async fn upload_map_local_fixture(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let Some(safe) = sanitize_fixture_name(&name) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "invalid fixture name: must be a single file name with no path separators or '..'"
            })),
        )
            .into_response();
    };
    let dir = map_local_fixtures_dir(&state);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return storage_error_response(e);
    }
    let path = dir.join(&safe);
    if let Err(e) = tokio::fs::write(&path, &body).await {
        return storage_error_response(e);
    }
    (
        axum::http::StatusCode::CREATED,
        axum::Json(serde_json::json!({ "name": safe, "size": body.len() })),
    )
        .into_response()
}

/// Delete a managed Map Local fixture from `storage/map-local/`.
pub(super) async fn delete_map_local_fixture(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(safe) = sanitize_fixture_name(&name) else {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    };
    let path = map_local_fixtures_dir(&state).join(&safe);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => axum::http::StatusCode::OK.into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        Err(e) => storage_error_response(e),
    }
}

pub(super) async fn create_map_local_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<MapLocalRule>,
) -> impl IntoResponse {
    rule.id = MapLocalRule::new_id();
    let fixtures_dir = map_local_fixtures_dir(&state);
    if let Err(err) = materialize_inline_fixture(&mut rule, &fixtures_dir).await {
        return err;
    }
    if let Some(err) =
        validate_map_local_path(&rule, &state.config.map_local_base_path, &fixtures_dir)
    {
        return err;
    }
    let saved = rule.clone();
    state.map_local_rules.write().await.push(rule);
    let rules = state.map_local_rules.read().await.clone();
    if let Err(e) = storage::save_map_local_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn update_map_local_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<MapLocalRule>,
) -> impl IntoResponse {
    rule.id = id.clone();
    let fixtures_dir = map_local_fixtures_dir(&state);
    if let Err(err) = materialize_inline_fixture(&mut rule, &fixtures_dir).await {
        return err;
    }
    if let Some(err) =
        validate_map_local_path(&rule, &state.config.map_local_base_path, &fixtures_dir)
    {
        return err;
    }
    {
        let mut rules = state.map_local_rules.write().await;
        match rules.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = rule,
            None => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    let rules = state.map_local_rules.read().await.clone();
    if let Err(e) = storage::save_map_local_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_map_local_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut rules = state.map_local_rules.write().await;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }
    let rules = state.map_local_rules.read().await.clone();
    if let Err(e) = storage::save_map_local_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── Map Remote rules ───────────────────────────────────────────────────────────

pub(super) async fn list_map_remote_rules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.map_remote_rules.read().await.clone())
}

pub(super) async fn create_map_remote_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(mut rule): axum::extract::Json<MapRemoteRule>,
) -> impl IntoResponse {
    rule.id = MapRemoteRule::new_id();
    let saved = rule.clone();
    state.map_remote_rules.write().await.push(rule);
    let rules = state.map_remote_rules.read().await.clone();
    if let Err(e) = storage::save_map_remote_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    (axum::http::StatusCode::CREATED, axum::Json(saved)).into_response()
}

pub(super) async fn update_map_remote_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Json(mut rule): axum::extract::Json<MapRemoteRule>,
) -> impl IntoResponse {
    rule.id = id.clone();
    {
        let mut rules = state.map_remote_rules.write().await;
        match rules.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = rule,
            None => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    let rules = state.map_remote_rules.read().await.clone();
    if let Err(e) = storage::save_map_remote_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_map_remote_rule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    {
        let mut rules = state.map_remote_rules.write().await;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }
    let rules = state.map_remote_rules.read().await.clone();
    if let Err(e) = storage::save_map_remote_rules(&state.storage_path, &rules).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

/// Quick connectivity probe for a Map Remote destination.
/// Fires a HEAD request (falling back to GET) and returns reachability info.
pub(super) async fn test_map_remote(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> impl IntoResponse {
    let destination = match body.get("destination").and_then(|v| v.as_str()) {
        Some(d) => d.trim_end_matches('/').to_string(),
        None => {
            return (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({ "error": "destination is required" })),
            )
                .into_response();
        }
    };
    let client = state.proxy_engine.http_client().await;
    let probe_url = format!("{destination}/");
    // Try HEAD first, fall back to GET
    let result = client
        .head(&probe_url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| reqwest::Client::new().get(&probe_url).build().unwrap_err());

    match result {
        Ok(resp) => axum::Json(serde_json::json!({
            "ok": true,
            "status": resp.status().as_u16(),
        }))
        .into_response(),
        Err(e) => axum::Json(serde_json::json!({
            "ok": false,
            "error": e.to_string(),
        }))
        .into_response(),
    }
}

// ── Capture filter ─────────────────────────────────────────────────────────────

pub(super) async fn get_capture_filter(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.capture_filter.read().await.clone())
}

pub(super) async fn update_capture_filter(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(new_cfg): axum::extract::Json<CaptureFilterConfig>,
) -> impl IntoResponse {
    let mut cfg = state.capture_filter.write().await;
    *cfg = new_cfg;
    if let Err(e) = storage::save_capture_filter(&state.storage_path, &cfg).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

// ── DNS overrides ──────────────────────────────────────────────────────────────

pub(super) async fn list_dns(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Return enabled entries as plain strings (backward-compat API contract);
    // disabled entries are returned as objects so their state is preserved.
    let overrides = state.dns_overrides.read().await;
    let out: std::collections::HashMap<String, serde_json::Value> = overrides
        .iter()
        .map(|(host, entry)| {
            let val = if entry.enabled {
                serde_json::Value::String(entry.ip.clone())
            } else {
                serde_json::json!({ "ip": entry.ip, "enabled": false })
            };
            (host.clone(), val)
        })
        .collect();
    axum::Json(out)
}

/// `POST /admin/dns` accepts whole-map replacement and additive single-entry
/// updates. A single-entry update does not replace the existing table.
/// entry is almost certainly not what a caller sending this shape means.
#[derive(Debug, serde::Deserialize)]
struct SingleDnsOverrideRequest {
    host: String,
    ip: String,
    #[serde(default)]
    enabled: Option<bool>,
}

pub(super) async fn update_dns(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Ok(single) = serde_json::from_value::<SingleDnsOverrideRequest>(body.clone())
        && !single.host.trim().is_empty()
    {
        let entry = crate::middleware::plugins::dns_override::DnsEntry {
            ip: single.ip,
            enabled: single.enabled.unwrap_or(true),
        };
        let mut overrides = state.dns_overrides.write().await;
        overrides.insert(single.host, entry);
        if let Err(e) = storage::save_dns_overrides(&state.storage_path, &overrides).await {
            return storage_error_response(e);
        }
        return axum::http::StatusCode::OK.into_response();
    }

    let new_map = match serde_json::from_value::<
        std::collections::HashMap<String, crate::middleware::plugins::dns_override::DnsValue>,
    >(body)
    {
        Ok(m) => m,
        Err(e) => {
            return (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": format!(
                        "expected either a single {{\"host\": ..., \"ip\": ..., \"enabled\": ...}} object, or a whole-map replace {{\"<host>\": {{\"ip\": ..., \"enabled\": ...}}, ...}}: {e}"
                    )
                })),
            )
                .into_response();
        }
    };

    let mut overrides = state.dns_overrides.write().await;
    *overrides = new_map
        .into_iter()
        .map(|(k, v)| (k, v.into_entry()))
        .collect();
    if let Err(e) = storage::save_dns_overrides(&state.storage_path, &overrides).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

/// Upsert a single DNS override entry by host key.  Unlike the bulk POST which
/// replaces the whole map, this is additive: existing entries for other hosts
/// are left untouched.
pub(super) async fn upsert_dns(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host): axum::extract::Path<String>,
    axum::extract::Json(entry): axum::extract::Json<
        crate::middleware::plugins::dns_override::DnsEntry,
    >,
) -> impl IntoResponse {
    let mut overrides = state.dns_overrides.write().await;
    overrides.insert(host, entry);
    if let Err(e) = storage::save_dns_overrides(&state.storage_path, &overrides).await {
        return storage_error_response(e);
    }
    axum::http::StatusCode::OK.into_response()
}

pub(super) async fn delete_dns(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut overrides = state.dns_overrides.write().await;
    if overrides.remove(&host).is_some() {
        if let Err(e) = storage::save_dns_overrides(&state.storage_path, &overrides).await {
            return storage_error_response(e);
        }
        axum::http::StatusCode::OK.into_response()
    } else {
        axum::http::StatusCode::NOT_FOUND.into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SingleDnsOverrideRequest, materialize_inline_fixture, sanitize_fixture_name,
        validate_map_local_path,
    };
    use crate::middleware::plugins::map_local::MapLocalRule;

    #[test]
    fn empty_file_path_is_rejected() {
        let rule = MapLocalRule {
            id: "x".into(),
            name: "n".into(),
            enabled: true,
            location: Default::default(),
            file_path: "   ".into(),
            inline_body: None,
        };
        let dir = std::env::temp_dir();
        assert!(
            validate_map_local_path(&rule, &None, &dir).is_some(),
            "blank file_path must not resolve to the fixtures directory itself"
        );
    }

    #[test]
    fn nonexistent_file_path_is_rejected_even_without_base_path() {
        let dir = std::env::temp_dir().join(format!("oml_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rule = MapLocalRule {
            id: "x".into(),
            name: "n".into(),
            enabled: true,
            location: Default::default(),
            file_path: "definitely-not-there.json".into(),
            inline_body: None,
        };
        assert!(
            validate_map_local_path(&rule, &None, &dir).is_some(),
            "a file_path that resolves to nothing must be rejected at creation time"
        );
    }

    #[test]
    fn existing_file_path_is_accepted() {
        let dir = std::env::temp_dir().join(format!("oml_present_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("users.json"), "{}").unwrap();
        let rule = MapLocalRule {
            id: "x".into(),
            name: "n".into(),
            enabled: true,
            location: Default::default(),
            file_path: "users.json".into(),
            inline_body: None,
        };
        assert!(
            validate_map_local_path(&rule, &None, &dir).is_none(),
            "a file_path that resolves to an existing managed fixture must be accepted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_dns_override_shape_parses() {
        let body = serde_json::json!({"host": "x", "ip": "127.0.0.1", "enabled": true});
        let parsed: SingleDnsOverrideRequest = serde_json::from_value(body).expect("should parse");
        assert_eq!(parsed.host, "x");
        assert_eq!(parsed.ip, "127.0.0.1");
        assert_eq!(parsed.enabled, Some(true));
    }

    #[test]
    fn single_dns_override_shape_enabled_is_optional() {
        let body = serde_json::json!({"host": "x", "ip": "127.0.0.1"});
        let parsed: SingleDnsOverrideRequest = serde_json::from_value(body).expect("should parse");
        assert_eq!(parsed.enabled, None);
    }

    /// The pre-existing whole-map replace shape must keep working unchanged.
    #[test]
    fn bulk_dns_map_shape_still_parses() {
        let body = serde_json::json!({"api.internal": {"ip": "10.0.0.5", "enabled": true}});
        let parsed: std::collections::HashMap<
            String,
            crate::middleware::plugins::dns_override::DnsValue,
        > = serde_json::from_value(body).expect("bulk map shape should still parse");
        assert_eq!(parsed.len(), 1);
    }

    #[tokio::test]
    async fn inline_body_is_written_and_file_path_rewritten() {
        let dir = std::env::temp_dir().join(format!("oml_inline_{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let mut rule = MapLocalRule {
            id: "x".into(),
            name: "n".into(),
            enabled: true,
            location: Default::default(),
            file_path: "users.json".into(),
            inline_body: Some("{\"ok\":true}".into()),
        };
        materialize_inline_fixture(&mut rule, &dir).await.unwrap();
        assert_eq!(rule.file_path, "users.json");
        assert!(rule.inline_body.is_none(), "inline_body must be cleared");
        let written = tokio::fs::read_to_string(dir.join("users.json"))
            .await
            .unwrap();
        assert_eq!(written, "{\"ok\":true}");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn inline_body_rejects_unsafe_file_path() {
        let dir = std::env::temp_dir().join(format!("oml_inline_bad_{}", std::process::id()));
        let mut rule = MapLocalRule {
            id: "x".into(),
            name: "n".into(),
            enabled: true,
            location: Default::default(),
            file_path: "../escape.json".into(),
            inline_body: Some("nope".into()),
        };
        assert!(materialize_inline_fixture(&mut rule, &dir).await.is_err());
    }

    #[tokio::test]
    async fn no_inline_body_is_a_noop() {
        let dir = std::env::temp_dir().join("oml_noop_should_not_be_created");
        let mut rule = MapLocalRule {
            id: "x".into(),
            name: "n".into(),
            enabled: true,
            location: Default::default(),
            file_path: "/abs/path.json".into(),
            inline_body: None,
        };
        materialize_inline_fixture(&mut rule, &dir).await.unwrap();
        assert_eq!(rule.file_path, "/abs/path.json");
        assert!(
            !dir.exists(),
            "no fixtures dir should be created when inline_body is absent"
        );
    }

    #[test]
    fn sanitize_accepts_plain_names() {
        assert_eq!(
            sanitize_fixture_name("users.json").as_deref(),
            Some("users.json")
        );
        assert_eq!(
            sanitize_fixture_name("  data.bin  ").as_deref(),
            Some("data.bin")
        );
    }

    #[test]
    fn sanitize_rejects_traversal_and_nesting() {
        assert!(sanitize_fixture_name("").is_none());
        assert!(sanitize_fixture_name("   ").is_none());
        assert!(sanitize_fixture_name("..").is_none());
        assert!(sanitize_fixture_name("../secret").is_none());
        assert!(sanitize_fixture_name("a/b.json").is_none());
        assert!(sanitize_fixture_name("/etc/passwd").is_none());
        assert!(sanitize_fixture_name("./x").is_none());
    }
}
