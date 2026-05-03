use std::collections::HashMap;
use std::path::Path;
use crate::middleware::plugins::breakpoints::BreakpointRule;
use crate::middleware::plugins::capture_filter::CaptureFilterConfig;
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

pub fn load_capture_filter(path: &Path) -> CaptureFilterConfig {
    std::fs::read_to_string(path.join("capture_filter.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_capture_filter(path: &Path, cfg: &CaptureFilterConfig) {
    if let Err(e) = std::fs::write(
        path.join("capture_filter.json"),
        serde_json::to_string_pretty(cfg).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist capture filter config");
    }
}

pub fn load_upstream_proxy(path: &Path) -> Option<String> {
    std::fs::read_to_string(path.join("upstream_proxy.json"))
        .ok()
        .and_then(|d| serde_json::from_str::<Option<String>>(&d).ok())
        .flatten()
}

pub fn save_upstream_proxy(path: &Path, url: &Option<String>) {
    if let Err(e) = std::fs::write(
        path.join("upstream_proxy.json"),
        serde_json::to_string(url).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist upstream proxy config");
    }
}

pub fn load_lua_scripts(path: &Path) -> Vec<crate::middleware::plugins::lua_engine::LuaScript> {
    std::fs::read_to_string(path.join("lua_scripts.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_lua_scripts(path: &Path, scripts: &[crate::middleware::plugins::lua_engine::LuaScript]) {
    if let Err(e) = std::fs::write(
        path.join("lua_scripts.json"),
        serde_json::to_string_pretty(scripts).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist Lua scripts");
    }
}

pub fn load_mock_rules(path: &Path) -> Vec<crate::middleware::plugins::mock::MockRule> {
    std::fs::read_to_string(path.join("mock_rules.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_mock_rules(path: &Path, rules: &[crate::middleware::plugins::mock::MockRule]) {
    if let Err(e) = std::fs::write(
        path.join("mock_rules.json"),
        serde_json::to_string_pretty(rules).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist mock rules");
    }
}

pub fn load_webhooks(path: &Path) -> Vec<crate::webhooks::WebhookConfig> {
    std::fs::read_to_string(path.join("webhooks.json"))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_webhooks(path: &Path, hooks: &[crate::webhooks::WebhookConfig]) {
    if let Err(e) = std::fs::write(
        path.join("webhooks.json"),
        serde_json::to_string_pretty(hooks).unwrap_or_default(),
    ) {
        tracing::warn!(error = %e, "Failed to persist webhooks");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::plugins::breakpoints::{BreakpointRule, BreakpointType};
    use crate::middleware::plugins::header_map::{HeaderMapRule, HmAction, HmScope};
    use crate::middleware::plugins::modification::ModificationRule;
    use crate::middleware::plugins::rewrite::{MatchCriteria, RewriteAction, RewriteRule};
    use crate::middleware::plugins::routing::ThrottlingConfig;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn tmp(label: &str) -> PathBuf {
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("oproxy_storage_{label}_{pid}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── routes ──────────────────────────────────────────────────────────────

    #[test]
    fn routes_roundtrip() {
        let dir = tmp("routes_rt");
        let mut routes = HashMap::new();
        routes.insert("api.local".to_string(), "http://10.0.0.1:8080".to_string());
        routes.insert("static.local".to_string(), "http://10.0.0.2".to_string());
        save_routes(&dir, &routes);
        let loaded = load_routes(&dir);
        assert_eq!(loaded, routes);
        cleanup(&dir);
    }

    #[test]
    fn routes_missing_file_returns_empty() {
        let dir = tmp("routes_missing");
        let loaded = load_routes(&dir);
        assert!(loaded.is_empty());
        cleanup(&dir);
    }

    // ── rewrites ─────────────────────────────────────────────────────────────

    #[test]
    fn rewrites_roundtrip() {
        let dir = tmp("rewrites_rt");
        let rules = vec![
            RewriteRule {
                name: "inject".to_string(),
                criteria: MatchCriteria::Host("example.com".to_string()),
                action: RewriteAction::AddHeader { name: "x-test".to_string(), value: "1".to_string() },
                enabled: true,
            },
            RewriteRule {
                name: "remove".to_string(),
                criteria: MatchCriteria::Path(r"^/api/".to_string()),
                action: RewriteAction::RemoveHeader { name: "authorization".to_string() },
                enabled: false,
            },
        ];
        save_rewrites(&dir, &rules);
        let loaded = load_rewrites(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "inject");
        assert!(!loaded[1].enabled);
        cleanup(&dir);
    }

    #[test]
    fn rewrites_missing_file_returns_empty() {
        let dir = tmp("rewrites_missing");
        assert!(load_rewrites(&dir).is_empty());
        cleanup(&dir);
    }

    // ── throttle ─────────────────────────────────────────────────────────────

    #[test]
    fn throttle_roundtrip() {
        let dir = tmp("throttle_rt");
        let cfg = ThrottlingConfig { latency_ms: 200, bandwidth_limit_kbps: 1024, enabled: true };
        save_throttle(&dir, &cfg);
        let loaded = load_throttle(&dir);
        assert_eq!(loaded.latency_ms, 200);
        assert_eq!(loaded.bandwidth_limit_kbps, 1024);
        assert!(loaded.enabled);
        cleanup(&dir);
    }

    #[test]
    fn throttle_missing_file_returns_zero_disabled() {
        let dir = tmp("throttle_missing");
        let loaded = load_throttle(&dir);
        assert_eq!(loaded.latency_ms, 0);
        assert!(!loaded.enabled);
        cleanup(&dir);
    }

    // ── dns_overrides ─────────────────────────────────────────────────────────

    #[test]
    fn dns_overrides_roundtrip() {
        let dir = tmp("dns_rt");
        let mut map = HashMap::new();
        map.insert("api.local".to_string(), "127.0.0.1".to_string());
        save_dns_overrides(&dir, &map);
        let loaded = load_dns_overrides(&dir);
        assert_eq!(loaded, map);
        cleanup(&dir);
    }

    #[test]
    fn dns_overrides_missing_file_returns_empty() {
        let dir = tmp("dns_missing");
        assert!(load_dns_overrides(&dir).is_empty());
        cleanup(&dir);
    }

    // ── breakpoints ──────────────────────────────────────────────────────────

    #[test]
    fn breakpoints_roundtrip() {
        let dir = tmp("bp_rt");
        let rules = vec![
            BreakpointRule { id: "bp1".to_string(), pattern: r"/secret".to_string(), bp_type: BreakpointType::Request, enabled: true },
            BreakpointRule { id: "bp2".to_string(), pattern: r"/admin".to_string(), bp_type: BreakpointType::Response, enabled: false },
        ];
        save_breakpoints(&dir, &rules);
        let loaded = load_breakpoints(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "bp1");
        assert!(loaded[0].enabled);
        assert_eq!(loaded[1].id, "bp2");
        assert!(!loaded[1].enabled);
        cleanup(&dir);
    }

    #[test]
    fn breakpoints_missing_file_returns_empty() {
        let dir = tmp("bp_missing");
        assert!(load_breakpoints(&dir).is_empty());
        cleanup(&dir);
    }

    // ── header_maps ──────────────────────────────────────────────────────────

    #[test]
    fn header_maps_roundtrip() {
        let dir = tmp("hm_rt");
        let rules = vec![
            HeaderMapRule {
                id: "hm1".to_string(),
                scope: HmScope::All,
                r#match: String::new(),
                action: HmAction::Set,
                name: "x-custom".to_string(),
                value: "hello".to_string(),
                enabled: true,
            },
        ];
        save_header_maps(&dir, &rules);
        let loaded = load_header_maps(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "hm1");
        assert_eq!(loaded[0].value, "hello");
        cleanup(&dir);
    }

    #[test]
    fn header_maps_missing_file_returns_empty() {
        let dir = tmp("hm_missing");
        assert!(load_header_maps(&dir).is_empty());
        cleanup(&dir);
    }

    // ── modifications ─────────────────────────────────────────────────────────

    #[test]
    fn modifications_roundtrip() {
        let dir = tmp("mod_rt");
        let mut hdrs = HashMap::new();
        hdrs.insert("x-injected".to_string(), "yes".to_string());
        let rules = vec![
            ModificationRule {
                request_uri_pattern: "/api".to_string(),
                header_replacements: hdrs,
                body_replacement: Some("replaced".to_string()),
            },
        ];
        save_modifications(&dir, &rules);
        let loaded = load_modifications(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].request_uri_pattern, "/api");
        assert_eq!(loaded[0].body_replacement.as_deref(), Some("replaced"));
        assert_eq!(loaded[0].header_replacements.get("x-injected").map(String::as_str), Some("yes"));
        cleanup(&dir);
    }

    #[test]
    fn modifications_missing_file_returns_empty() {
        let dir = tmp("mod_missing");
        assert!(load_modifications(&dir).is_empty());
        cleanup(&dir);
    }

    // ── hot_config ────────────────────────────────────────────────────────────

    #[test]
    fn hot_config_roundtrip_with_value() {
        let dir = tmp("hot_rt");
        let cfg = HotConfig { max_body_bytes: Some(4096) };
        save_hot_config(&dir, &cfg);
        let loaded = load_hot_config(&dir);
        assert_eq!(loaded.max_body_bytes, Some(4096));
        cleanup(&dir);
    }

    #[test]
    fn hot_config_roundtrip_none_value() {
        let dir = tmp("hot_none");
        let cfg = HotConfig { max_body_bytes: None };
        save_hot_config(&dir, &cfg);
        let loaded = load_hot_config(&dir);
        assert_eq!(loaded.max_body_bytes, None);
        cleanup(&dir);
    }

    #[test]
    fn hot_config_missing_file_returns_default() {
        let dir = tmp("hot_missing");
        let loaded = load_hot_config(&dir);
        assert_eq!(loaded.max_body_bytes, None);
        cleanup(&dir);
    }

    // ── map_local ─────────────────────────────────────────────────────────────

    #[test]
    fn map_local_roundtrip() {
        let dir = tmp("ml_rt");
        let mut map = HashMap::new();
        map.insert("local.test".to_string(), "/tmp/test.html".to_string());
        save_map_local(&dir, &map);
        let loaded = load_map_local(&dir);
        assert_eq!(loaded, map);
        cleanup(&dir);
    }

    #[test]
    fn map_local_missing_file_returns_empty() {
        let dir = tmp("ml_missing");
        assert!(load_map_local(&dir).is_empty());
        cleanup(&dir);
    }

    // ── overwrite semantics ───────────────────────────────────────────────────

    // ── upstream_proxy ────────────────────────────────────────────────────────

    #[test]
    fn upstream_proxy_roundtrip_with_url() {
        let dir = tmp("up_rt");
        let url = Some("http://proxy.corp.example.com:3128".to_string());
        save_upstream_proxy(&dir, &url);
        let loaded = load_upstream_proxy(&dir);
        assert_eq!(loaded, url);
        cleanup(&dir);
    }

    #[test]
    fn upstream_proxy_roundtrip_none() {
        let dir = tmp("up_none");
        save_upstream_proxy(&dir, &None);
        let loaded = load_upstream_proxy(&dir);
        assert!(loaded.is_none());
        cleanup(&dir);
    }

    #[test]
    fn upstream_proxy_missing_file_returns_none() {
        let dir = tmp("up_missing");
        let loaded = load_upstream_proxy(&dir);
        assert!(loaded.is_none());
        cleanup(&dir);
    }

    #[test]
    fn save_overwrites_previous_data() {
        let dir = tmp("overwrite");
        let mut first = HashMap::new();
        first.insert("a".to_string(), "1".to_string());
        save_routes(&dir, &first);

        let mut second = HashMap::new();
        second.insert("b".to_string(), "2".to_string());
        save_routes(&dir, &second);

        let loaded = load_routes(&dir);
        assert!(!loaded.contains_key("a"), "first entry must be gone after overwrite");
        assert_eq!(loaded.get("b").map(String::as_str), Some("2"));
        cleanup(&dir);
    }
}
