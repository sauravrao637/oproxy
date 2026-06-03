use crate::middleware::plugins::access_control::AccessRule;
use crate::middleware::plugins::breakpoints::BreakpointRule;
use crate::middleware::plugins::capture_filter::CaptureFilterConfig;
use crate::middleware::plugins::map_local::MapLocalRule;
use crate::middleware::plugins::map_remote::MapRemoteRule;
use crate::middleware::plugins::routing::ThrottlingConfig;
use crate::middleware::plugins::rules::RewriteRuleSet;
use std::collections::HashMap;
use std::io;
use std::path::Path;

fn to_io_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

async fn write_atomic(path: &Path, contents: String) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, &contents).await?;
    tokio::fs::rename(&tmp, path).await
}

async fn write_pretty<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(to_io_error)?;
    write_atomic(path, json).await
}

fn load_json<T: serde::de::DeserializeOwned + Default>(path: &Path, file_name: &str) -> T {
    std::fs::read_to_string(path.join(file_name))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

async fn save_json<T: serde::Serialize>(path: &Path, file_name: &str, value: &T) -> io::Result<()> {
    write_pretty(&path.join(file_name), value).await
}

pub fn load_rule_sets(path: &Path) -> Vec<RewriteRuleSet> {
    load_json(path, "rule_sets.json")
}

pub async fn save_rule_sets(path: &Path, rules: &[RewriteRuleSet]) -> io::Result<()> {
    save_json(path, "rule_sets.json", &rules).await
}

pub fn load_throttle(path: &Path) -> ThrottlingConfig {
    load_json(path, "throttle.json")
}

pub async fn save_throttle(path: &Path, config: &ThrottlingConfig) -> io::Result<()> {
    save_json(path, "throttle.json", config).await
}

pub fn load_dns_overrides(path: &Path) -> HashMap<String, String> {
    load_json(path, "dns_overrides.json")
}

pub async fn save_dns_overrides(path: &Path, map: &HashMap<String, String>) -> io::Result<()> {
    save_json(path, "dns_overrides.json", map).await
}

pub fn load_breakpoints(path: &Path) -> Vec<BreakpointRule> {
    load_json(path, "breakpoints.json")
}

pub async fn save_breakpoints(path: &Path, rules: &[BreakpointRule]) -> io::Result<()> {
    save_json(path, "breakpoints.json", &rules).await
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct HotConfig {
    pub max_body_bytes: Option<usize>,
}

pub fn load_hot_config(path: &Path) -> HotConfig {
    load_json(path, "hot_config.json")
}

pub async fn save_hot_config(path: &Path, cfg: &HotConfig) -> io::Result<()> {
    save_json(path, "hot_config.json", cfg).await
}

pub fn load_capture_filter(path: &Path) -> CaptureFilterConfig {
    load_json(path, "capture_filter.json")
}

pub async fn save_capture_filter(path: &Path, cfg: &CaptureFilterConfig) -> io::Result<()> {
    save_json(path, "capture_filter.json", cfg).await
}

pub fn load_upstream_proxy(path: &Path) -> Option<String> {
    load_json(path, "upstream_proxy.json")
}

pub async fn save_upstream_proxy(path: &Path, url: &Option<String>) -> io::Result<()> {
    save_json(path, "upstream_proxy.json", url).await
}

pub fn load_lua_scripts(path: &Path) -> Vec<crate::middleware::plugins::lua_engine::LuaScript> {
    load_json(path, "lua_scripts.json")
}

pub async fn save_lua_scripts(
    path: &Path,
    scripts: &[crate::middleware::plugins::lua_engine::LuaScript],
) -> io::Result<()> {
    save_json(path, "lua_scripts.json", &scripts).await
}

pub fn load_mock_rules(path: &Path) -> Vec<crate::middleware::plugins::mock::MockRule> {
    load_json(path, "mock_rules.json")
}

pub async fn save_mock_rules(
    path: &Path,
    rules: &[crate::middleware::plugins::mock::MockRule],
) -> io::Result<()> {
    save_json(path, "mock_rules.json", &rules).await
}

pub fn load_webhooks(path: &Path) -> Vec<crate::webhooks::WebhookConfig> {
    let mut hooks: Vec<crate::webhooks::WebhookConfig> = load_json(path, "webhooks.json");
    hooks.iter_mut().for_each(|hook| {
        crate::webhooks::sanitize_webhook_events(&mut hook.events);
    });
    hooks.retain(|hook| !hook.events.is_empty());
    hooks
}

pub async fn save_webhooks(
    path: &Path,
    hooks: &[crate::webhooks::WebhookConfig],
) -> io::Result<()> {
    save_json(path, "webhooks.json", &hooks).await
}

pub fn load_map_local_rules(path: &Path) -> Vec<MapLocalRule> {
    load_json(path, "map_local_rules.json")
}

pub async fn save_map_local_rules(path: &Path, rules: &[MapLocalRule]) -> io::Result<()> {
    save_json(path, "map_local_rules.json", &rules).await
}

pub fn load_access_rules(path: &Path) -> Vec<AccessRule> {
    load_json(path, "access_rules.json")
}

pub async fn save_access_rules(path: &Path, rules: &[AccessRule]) -> io::Result<()> {
    save_json(path, "access_rules.json", &rules).await
}

pub fn load_map_remote_rules(path: &Path) -> Vec<MapRemoteRule> {
    load_json(path, "map_remote_rules.json")
}

pub async fn save_map_remote_rules(path: &Path, rules: &[MapRemoteRule]) -> io::Result<()> {
    save_json(path, "map_remote_rules.json", &rules).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::matcher::Location;
    use crate::middleware::plugins::breakpoints::{BreakpointRule, BreakpointType};
    use crate::middleware::plugins::routing::ThrottlingConfig;
    use crate::middleware::plugins::rules::{AppliesTo, RewriteAction, RewriteRuleSet};
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

    // ── rule_sets ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rule_sets_roundtrip() {
        let dir = tmp("rule_sets_rt");
        let rules = vec![
            RewriteRuleSet {
                id: "r1".to_string(),
                name: "inject header".to_string(),
                enabled: true,
                location: Location {
                    host: Some("example.com".into()),
                    ..Default::default()
                },
                applies_to: AppliesTo::Request,
                actions: vec![RewriteAction::SetHeader {
                    name: "x-test".to_string(),
                    value: "1".to_string(),
                }],
            },
            RewriteRuleSet {
                id: "r2".to_string(),
                name: "block admin".to_string(),
                enabled: false,
                location: Location {
                    path: Some("/admin/*".into()),
                    ..Default::default()
                },
                applies_to: AppliesTo::Both,
                actions: vec![RewriteAction::Block { status: 403 }],
            },
        ];
        save_rule_sets(&dir, &rules).await.unwrap();
        let loaded = load_rule_sets(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "r1");
        assert!(loaded[0].enabled);
        assert_eq!(loaded[1].id, "r2");
        assert!(!loaded[1].enabled);
        cleanup(&dir);
    }

    #[test]
    fn rule_sets_missing_file_returns_empty() {
        let dir = tmp("rule_sets_missing");
        assert!(load_rule_sets(&dir).is_empty());
        cleanup(&dir);
    }

    // ── throttle ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn throttle_roundtrip() {
        let dir = tmp("throttle_rt");
        let cfg = ThrottlingConfig {
            latency_ms: 200,
            bandwidth_limit_kbps: 1024,
            enabled: true,
        };
        save_throttle(&dir, &cfg).await.unwrap();
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

    #[tokio::test]
    async fn dns_overrides_roundtrip() {
        let dir = tmp("dns_rt");
        let mut map = HashMap::new();
        map.insert("api.local".to_string(), "127.0.0.1".to_string());
        save_dns_overrides(&dir, &map).await.unwrap();
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

    #[tokio::test]
    async fn breakpoints_roundtrip() {
        use crate::middleware::matcher::MatchMode;
        let dir = tmp("bp_rt");
        let rules = vec![
            BreakpointRule {
                id: "bp1".to_string(),
                location: Location {
                    path: Some(r"/secret".to_string()),
                    mode: MatchMode::Regex,
                    ..Default::default()
                },
                bp_type: BreakpointType::Request,
                enabled: true,
            },
            BreakpointRule {
                id: "bp2".to_string(),
                location: Location {
                    path: Some(r"/admin".to_string()),
                    mode: MatchMode::Regex,
                    ..Default::default()
                },
                bp_type: BreakpointType::Response,
                enabled: false,
            },
        ];
        save_breakpoints(&dir, &rules).await.unwrap();
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

    #[tokio::test]
    async fn webhooks_load_drops_never_dispatched_events() {
        let dir = tmp("webhooks_events");
        let hooks = vec![crate::webhooks::WebhookConfig {
            id: "hook".to_string(),
            name: None,
            url: "http://example.com".to_string(),
            events: vec![
                crate::webhooks::WebhookEvent::BreakpointHit,
                crate::webhooks::WebhookEvent::RequestCaptured,
                crate::webhooks::WebhookEvent::Error,
            ],
            enabled: true,
            secret: None,
        }];
        save_webhooks(&dir, &hooks).await.unwrap();

        let loaded = load_webhooks(&dir);

        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].events,
            vec![crate::webhooks::WebhookEvent::RequestCaptured]
        );
        cleanup(&dir);
    }

    // ── hot_config ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn hot_config_roundtrip_with_value() {
        let dir = tmp("hot_rt");
        let cfg = HotConfig {
            max_body_bytes: Some(4096),
        };
        save_hot_config(&dir, &cfg).await.unwrap();
        let loaded = load_hot_config(&dir);
        assert_eq!(loaded.max_body_bytes, Some(4096));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn hot_config_roundtrip_none_value() {
        let dir = tmp("hot_none");
        let cfg = HotConfig {
            max_body_bytes: None,
        };
        save_hot_config(&dir, &cfg).await.unwrap();
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

    // ── map_local_rules ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn map_local_rules_roundtrip() {
        let dir = tmp("mlr_rt");
        let rules = vec![MapLocalRule {
            id: "r1".to_string(),
            name: "serve fixtures".to_string(),
            enabled: true,
            location: Location {
                host: Some("local.test".into()),
                ..Default::default()
            },
            file_path: "/tmp/fixtures".to_string(),
        }];
        save_map_local_rules(&dir, &rules).await.unwrap();
        let loaded = load_map_local_rules(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "r1");
        assert_eq!(loaded[0].file_path, "/tmp/fixtures");
        cleanup(&dir);
    }

    #[test]
    fn map_local_rules_missing_file_returns_empty() {
        let dir = tmp("mlr_missing");
        assert!(load_map_local_rules(&dir).is_empty());
        cleanup(&dir);
    }

    // ── map_remote_rules ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn map_remote_rules_roundtrip() {
        let dir = tmp("mrr_rt");
        let rules = vec![MapRemoteRule {
            id: "r1".to_string(),
            name: "staging".to_string(),
            enabled: true,
            location: Location {
                host: Some("api.local".into()),
                ..Default::default()
            },
            destination: "http://10.0.0.1:3000".to_string(),
        }];
        save_map_remote_rules(&dir, &rules).await.unwrap();
        let loaded = load_map_remote_rules(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "r1");
        assert_eq!(loaded[0].destination, "http://10.0.0.1:3000");
        cleanup(&dir);
    }

    #[test]
    fn map_remote_rules_missing_file_returns_empty() {
        let dir = tmp("mrr_missing");
        assert!(load_map_remote_rules(&dir).is_empty());
        cleanup(&dir);
    }

    // ── upstream_proxy ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn upstream_proxy_roundtrip_with_url() {
        let dir = tmp("up_rt");
        let url = Some("http://proxy.corp.example.com:3128".to_string());
        save_upstream_proxy(&dir, &url).await.unwrap();
        let loaded = load_upstream_proxy(&dir);
        assert_eq!(loaded, url);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn upstream_proxy_roundtrip_none() {
        let dir = tmp("up_none");
        save_upstream_proxy(&dir, &None).await.unwrap();
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
}
