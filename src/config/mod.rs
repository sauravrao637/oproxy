use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

fn default_timeout_secs() -> u64 { 30 }
fn default_max_body_bytes() -> usize { 10 * 1024 * 1024 }
fn default_pool_max_idle_per_host() -> usize { 10 }
fn default_pool_idle_timeout_secs() -> u64 { 30 }
fn default_max_sessions() -> usize { 10_000 }
fn default_bind_host() -> String { "0.0.0.0".to_string() }
fn default_log_level() -> String { "info".to_string() }
fn default_log_dir() -> PathBuf { PathBuf::from(".") }
fn default_log_file() -> String { "server.log".to_string() }
fn default_inspect_ws_frames() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// Log level: trace, debug, info, warn, error (overridden by RUST_LOG).
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Directory where rolling log files are written.
    #[serde(default = "default_log_dir")]
    pub dir: PathBuf,
    /// Log file name prefix (rotated daily, date suffix appended).
    #[serde(default = "default_log_file")]
    pub file: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            dir: default_log_dir(),
            file: default_log_file(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub port: u16,
    /// IP address the proxy binds to. Use "127.0.0.1" to restrict to localhost only.
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    pub mitm: MitmConfig,
    pub storage_path: PathBuf,
    /// Upstream request timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Maximum request/response body buffered in memory (bytes). Default 10 MB.
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// Max idle connections kept per upstream host.
    #[serde(default = "default_pool_max_idle_per_host")]
    pub pool_max_idle_per_host: usize,
    /// Idle connection eviction timeout (seconds).
    #[serde(default = "default_pool_idle_timeout_secs")]
    pub pool_idle_timeout_secs: u64,
    /// Maximum sessions retained in memory; oldest evicted when full.
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
    /// Optional second listener port that accepts TLS connections (HTTPS proxy).
    /// When set, the proxy accepts CONNECT and plain requests over TLS on this port.
    /// Requires the CA cert to be trusted by the client. Disabled by default.
    #[serde(default)]
    pub https_port: Option<u16>,
    /// Parse and log individual WebSocket frames for ws:// sessions. Default true.
    #[serde(default = "default_inspect_ws_frames")]
    pub inspect_ws_frames: bool,
    /// Logging configuration.
    #[serde(default)]
    pub log: LogConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MitmConfig {
    pub enabled: bool,
    pub root_ca_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8080,
            bind_host: default_bind_host(),
            mitm: MitmConfig {
                enabled: false,
                root_ca_path: PathBuf::from("./certs"),
            },
            storage_path: PathBuf::from("./storage"),
            timeout_secs: default_timeout_secs(),
            max_body_bytes: default_max_body_bytes(),
            pool_max_idle_per_host: default_pool_max_idle_per_host(),
            pool_idle_timeout_secs: default_pool_idle_timeout_secs(),
            max_sessions: default_max_sessions(),
            log: LogConfig::default(),
            https_port: None,
            inspect_ws_frames: default_inspect_ws_frames(),
        }
    }
}

impl Config {
    /// Load configuration from a YAML file, then apply environment variable overrides.
    ///
    /// Resolution order (highest wins):
    ///   1. `OPROXY_PORT` / `OPROXY_MITM_ENABLED` / `OPROXY_STORAGE_PATH` env vars
    ///   2. Fields in the config file
    ///   3. Built-in defaults
    ///
    /// Config file path: `OPROXY_CONFIG` env var → `./configs/default.yaml` → built-in defaults.
    /// Returns a list of human-readable validation warnings (non-fatal).
    /// Callers should log these at startup so operators see them early.
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        if self.port == 0 {
            warnings.push("port is 0 — OS will assign an ephemeral port".to_string());
        }

        if self.timeout_secs == 0 {
            warnings.push("timeout_secs is 0 — upstream requests will never time out".to_string());
        }

        if self.max_body_bytes == 0 {
            warnings.push("max_body_bytes is 0 — request/response bodies will not be buffered".to_string());
        }

        // Check storage path is writable by attempting a temp file.
        if !self.storage_path.exists() {
            warnings.push(format!(
                "storage_path '{}' does not exist — it will be created on startup",
                self.storage_path.display()
            ));
        } else if std::fs::metadata(&self.storage_path)
            .map(|m| m.permissions().readonly())
            .unwrap_or(true)
        {
            warnings.push(format!(
                "storage_path '{}' appears to be read-only",
                self.storage_path.display()
            ));
        }

        // Check CA path when MITM is enabled.
        if self.mitm.enabled && !self.mitm.root_ca_path.exists() {
            warnings.push(format!(
                "mitm.root_ca_path '{}' does not exist — CA will be generated on first start",
                self.mitm.root_ca_path.display()
            ));
        }

        warnings
    }

    pub fn load() -> Self {
        let path = std::env::var("OPROXY_CONFIG")
            .unwrap_or_else(|_| "./configs/default.yaml".to_string());

        let mut config = match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_yaml::from_str::<Config>(&contents) {
                Ok(cfg) => {
                    info!(path = %path, "Loaded config from file");
                    cfg
                }
                Err(e) => {
                    warn!(path = %path, error = %e, "Failed to parse config file, using defaults");
                    Self::default()
                }
            },
            Err(_) => {
                info!(path = %path, "Config file not found, using defaults");
                Self::default()
            }
        };

        // Environment variable overrides
        if let Ok(port_str) = std::env::var("OPROXY_PORT") {
            match port_str.parse::<u16>() {
                Ok(port) => {
                    info!(port = port, "OPROXY_PORT override applied");
                    config.port = port;
                }
                Err(_) => warn!(value = %port_str, "OPROXY_PORT is not a valid port number, ignoring"),
            }
        }
        if let Ok(val) = std::env::var("OPROXY_MITM_ENABLED") {
            config.mitm.enabled = matches!(val.to_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(val) = std::env::var("OPROXY_STORAGE_PATH") {
            config.storage_path = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("OPROXY_BIND_HOST") {
            config.bind_host = val;
        }
        if let Ok(val) = std::env::var("OPROXY_LOG_LEVEL") {
            config.log.level = val;
        }
        if let Ok(val) = std::env::var("OPROXY_LOG_DIR") {
            config.log.dir = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("OPROXY_HTTPS_PORT") {
            match val.parse::<u16>() {
                Ok(p) => config.https_port = Some(p),
                Err(_) => warn!(value = %val, "OPROXY_HTTPS_PORT is not a valid port number, ignoring"),
            }
        }
        if let Ok(val) = std::env::var("OPROXY_INSPECT_WS_FRAMES") {
            config.inspect_ws_frames = !matches!(val.to_lowercase().as_str(), "0" | "false" | "no");
        }

        for w in config.validate() {
            warn!(warning = %w, "Config validation");
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    // Env-var tests mutate global process state; serialize them to avoid races.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn default_values() {
        let cfg = Config::default();
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.bind_host, "0.0.0.0");
        assert!(!cfg.mitm.enabled);
        assert_eq!(cfg.mitm.root_ca_path, PathBuf::from("./certs"));
        assert_eq!(cfg.storage_path, PathBuf::from("./storage"));
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.max_body_bytes, 10 * 1024 * 1024);
        assert_eq!(cfg.pool_max_idle_per_host, 10);
        assert_eq!(cfg.pool_idle_timeout_secs, 30);
        assert_eq!(cfg.max_sessions, 10_000);
        assert_eq!(cfg.log.level, "info");
        assert_eq!(cfg.log.dir, PathBuf::from("."));
        assert_eq!(cfg.log.file, "server.log");
    }

    #[test]
    fn load_returns_usable_defaults_when_no_file() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");
            std::env::remove_var("OPROXY_PORT");
            std::env::remove_var("OPROXY_MITM_ENABLED");
            std::env::remove_var("OPROXY_STORAGE_PATH");
        }
        let cfg = Config::load();
        unsafe { std::env::remove_var("OPROXY_CONFIG"); }
        assert_eq!(cfg.port, 8080);
        assert!(!cfg.mitm.enabled);
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.max_sessions, 10_000);
    }

    #[test]
    fn oproxy_port_env_var_overrides_port() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");
            std::env::set_var("OPROXY_PORT", "9090");
        }
        let cfg = Config::load();
        unsafe {
            std::env::remove_var("OPROXY_CONFIG");
            std::env::remove_var("OPROXY_PORT");
        }
        assert_eq!(cfg.port, 9090);
    }

    #[test]
    fn invalid_oproxy_port_is_ignored() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");
            std::env::set_var("OPROXY_PORT", "not_a_number");
        }
        let cfg = Config::load();
        unsafe {
            std::env::remove_var("OPROXY_CONFIG");
            std::env::remove_var("OPROXY_PORT");
        }
        assert_eq!(cfg.port, 8080);
    }

    #[test]
    fn oproxy_mitm_enabled_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        for val in ["1", "true", "yes", "TRUE", "YES"] {
            unsafe {
                std::env::set_var("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");
                std::env::set_var("OPROXY_MITM_ENABLED", val);
            }
            let cfg = Config::load();
            unsafe {
                std::env::remove_var("OPROXY_CONFIG");
                std::env::remove_var("OPROXY_MITM_ENABLED");
            }
            assert!(cfg.mitm.enabled, "expected mitm enabled for value '{val}'");
        }
    }

    #[test]
    fn oproxy_storage_path_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");
            std::env::set_var("OPROXY_STORAGE_PATH", "/tmp/my_storage");
        }
        let cfg = Config::load();
        unsafe {
            std::env::remove_var("OPROXY_CONFIG");
            std::env::remove_var("OPROXY_STORAGE_PATH");
        }
        assert_eq!(cfg.storage_path, PathBuf::from("/tmp/my_storage"));
    }

    #[test]
    fn oproxy_bind_host_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");
            std::env::set_var("OPROXY_BIND_HOST", "127.0.0.1");
        }
        let cfg = Config::load();
        unsafe {
            std::env::remove_var("OPROXY_CONFIG");
            std::env::remove_var("OPROXY_BIND_HOST");
        }
        assert_eq!(cfg.bind_host, "127.0.0.1");
    }

    #[test]
    fn oproxy_log_level_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");
            std::env::set_var("OPROXY_LOG_LEVEL", "debug");
        }
        let cfg = Config::load();
        unsafe {
            std::env::remove_var("OPROXY_CONFIG");
            std::env::remove_var("OPROXY_LOG_LEVEL");
        }
        assert_eq!(cfg.log.level, "debug");
    }

    #[test]
    fn oproxy_log_dir_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe {
            std::env::set_var("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");
            std::env::set_var("OPROXY_LOG_DIR", "/var/log/oproxy");
        }
        let cfg = Config::load();
        unsafe {
            std::env::remove_var("OPROXY_CONFIG");
            std::env::remove_var("OPROXY_LOG_DIR");
        }
        assert_eq!(cfg.log.dir, PathBuf::from("/var/log/oproxy"));
    }

    #[test]
    fn config_round_trips_through_yaml() {
        let original = Config::default();
        let yaml = serde_yaml::to_string(&original).expect("serialize failed");
        let restored: Config = serde_yaml::from_str(&yaml).expect("deserialize failed");
        assert_eq!(restored.port, original.port);
        assert_eq!(restored.bind_host, original.bind_host);
        assert_eq!(restored.mitm.enabled, original.mitm.enabled);
        assert_eq!(restored.mitm.root_ca_path, original.mitm.root_ca_path);
        assert_eq!(restored.storage_path, original.storage_path);
        assert_eq!(restored.timeout_secs, original.timeout_secs);
        assert_eq!(restored.max_body_bytes, original.max_body_bytes);
        assert_eq!(restored.pool_max_idle_per_host, original.pool_max_idle_per_host);
        assert_eq!(restored.pool_idle_timeout_secs, original.pool_idle_timeout_secs);
        assert_eq!(restored.max_sessions, original.max_sessions);
        assert_eq!(restored.log.level, original.log.level);
        assert_eq!(restored.log.dir, original.log.dir);
        assert_eq!(restored.log.file, original.log.file);
    }

    #[test]
    fn yaml_partial_fields_use_defaults() {
        // Only core fields specified; bind_host and log should fall back to defaults.
        let yaml = "port: 7777\nmitm:\n  enabled: false\n  root_ca_path: ./certs\nstorage_path: ./storage\n";
        let cfg: Config = serde_yaml::from_str(yaml).expect("deserialize failed");
        assert_eq!(cfg.port, 7777);
        assert_eq!(cfg.bind_host, "0.0.0.0");
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.max_body_bytes, 10 * 1024 * 1024);
        assert_eq!(cfg.max_sessions, 10_000);
        assert_eq!(cfg.log.level, "info");
        assert_eq!(cfg.log.file, "server.log");
    }

    #[test]
    fn load_from_valid_yaml_file() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let path = std::env::temp_dir().join("oproxy_test_config.yaml");
        std::fs::write(&path, "port: 7777\nmitm:\n  enabled: true\n  root_ca_path: ./certs\nstorage_path: ./storage\n").unwrap();
        unsafe {
            std::env::set_var("OPROXY_CONFIG", path.to_str().unwrap());
            std::env::remove_var("OPROXY_PORT");
        }
        let cfg = Config::load();
        unsafe { std::env::remove_var("OPROXY_CONFIG"); }
        let _ = std::fs::remove_file(&path);
        assert_eq!(cfg.port, 7777);
        assert!(cfg.mitm.enabled);
    }
}
