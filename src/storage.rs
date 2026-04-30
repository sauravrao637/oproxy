use std::collections::HashMap;
use std::path::Path;
use crate::middleware::plugins::breakpoints::BreakpointRule;
use crate::middleware::plugins::header_map::HeaderMapRule;
use crate::middleware::plugins::modification::ModificationRule;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::middleware::plugins::rewrite::RewriteRule;

pub fn load_routes(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path.join("routes.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_routes(path: &Path, routes: &HashMap<String, String>) {
    if let Err(e) = std::fs::write(
        path.join("routes.json"),
        serde_json::to_string_pretty(routes).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist routes");
    }
}

pub fn load_rewrites(path: &Path) -> Vec<RewriteRule> {
    std::fs::read_to_string(path.join("rewrites.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_rewrites(path: &Path, rules: &[RewriteRule]) {
    if let Err(e) = std::fs::write(
        path.join("rewrites.json"),
        serde_json::to_string_pretty(rules).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist rewrites");
    }
}

pub fn load_throttle(path: &Path) -> ThrottlingConfig {
    std::fs::read_to_string(path.join("throttle.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_else(|| ThrottlingConfig { latency_ms: 0, bandwidth_limit_kbps: 0, enabled: false })
}

pub fn save_throttle(path: &Path, config: &ThrottlingConfig) {
    if let Err(e) = std::fs::write(
        path.join("throttle.json"),
        serde_json::to_string_pretty(config).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist throttle config");
    }
}

pub fn load_dns_overrides(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path.join("dns_overrides.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_dns_overrides(path: &Path, map: &HashMap<String, String>) {
    if let Err(e) = std::fs::write(
        path.join("dns_overrides.json"),
        serde_json::to_string_pretty(map).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist DNS overrides");
    }
}

pub fn load_breakpoints(path: &Path) -> Vec<BreakpointRule> {
    std::fs::read_to_string(path.join("breakpoints.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_breakpoints(path: &Path, rules: &[BreakpointRule]) {
    if let Err(e) = std::fs::write(
        path.join("breakpoints.json"),
        serde_json::to_string_pretty(rules).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist breakpoint rules");
    }
}

pub fn load_header_maps(path: &Path) -> Vec<HeaderMapRule> {
    std::fs::read_to_string(path.join("header_maps.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_header_maps(path: &Path, rules: &[HeaderMapRule]) {
    if let Err(e) = std::fs::write(
        path.join("header_maps.json"),
        serde_json::to_string_pretty(rules).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist header map rules");
    }
}

pub fn load_modifications(path: &Path) -> Vec<ModificationRule> {
    std::fs::read_to_string(path.join("modifications.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_modifications(path: &Path, rules: &[ModificationRule]) {
    if let Err(e) = std::fs::write(
        path.join("modifications.json"),
        serde_json::to_string_pretty(rules).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist modification rules");
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct HotConfig {
    pub max_body_bytes: Option<usize>,
}

pub fn load_hot_config(path: &Path) -> HotConfig {
    std::fs::read_to_string(path.join("hot_config.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_hot_config(path: &Path, cfg: &HotConfig) {
    if let Err(e) = std::fs::write(
        path.join("hot_config.json"),
        serde_json::to_string_pretty(cfg).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist hot config");
    }
}

pub fn load_map_local(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path.join("map_local.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_map_local(path: &Path, map: &HashMap<String, String>) {
    if let Err(e) = std::fs::write(
        path.join("map_local.json"),
        serde_json::to_string_pretty(map).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist map-local rules");
    }
}
