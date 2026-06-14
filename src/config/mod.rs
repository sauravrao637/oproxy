use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{info, warn};

fn default_timeout_secs() -> u64 {
    30
}

fn default_connect_timeout_secs() -> u64 {
    10
}

fn default_handshake_timeout_secs() -> u64 {
    10
}

fn default_shutdown_grace_secs() -> u64 {
    10
}

fn default_max_body_bytes() -> usize {
    10 * 1024 * 1024
}

fn default_pool_max_idle_per_host() -> usize {
    10
}

fn default_pool_idle_timeout_secs() -> u64 {
    30
}

fn default_max_sessions() -> usize {
    10_000
}

fn default_max_retained_body_bytes() -> usize {
    64 * 1024 * 1024
}

fn default_max_connections() -> usize {
    1024
}

fn default_bind_host() -> String {
    "127.0.0.1".to_string()
}

fn default_mitm_config() -> MitmConfig {
    MitmConfig {
        enabled: false,
        root_ca_path: PathBuf::from("./certs"),
    }
}

fn default_storage_path() -> PathBuf {
    PathBuf::from("./storage")
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_dir() -> PathBuf {
    PathBuf::from(".")
}

fn default_log_file() -> String {
    "server.log".to_string()
}

fn default_inspect_ws_frames() -> bool {
    true
}

fn default_allow_remote_admin() -> bool {
    false
}

fn default_allow_private_admin_egress() -> bool {
    false
}

fn default_update_check() -> bool {
    true
}

fn default_map_local_base_path() -> Option<PathBuf> {
    None
}

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
    /// Enable Man-in-the-Middle (MITM) interception and decryption of HTTPS traffic.
    #[serde(default = "default_mitm_config")]
    pub mitm: MitmConfig,
    /// Path to the storage directory.
    #[serde(default = "default_storage_path")]
    pub storage_path: PathBuf,
    /// Upstream request timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// TCP connect timeout for CONNECT, SOCKS5, and WebSocket upstream dials.
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// Timeout for client-side SOCKS5 and TLS handshake phases.
    #[serde(default = "default_handshake_timeout_secs")]
    pub handshake_timeout_secs: u64,
    /// Time to wait for listener tasks and accepted connections after shutdown signal.
    #[serde(default = "default_shutdown_grace_secs")]
    pub shutdown_grace_secs: u64,
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
    /// Approximate request/response body bytes retained across sessions.
    /// Older bodies are dropped when the budget is exceeded; metadata stays.
    #[serde(default = "default_max_retained_body_bytes")]
    pub max_retained_body_bytes: usize,
    /// Maximum concurrent accepted downstream connections across all listeners.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    /// Optional second listener port that accepts TLS connections (HTTPS proxy).
    /// When set, the proxy accepts CONNECT and plain requests over TLS on this port.
    /// Requires the CA cert to be trusted by the client. Disabled by default.
    #[serde(default)]
    pub https_port: Option<u16>,
    /// Parse and log individual WebSocket frames for ws:// sessions. Default true.
    #[serde(default = "default_inspect_ws_frames")]
    pub inspect_ws_frames: bool,
    /// Allow the management UI/API to be served on non-loopback Host headers.
    /// Disabled by default so binding the proxy to 0.0.0.0 does not expose admin APIs.
    #[serde(default = "default_allow_remote_admin")]
    pub allow_remote_admin: bool,
    /// Optional shared secret for the management UI/API.
    /// When set, clients must provide it via x-oproxy-admin-token, Authorization: Bearer,
    /// the oproxy_admin_token cookie, or a token/admin_token query parameter.
    #[serde(default)]
    pub admin_token: Option<String>,
    /// Allow admin-initiated outbound requests to private/local networks when remote admin is enabled.
    /// Disabled by default to reduce SSRF risk for /admin/forward, replay, and webhooks.
    #[serde(default = "default_allow_private_admin_egress")]
    pub allow_private_admin_egress: bool,
    /// Upstream proxy URL for chaining (e.g. "http://corp-proxy:3128" or "socks5://proxy:1080").
    /// When set, all outbound requests are routed through this proxy.
    #[serde(default)]
    pub upstream_proxy: Option<String>,
    /// Port to listen for SOCKS5 connections. Disabled when None (default).
    #[serde(default)]
    pub socks5_port: Option<u16>,
    /// Enable the HTTP/3 (QUIC) listener. Requires the `http3` build feature and
    /// `http3_port`. HTTP/3 uses a dedicated UDP port.
    #[serde(default)]
    pub http3_enabled: bool,
    /// UDP port for the HTTP/3 (QUIC) listener. Kept distinct from the TCP `port`
    /// so the QUIC listener lifecycle is independent and Alt-Svc can advertise it
    /// explicitly (`alt-svc: h3=":<http3_port>"`). Disabled when None.
    #[serde(default)]
    pub http3_port: Option<u16>,
    /// Export per-exchange protocol spans over OpenTelemetry. Requires
    /// the `otel` build feature and `otel_endpoint`. Disabled by default.
    #[serde(default)]
    pub otel_enabled: bool,
    /// OTLP collector endpoint (e.g. "http://localhost:4317"). The exporter layer
    /// is attached at startup when `otel_enabled` and the `otel` feature are on.
    #[serde(default)]
    pub otel_endpoint: Option<String>,
    /// Base directory for map_local fixture files. When set, MapLocalRule.file_path
    /// is resolved relative to this directory. When unset, paths are absolute.
    /// In containerized deployments, set via OPROXY_MAP_LOCAL_BASE_PATH env var.
    #[serde(default = "default_map_local_base_path")]
    pub map_local_base_path: Option<PathBuf>,
    /// Check GitHub Releases once at startup to surface a "newer version
    /// available" badge in the UI. Best-effort and non-blocking. This is the
    /// only outbound call oproxy makes on its own behalf; set
    /// `OPROXY_UPDATE_CHECK=false` to disable it.
    #[serde(default = "default_update_check")]
    pub update_check: bool,
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
            mitm: default_mitm_config(),
            storage_path: default_storage_path(),
            timeout_secs: default_timeout_secs(),
            connect_timeout_secs: default_connect_timeout_secs(),
            handshake_timeout_secs: default_handshake_timeout_secs(),
            shutdown_grace_secs: default_shutdown_grace_secs(),
            max_body_bytes: default_max_body_bytes(),
            pool_max_idle_per_host: default_pool_max_idle_per_host(),
            pool_idle_timeout_secs: default_pool_idle_timeout_secs(),
            max_sessions: default_max_sessions(),
            max_retained_body_bytes: default_max_retained_body_bytes(),
            max_connections: default_max_connections(),
            log: LogConfig::default(),
            https_port: None,
            inspect_ws_frames: default_inspect_ws_frames(),
            allow_remote_admin: default_allow_remote_admin(),
            admin_token: None,
            allow_private_admin_egress: default_allow_private_admin_egress(),
            upstream_proxy: None,
            socks5_port: None,
            http3_enabled: false,
            http3_port: None,
            otel_enabled: false,
            otel_endpoint: None,
            map_local_base_path: default_map_local_base_path(),
            update_check: default_update_check(),
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
    /// Config file path: `OPROXY_CONFIG` env var -> `./configs/default.yaml`.
    /// Loading panics if the selected file cannot be read or parsed.
    pub fn load() -> Self {
        let path =
            env_value("OPROXY_CONFIG").unwrap_or_else(|| "./configs/default.yaml".to_string());

        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read config file '{path}': {e}"));

        let mut config = serde_yaml::from_str::<Config>(&contents)
            .unwrap_or_else(|e| panic!("Failed to parse config file '{path}': {e}"));

        info!(path = %path, "Loaded config from file");

        config.apply_env_overrides();

        for w in config.validate() {
            warn!(warning = %w, "Config validation");
        }

        config
    }

    /// Override config values from environment variables
    fn apply_env_overrides(&mut self) {
        self.apply_network_env();
        self.apply_runtime_env();
        self.apply_security_env();
        self.apply_observability_env();
        self.apply_path_env();
    }

    fn apply_network_env(&mut self) {
        if let Some(value) = env_value("OPROXY_PORT") {
            self.port = parse_env("OPROXY_PORT", &value);
            info!(port = self.port, "OPROXY_PORT override applied");
        }
        if let Some(value) = env_value("OPROXY_BIND_HOST") {
            self.bind_host = value;
        }
        if let Some(value) = env_value("OPROXY_HTTPS_PORT") {
            self.https_port = Some(parse_env("OPROXY_HTTPS_PORT", &value));
        }
        if let Some(value) = env_value("OPROXY_HTTP3_ENABLED") {
            self.http3_enabled = parse_env_bool("OPROXY_HTTP3_ENABLED", &value);
        }
        if let Some(value) = env_value("OPROXY_HTTP3_PORT") {
            self.http3_port = Some(parse_env("OPROXY_HTTP3_PORT", &value));
        }
        if let Some(value) = env_value("OPROXY_SOCKS5_PORT") {
            self.socks5_port = parse_optional_port("OPROXY_SOCKS5_PORT", &value);
        }
    }

    fn apply_runtime_env(&mut self) {
        if let Some(value) = env_value("OPROXY_MAX_BODY_BYTES") {
            self.max_body_bytes = parse_env("OPROXY_MAX_BODY_BYTES", &value);
        }
        if let Some(value) = env_value("OPROXY_MAX_SESSIONS") {
            self.max_sessions = parse_env("OPROXY_MAX_SESSIONS", &value);
        }
        if let Some(value) = env_value("OPROXY_MAX_RETAINED_BODY_BYTES") {
            self.max_retained_body_bytes = parse_env("OPROXY_MAX_RETAINED_BODY_BYTES", &value);
        }
        if let Some(value) = env_value("OPROXY_MAX_CONNECTIONS") {
            self.max_connections = parse_env("OPROXY_MAX_CONNECTIONS", &value);
        }
        if let Some(value) = env_value("OPROXY_CONNECT_TIMEOUT_SECS") {
            self.connect_timeout_secs = parse_env("OPROXY_CONNECT_TIMEOUT_SECS", &value);
        }
        if let Some(value) = env_value("OPROXY_HANDSHAKE_TIMEOUT_SECS") {
            self.handshake_timeout_secs = parse_env("OPROXY_HANDSHAKE_TIMEOUT_SECS", &value);
        }
        if let Some(value) = env_value("OPROXY_SHUTDOWN_GRACE_SECS") {
            self.shutdown_grace_secs = parse_env("OPROXY_SHUTDOWN_GRACE_SECS", &value);
        }
        if let Some(value) = env_value("OPROXY_INSPECT_WS_FRAMES") {
            self.inspect_ws_frames = parse_env_bool("OPROXY_INSPECT_WS_FRAMES", &value);
        }
    }

    fn apply_security_env(&mut self) {
        if let Some(value) = env_value("OPROXY_MITM_ENABLED") {
            self.mitm.enabled = parse_env_bool("OPROXY_MITM_ENABLED", &value);
        }
        if let Some(value) = env_value("OPROXY_ALLOW_REMOTE_ADMIN") {
            self.allow_remote_admin = parse_env_bool("OPROXY_ALLOW_REMOTE_ADMIN", &value);
        }
        if let Some(value) = env_value("OPROXY_ADMIN_TOKEN") {
            self.admin_token = non_empty(value);
        }
        if let Some(value) = env_value("OPROXY_ALLOW_PRIVATE_ADMIN_EGRESS") {
            self.allow_private_admin_egress =
                parse_env_bool("OPROXY_ALLOW_PRIVATE_ADMIN_EGRESS", &value);
        }
    }

    fn apply_observability_env(&mut self) {
        if let Some(value) = env_value("OPROXY_LOG_LEVEL") {
            self.log.level = value;
        }
        if let Some(value) = env_value("OPROXY_LOG_DIR") {
            self.log.dir = PathBuf::from(value);
        }
        if let Some(value) = env_value("OPROXY_OTEL_ENABLED") {
            self.otel_enabled = parse_env_bool("OPROXY_OTEL_ENABLED", &value);
        }
        if let Some(value) = env_value("OPROXY_OTEL_ENDPOINT") {
            self.otel_endpoint = non_empty(value);
        }
        if let Some(value) = env_value("OPROXY_UPDATE_CHECK") {
            self.update_check = parse_env_bool("OPROXY_UPDATE_CHECK", &value);
        }
    }

    fn apply_path_env(&mut self) {
        if let Some(value) = env_value("OPROXY_STORAGE_PATH") {
            self.storage_path = PathBuf::from(value);
        }
        if let Some(value) = env_value("OPROXY_MAP_LOCAL_BASE_PATH") {
            self.map_local_base_path = non_empty(value).map(PathBuf::from);
        }
    }

    /// Returns a list of human-readable validation warnings (non-fatal).
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        self.validate_limits(&mut warnings);
        self.validate_paths(&mut warnings);
        self.validate_optional_features(&mut warnings);
        self.validate_admin_security(&mut warnings);
        warnings
    }

    fn validate_limits(&self, warnings: &mut Vec<String>) {
        if self.port == 0 {
            warnings.push("port is 0 - OS will assign an ephemeral port".to_string());
        }
        if self.timeout_secs == 0 {
            warnings.push("timeout_secs is 0 - upstream requests will never time out".to_string());
        }
        if self.connect_timeout_secs == 0 {
            warnings.push(
                "connect_timeout_secs is 0 - TCP connect attempts time out immediately".to_string(),
            );
        }
        if self.handshake_timeout_secs == 0 {
            warnings.push(
                "handshake_timeout_secs is 0 - protocol handshakes time out immediately"
                    .to_string(),
            );
        }
        if self.shutdown_grace_secs == 0 {
            warnings
                .push("shutdown_grace_secs is 0 - active connections are not drained".to_string());
        }
        if self.max_body_bytes == 0 {
            warnings.push(
                "max_body_bytes is 0 - request/response bodies will not be buffered".to_string(),
            );
        }
        if self.max_connections == 0 {
            warnings.push(
                "max_connections is 0 - all downstream connections will be rejected".to_string(),
            );
        }
    }

    fn validate_paths(&self, warnings: &mut Vec<String>) {
        if !self.storage_path.exists() {
            warnings.push(format!(
                "storage_path '{}' does not exist - it will be created on startup",
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
        if self.mitm.enabled && !self.mitm.root_ca_path.exists() {
            warnings.push(format!(
                "mitm.root_ca_path '{}' does not exist - CA will be generated on first start",
                self.mitm.root_ca_path.display()
            ));
        }
        if let Some(path) = &self.map_local_base_path
            && !path.exists()
        {
            warnings.push(format!(
                "map_local_base_path '{}' does not exist - map_local rules will fail at runtime",
                path.display()
            ));
        }
    }

    fn validate_optional_features(&self, warnings: &mut Vec<String>) {
        if self.http3_enabled && self.http3_port.is_none() {
            warnings.push(
                "http3_enabled is true but http3_port is not set - the HTTP/3 listener will not start"
                    .to_string(),
            );
        }

        if let Some(h3) = self.http3_port {
            if h3 == self.port {
                warnings.push(
                    "http3_port equals the TCP port - HTTP/3 uses UDP so this is allowed, but a distinct port is recommended"
                        .to_string(),
                );
            }
            if !cfg!(feature = "http3") {
                warnings.push(
                    "http3_port is set but the binary was built without the `http3` feature - HTTP/3 is unavailable"
                        .to_string(),
                );
            }
        }
        if self.otel_enabled {
            if self.otel_endpoint.is_none() {
                warnings.push(
                    "otel_enabled is true but otel_endpoint is not set - no spans will be exported"
                        .to_string(),
                );
            }
            if !cfg!(feature = "otel") {
                warnings.push(
                    "otel_enabled is true but the binary was built without the `otel` feature - OpenTelemetry export is unavailable"
                        .to_string(),
                );
            }
        }
    }

    fn validate_admin_security(&self, warnings: &mut Vec<String>) {
        let admin_token_missing = self
            .admin_token
            .as_deref()
            .is_none_or(|token| token.trim().is_empty());
        if self.allow_remote_admin && admin_token_missing {
            warnings.push(
                "allow_remote_admin is enabled without admin_token - management APIs are exposed"
                    .to_string(),
            );
        }

        if matches!(self.bind_host.trim(), "0.0.0.0" | "::" | "[::]") && admin_token_missing {
            warnings.push(
                "bind_host is wildcard without admin_token - use OPROXY_ADMIN_TOKEN when exposing the proxy to untrusted clients"
                    .to_string(),
            );
        }

        if self.allow_remote_admin && self.allow_private_admin_egress {
            warnings.push(
                "allow_private_admin_egress is enabled with remote admin - admin forward/webhook requests can reach private networks"
                    .to_string(),
            );
        }
    }
}

fn env_value(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => panic!("Environment variable {name} is invalid: {e}"),
    }
}

fn parse_env<T>(name: &str, value: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|e| panic!("Environment variable {name} has invalid value '{value}': {e}"))
}

fn parse_env_bool(name: &str, value: &str) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => true,
        "0" | "false" | "no" => false,
        _ => panic!(
            "Environment variable {name} has invalid boolean value '{value}'; expected true/false, 1/0, or yes/no"
        ),
    }
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn parse_optional_port(name: &str, value: &str) -> Option<u16> {
    let value = value.trim();
    if value.is_empty() || value == "0" {
        None
    } else {
        Some(parse_env(name, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    // Env-var tests mutate global process state; serialize them to avoid races.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());
    const DEFAULT_CONFIG_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/configs/default.yaml");

    /// Test-only RAII guard for process environment variables.
    ///
    /// Constructing it locks [`ENV_MUTEX`], serialising env-mutating tests.
    /// [`set`](EnvGuard::set)/[`remove`](EnvGuard::remove) record the previous
    /// value; on drop the originals are restored (even on panic). Centralising
    /// the `unsafe` env calls here keeps the test bodies clean and safe.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = ENV_MUTEX
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self {
                _lock: lock,
                saved: Vec::new(),
            }
        }

        fn set(&mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) -> &mut Self {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            // SAFETY: env access in these tests is serialised by the guard's lock.
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var(key, value);
            }
            self
        }

        fn remove(&mut self, key: &str) -> &mut Self {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            // SAFETY: env access in these tests is serialised by the guard's lock.
            #[allow(unsafe_code)]
            unsafe {
                std::env::remove_var(key);
            }
            self
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, prev) in self.saved.drain(..).rev() {
                // SAFETY: the guard's lock is still held during drop.
                #[allow(unsafe_code)]
                unsafe {
                    match prev {
                        Some(value) => std::env::set_var(&key, value),
                        None => std::env::remove_var(&key),
                    }
                }
            }
        }
    }

    #[test]
    fn default_values() {
        let cfg = Config::default();
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.bind_host, "127.0.0.1");
        assert!(!cfg.mitm.enabled);
        assert_eq!(cfg.mitm.root_ca_path, PathBuf::from("./certs"));
        assert_eq!(cfg.storage_path, PathBuf::from("./storage"));
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.connect_timeout_secs, 10);
        assert_eq!(cfg.handshake_timeout_secs, 10);
        assert_eq!(cfg.shutdown_grace_secs, 10);
        assert_eq!(cfg.max_body_bytes, 10 * 1024 * 1024);
        assert_eq!(cfg.max_retained_body_bytes, 64 * 1024 * 1024);
        assert_eq!(cfg.pool_max_idle_per_host, 10);
        assert_eq!(cfg.pool_idle_timeout_secs, 30);
        assert_eq!(cfg.max_sessions, 10_000);
        assert_eq!(cfg.max_connections, 1024);
        assert_eq!(cfg.log.level, "info");
        assert_eq!(cfg.log.dir, PathBuf::from("."));
        assert_eq!(cfg.log.file, "server.log");
        assert!(!cfg.allow_remote_admin);
        assert_eq!(cfg.admin_token, None);
        assert!(!cfg.allow_private_admin_egress);
    }

    #[test]
    fn load_panics_when_file_does_not_exist() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", "/tmp/oproxy_no_such_file.yaml");

        let result = std::panic::catch_unwind(Config::load);

        env.remove("OPROXY_CONFIG");

        let panic = result.expect_err("missing config should panic");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("Failed to read config file"));
        assert!(message.contains("/tmp/oproxy_no_such_file.yaml"));
    }

    #[test]
    fn oproxy_port_env_var_overrides_port() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_PORT", "9090");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_PORT");
        assert_eq!(cfg.port, 9090);
    }

    #[test]
    fn http3_env_vars_override_enabled_and_port() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_HTTP3_ENABLED", "true");
        env.set("OPROXY_HTTP3_PORT", "8443");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_HTTP3_ENABLED");
        env.remove("OPROXY_HTTP3_PORT");
        assert!(cfg.http3_enabled);
        assert_eq!(cfg.http3_port, Some(8443));
    }

    #[test]
    fn http3_enabled_without_port_warns() {
        let cfg = Config {
            http3_enabled: true,
            http3_port: None,
            ..Default::default()
        };
        assert!(
            cfg.validate()
                .iter()
                .any(|w| w.contains("http3_port is not set")),
            "must warn when h3 is enabled without a port"
        );
    }

    #[test]
    fn http3_disabled_by_default() {
        let cfg = Config::default();
        assert!(!cfg.http3_enabled);
        assert_eq!(cfg.http3_port, None);
    }

    #[test]
    fn otel_env_vars_override_enabled_and_endpoint() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_OTEL_ENABLED", "true");
        env.set("OPROXY_OTEL_ENDPOINT", "http://localhost:4317");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_OTEL_ENABLED");
        env.remove("OPROXY_OTEL_ENDPOINT");
        assert!(cfg.otel_enabled);
        assert_eq!(cfg.otel_endpoint.as_deref(), Some("http://localhost:4317"));
    }

    #[test]
    fn otel_enabled_without_endpoint_warns() {
        let cfg = Config {
            otel_enabled: true,
            otel_endpoint: None,
            ..Default::default()
        };
        assert!(
            cfg.validate()
                .iter()
                .any(|w| w.contains("otel_endpoint is not set")),
            "must warn when otel is enabled without an endpoint"
        );
    }

    #[test]
    fn otel_disabled_by_default() {
        let cfg = Config::default();
        assert!(!cfg.otel_enabled);
        assert_eq!(cfg.otel_endpoint, None);
    }

    #[test]
    fn invalid_oproxy_port_panics() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_PORT", "not_a_number");

        let result = std::panic::catch_unwind(Config::load);

        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_PORT");

        let panic = result.expect_err("invalid environment variable should panic");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("OPROXY_PORT"));
        assert!(message.contains("not_a_number"));
    }

    #[test]
    fn invalid_boolean_env_var_panics() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_MITM_ENABLED", "sometimes");

        let result = std::panic::catch_unwind(Config::load);

        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_MITM_ENABLED");

        let panic = result.expect_err("invalid boolean environment variable should panic");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("OPROXY_MITM_ENABLED"));
        assert!(message.contains("sometimes"));
    }

    #[test]
    fn oproxy_mitm_enabled_env_var() {
        let mut env = EnvGuard::new();
        for val in ["1", "true", "yes", "TRUE", "YES"] {
            env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
            env.set("OPROXY_MITM_ENABLED", val);
            let cfg = Config::load();
            env.remove("OPROXY_CONFIG");
            env.remove("OPROXY_MITM_ENABLED");
            assert!(cfg.mitm.enabled, "expected mitm enabled for value '{val}'");
        }
    }

    #[test]
    fn oproxy_storage_path_env_var() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_STORAGE_PATH", "/tmp/my_storage");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_STORAGE_PATH");
        assert_eq!(cfg.storage_path, PathBuf::from("/tmp/my_storage"));
    }

    #[test]
    fn oproxy_bind_host_env_var() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_BIND_HOST", "127.0.0.1");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_BIND_HOST");
        assert_eq!(cfg.bind_host, "127.0.0.1");
    }

    #[test]
    fn admin_security_env_vars_override_defaults() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_ALLOW_REMOTE_ADMIN", "true");
        env.set("OPROXY_ADMIN_TOKEN", "secret-token");
        env.set("OPROXY_ALLOW_PRIVATE_ADMIN_EGRESS", "true");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_ALLOW_REMOTE_ADMIN");
        env.remove("OPROXY_ADMIN_TOKEN");
        env.remove("OPROXY_ALLOW_PRIVATE_ADMIN_EGRESS");
        assert!(cfg.allow_remote_admin);
        assert_eq!(cfg.admin_token.as_deref(), Some("secret-token"));
        assert!(cfg.allow_private_admin_egress);
    }

    #[test]
    fn wildcard_bind_without_admin_token_warns() {
        let cfg = Config {
            bind_host: "0.0.0.0".to_string(),
            ..Config::default()
        };

        assert!(
            cfg.validate()
                .iter()
                .any(|warning| warning.contains("wildcard without admin_token"))
        );
    }

    #[test]
    fn private_admin_egress_with_remote_admin_warns() {
        let cfg = Config {
            allow_remote_admin: true,
            admin_token: Some("secret".to_string()),
            allow_private_admin_egress: true,
            ..Config::default()
        };

        assert!(
            cfg.validate()
                .iter()
                .any(|warning| warning.contains("allow_private_admin_egress"))
        );
    }

    #[test]
    fn capture_limit_env_vars_override_defaults() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_MAX_BODY_BYTES", "4096");
        env.set("OPROXY_MAX_SESSIONS", "123");
        env.set("OPROXY_MAX_RETAINED_BODY_BYTES", "8192");
        env.set("OPROXY_MAX_CONNECTIONS", "44");
        env.set("OPROXY_CONNECT_TIMEOUT_SECS", "3");
        env.set("OPROXY_HANDSHAKE_TIMEOUT_SECS", "4");
        env.set("OPROXY_SHUTDOWN_GRACE_SECS", "5");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_MAX_BODY_BYTES");
        env.remove("OPROXY_MAX_SESSIONS");
        env.remove("OPROXY_MAX_RETAINED_BODY_BYTES");
        env.remove("OPROXY_MAX_CONNECTIONS");
        env.remove("OPROXY_CONNECT_TIMEOUT_SECS");
        env.remove("OPROXY_HANDSHAKE_TIMEOUT_SECS");
        env.remove("OPROXY_SHUTDOWN_GRACE_SECS");
        assert_eq!(cfg.max_body_bytes, 4096);
        assert_eq!(cfg.max_sessions, 123);
        assert_eq!(cfg.max_retained_body_bytes, 8192);
        assert_eq!(cfg.max_connections, 44);
        assert_eq!(cfg.connect_timeout_secs, 3);
        assert_eq!(cfg.handshake_timeout_secs, 4);
        assert_eq!(cfg.shutdown_grace_secs, 5);
    }

    #[test]
    fn oproxy_log_level_env_var() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_LOG_LEVEL", "debug");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_LOG_LEVEL");
        assert_eq!(cfg.log.level, "debug");
    }

    #[test]
    fn oproxy_log_dir_env_var() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_LOG_DIR", "/var/log/oproxy");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_LOG_DIR");
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
        assert_eq!(restored.connect_timeout_secs, original.connect_timeout_secs);
        assert_eq!(
            restored.handshake_timeout_secs,
            original.handshake_timeout_secs
        );
        assert_eq!(restored.shutdown_grace_secs, original.shutdown_grace_secs);
        assert_eq!(restored.max_body_bytes, original.max_body_bytes);
        assert_eq!(
            restored.pool_max_idle_per_host,
            original.pool_max_idle_per_host
        );
        assert_eq!(
            restored.pool_idle_timeout_secs,
            original.pool_idle_timeout_secs
        );
        assert_eq!(restored.max_sessions, original.max_sessions);
        assert_eq!(
            restored.max_retained_body_bytes,
            original.max_retained_body_bytes
        );
        assert_eq!(restored.max_connections, original.max_connections);
        assert_eq!(restored.log.level, original.log.level);
        assert_eq!(restored.log.dir, original.log.dir);
        assert_eq!(restored.log.file, original.log.file);
        assert_eq!(restored.allow_remote_admin, original.allow_remote_admin);
        assert_eq!(restored.admin_token, original.admin_token);
        assert_eq!(
            restored.allow_private_admin_egress,
            original.allow_private_admin_egress
        );
    }

    #[test]
    fn yaml_partial_fields_use_defaults() {
        let yaml = "port: 7777\n";
        let cfg: Config = serde_yaml::from_str(yaml).expect("deserialize failed");
        assert_eq!(cfg.port, 7777);
        assert_eq!(cfg.bind_host, "127.0.0.1");
        assert!(!cfg.mitm.enabled);
        assert_eq!(cfg.mitm.root_ca_path, PathBuf::from("./certs"));
        assert_eq!(cfg.storage_path, PathBuf::from("./storage"));
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.connect_timeout_secs, 10);
        assert_eq!(cfg.handshake_timeout_secs, 10);
        assert_eq!(cfg.shutdown_grace_secs, 10);
        assert_eq!(cfg.max_body_bytes, 10 * 1024 * 1024);
        assert_eq!(cfg.max_sessions, 10_000);
        assert_eq!(cfg.max_retained_body_bytes, 64 * 1024 * 1024);
        assert_eq!(cfg.max_connections, 1024);
        assert_eq!(cfg.log.level, "info");
        assert_eq!(cfg.log.file, "server.log");
        assert!(!cfg.allow_remote_admin);
        assert_eq!(cfg.admin_token, None);
        assert!(!cfg.allow_private_admin_egress);
    }

    #[test]
    fn load_from_valid_yaml_file() {
        let mut env = EnvGuard::new();
        let path = std::env::temp_dir().join("oproxy_test_config.yaml");
        std::fs::write(&path, "port: 7777\nmitm:\n  enabled: true\n  root_ca_path: ./certs\nstorage_path: ./storage\n").unwrap();
        env.set("OPROXY_CONFIG", path.to_str().unwrap());
        env.remove("OPROXY_PORT");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        let _ = std::fs::remove_file(&path);
        assert_eq!(cfg.port, 7777);
        assert!(cfg.mitm.enabled);
    }

    #[test]
    fn load_panics_when_yaml_file_is_invalid() {
        let mut env = EnvGuard::new();
        let path = std::env::temp_dir().join("oproxy_test_invalid_config.yaml");
        std::fs::write(&path, "port: not-a-number\n").unwrap();
        env.set("OPROXY_CONFIG", &path);

        let result = std::panic::catch_unwind(Config::load);

        env.remove("OPROXY_CONFIG");
        let _ = std::fs::remove_file(&path);

        let panic = result.expect_err("invalid config should panic");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("Failed to parse config file"));
        assert!(message.contains(path.to_str().unwrap()));
    }

    #[test]
    fn map_local_base_path_nonexistent_warns() {
        let cfg = Config {
            map_local_base_path: Some(PathBuf::from("/nonexistent/fixtures")),
            ..Config::default()
        };
        let warnings = cfg.validate();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("map_local_base_path") && w.contains("does not exist"))
        );
    }

    #[test]
    fn map_local_base_path_existing_no_warn() {
        let base = std::env::temp_dir().join("map_local_test_base");
        std::fs::create_dir_all(&base).unwrap();
        let cfg = Config {
            map_local_base_path: Some(base.clone()),
            ..Config::default()
        };
        let warnings = cfg.validate();
        assert!(!warnings.iter().any(|w| w.contains("map_local_base_path")));
        let _ = std::fs::remove_dir(&base);
    }

    #[test]
    fn oproxy_map_local_base_path_env_var() {
        let mut env = EnvGuard::new();
        env.set("OPROXY_CONFIG", DEFAULT_CONFIG_PATH);
        env.set("OPROXY_MAP_LOCAL_BASE_PATH", "/fixtures");
        let cfg = Config::load();
        env.remove("OPROXY_CONFIG");
        env.remove("OPROXY_MAP_LOCAL_BASE_PATH");
        assert_eq!(cfg.map_local_base_path, Some(PathBuf::from("/fixtures")));
    }
}
