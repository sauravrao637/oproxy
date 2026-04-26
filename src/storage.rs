use std::collections::HashMap;
use std::path::Path;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::middleware::plugins::rewrite::RewriteRule;
use crate::middleware::plugins::breakpoints::BreakpointRule;

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
