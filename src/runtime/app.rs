use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::control_plane;
use crate::transport::socks5::ProxySocks5Service;
use crate::transport::{TransportContext, http::ProxyHttpService};

use super::StartupError;

struct RuntimeTimeouts {
    connect: Duration,
    handshake: Duration,
    shutdown_grace: Duration,
}

struct BoundListeners {
    http: tokio::net::TcpListener,
    https: Option<(tokio::net::TcpListener, tokio_rustls::TlsAcceptor)>,
    socks5: Option<tokio::net::TcpListener>,
    #[cfg(feature = "http3")]
    http3: Option<h3_quinn::quinn::Endpoint>,
}

pub(crate) async fn run() -> Result<(), StartupError> {
    let (config, config_info, config_warnings) = crate::config::Config::load_with_diagnostics();
    let _logging_guard = super::logging::setup_logging(&config);

    // Config-phase diagnostics are buffered by `load_with_diagnostics()` because
    // no tracing subscriber exists yet at that point; flush them now that
    // logging is active so they are not silently dropped.
    for m in &config_info {
        tracing::info!("{m}");
    }
    for w in &config_warnings {
        tracing::warn!(warning = %w, "Config validation");
    }

    let services = super::state::build_runtime_services(&config).await?;
    let timeouts = build_timeouts(&config);

    // Best-effort, non-blocking update check (notify only).
    if config.update_check {
        tokio::spawn(control_plane::refresh_update_status(
            services.state.update_status.clone(),
        ));
    }

    let listeners = bind_listeners(&config, &services.ca).await?;
    log_startup_summary(&config, &listeners);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut supervisor = super::supervisor::RuntimeSupervisor::new(config.max_connections);

    spawn_runtime_listeners(
        listeners,
        services.state,
        &config,
        &timeouts,
        shutdown_rx,
        &mut supervisor,
    );

    tracing::info!("Proxy started. Press Ctrl-C to stop.");

    wait_for_shutdown(shutdown_tx, supervisor, timeouts.shutdown_grace).await;

    Ok(())
}

/// Hostname to use in clickable URLs. Wildcard binds aren't reachable as-is,
/// so point the user at localhost (the admin UI is served on localhost-style
/// hostnames regardless of the bind address).
fn url_host(bind_host: &str) -> &str {
    match bind_host {
        "0.0.0.0" | "::" | "[::]" => "localhost",
        other => other,
    }
}

/// One consolidated "what is served where" banner, logged right after every
/// listener has bound (with the real bound addresses, so port 0 / fallbacks
/// are reported correctly).
fn log_startup_summary(config: &crate::config::Config, listeners: &BoundListeners) {
    let ui_host = url_host(&config.bind_host);
    log_listener_summary(listeners, ui_host);
    log_security_summary(config, ui_host);
}

fn log_listener_summary(listeners: &BoundListeners, ui_host: &str) {
    if let Ok(addr) = listeners.http.local_addr() {
        tracing::info!("HTTP proxy     http://{addr} - set as the HTTP/HTTPS proxy in your client");
        tracing::info!(
            "Admin UI/API   http://{ui_host}:{} - open in a browser (served on localhost-style hostnames)",
            addr.port()
        );
    }
    if let Some((tls_listener, _)) = &listeners.https
        && let Ok(addr) = tls_listener.local_addr()
    {
        tracing::info!(
            "HTTPS proxy    https://{addr} - TLS proxy listener (clients must trust the oproxy CA)"
        );
    }
    if let Some(socks5) = &listeners.socks5
        && let Ok(addr) = socks5.local_addr()
    {
        tracing::info!("SOCKS5 proxy   socks5://{addr}");
    }
    #[cfg(feature = "http3")]
    if let Some(endpoint) = &listeners.http3
        && let Ok(addr) = endpoint.local_addr()
    {
        tracing::info!("HTTP/3 (QUIC)  udp://{addr} - advertised to clients via alt-svc");
    }
}

/// Severity for a `security_summary_lines()` entry - kept separate from
/// `tracing::Level` so the message-building logic is plain data and testable
/// without a subscriber.
#[derive(Debug, PartialEq, Eq)]
enum Severity {
    Info,
    Warn,
}

/// True for addresses that are only reachable from the machine oproxy runs
/// on. Anything else (a wildcard bind or a specific LAN/public interface) is
/// reachable from other machines and changes what "no admin token" means.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    host == "127.0.0.1" || host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// Builds the startup security posture banner as plain data (severity +
/// message pairs) so it can be unit tested without a tracing subscriber.
/// Gathers the proxy's security-relevant configuration — bind exposure,
/// insecure-upstream status, and admin-auth state — so an operator can see
/// the whole posture at a glance.
fn security_summary_lines(
    config: &crate::config::Config,
    ui_host: &str,
) -> Vec<(Severity, String)> {
    let mut lines = Vec::new();

    let loopback = is_loopback_host(&config.bind_host);
    if loopback {
        lines.push((
            Severity::Info,
            format!(
                "Bind host      {} - loopback only, not reachable from other machines",
                config.bind_host
            ),
        ));
    } else {
        lines.push((
            Severity::Info,
            format!(
                "Bind host      {} - reachable from other machines on the network",
                config.bind_host
            ),
        ));
    }

    if config.mitm.enabled {
        lines.push((
            Severity::Info,
            format!(
                "HTTPS MITM     enabled - download and trust the root CA at http://{ui_host}:{}/admin/ca",
                config.port
            ),
        ));
    } else {
        lines.push((
            Severity::Info,
            "HTTPS MITM     disabled - HTTPS is tunnelled without decryption".to_string(),
        ));
    }

    let admin_token_missing = config
        .admin_token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty());
    if !admin_token_missing {
        lines.push((
            Severity::Info,
            format!(
                "Admin auth     token required - sign in at http://{ui_host}:{}/login",
                config.port
            ),
        ));
    } else if loopback {
        lines.push((
            Severity::Info,
            "Admin auth     none - no OPROXY_ADMIN_TOKEN set (fine for loopback-only use)"
                .to_string(),
        ));
    } else {
        // Non-loopback bind with no token: config validation already warns
        // for the wildcard case (0.0.0.0/::); this also covers a specific
        // non-loopback interface (e.g. a LAN IP). Either way, this is the
        // exposure that matters at a glance during startup.
        lines.push((
            Severity::Warn,
            format!(
                "Admin auth     none - admin UI has no token and is reachable from the network \
                 (bind_host={}) - set OPROXY_ADMIN_TOKEN",
                config.bind_host
            ),
        ));
    }

    if crate::core::engine::insecure_upstream_enabled() {
        lines.push((
            Severity::Warn,
            "Upstream TLS   verification DISABLED (OPROXY_INSECURE_UPSTREAM) - do not point \
             this at untrusted origins"
                .to_string(),
        ));
    } else {
        lines.push((
            Severity::Info,
            "Upstream TLS   certificate verification enabled".to_string(),
        ));
    }

    lines
}

fn log_security_summary(config: &crate::config::Config, ui_host: &str) {
    for (severity, message) in security_summary_lines(config, ui_host) {
        match severity {
            Severity::Info => tracing::info!("{message}"),
            Severity::Warn => tracing::warn!("{message}"),
        }
    }
}

fn build_timeouts(config: &crate::config::Config) -> RuntimeTimeouts {
    RuntimeTimeouts {
        connect: Duration::from_secs(config.connect_timeout_secs),
        handshake: Duration::from_secs(config.handshake_timeout_secs),
        shutdown_grace: Duration::from_secs(config.shutdown_grace_secs),
    }
}

async fn bind_listeners(
    config: &crate::config::Config,
    ca: &std::sync::Arc<crate::certs::CertificateAuthority>,
) -> Result<BoundListeners, StartupError> {
    Ok(BoundListeners {
        http: super::listeners::bind_http_listener(config).await?,
        https: super::listeners::bind_https_listener(config, ca).await?,
        socks5: super::listeners::bind_socks5_listener(config).await?,

        #[cfg(feature = "http3")]
        http3: super::listeners::bind_http3_listener(config, ca).await,
    })
}

fn build_control_plane_app(state: Arc<super::state::AppState>) -> axum::Router {
    control_plane::control_plane_router(state.clone()).layer(axum::middleware::from_fn_with_state(
        state,
        control_plane::proxy_dispatch_layer,
    ))
}

fn build_http_service(
    state: Arc<super::state::AppState>,
    config: &crate::config::Config,
    timeouts: &RuntimeTimeouts,
    supervisor: &super::supervisor::RuntimeSupervisor,
) -> ProxyHttpService {
    let app = build_control_plane_app(state.clone());

    let context = TransportContext {
        session_manager: state.session_manager.clone(),
        breakpoint_manager: state.breakpoint_manager.clone(),
        mock_rules: state.mock_rules.clone(),
        engine: state.proxy_engine.clone(),
        dns_overrides: state.dns_overrides.clone(),
        connections: supervisor.connections(),
        inspect_ws_frames: config.inspect_ws_frames,
        connect_timeout: timeouts.connect,
        handshake_timeout: timeouts.handshake,
    };

    ProxyHttpService::new(app, context)
}

fn build_socks5_service(
    state: Arc<super::state::AppState>,
    timeouts: &RuntimeTimeouts,
) -> ProxySocks5Service {
    ProxySocks5Service {
        engine: state.proxy_engine.clone(),
        dns: state.dns_overrides.clone(),
        mock_rules: state.mock_rules.clone(),
        connect_timeout: timeouts.connect,
        handshake_timeout: timeouts.handshake,
    }
}

fn spawn_runtime_listeners(
    listeners: BoundListeners,
    state: Arc<super::state::AppState>,
    config: &crate::config::Config,
    timeouts: &RuntimeTimeouts,
    shutdown_rx: watch::Receiver<bool>,
    supervisor: &mut super::supervisor::RuntimeSupervisor,
) {
    // Record whether SOCKS5 actually bound so the status endpoint reflects reality.
    state.socks5_bound.store(
        listeners.socks5.is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );

    let http_service = build_http_service(state.clone(), config, timeouts, supervisor);
    let socks5_service = build_socks5_service(state.clone(), timeouts);

    super::listeners::spawn_http_listener(
        listeners.http,
        http_service.clone(),
        shutdown_rx.clone(),
        supervisor,
    );

    super::listeners::spawn_https_listener(
        listeners.https,
        http_service,
        shutdown_rx.clone(),
        timeouts.handshake,
        supervisor,
    );

    super::listeners::spawn_socks5_listener(
        listeners.socks5,
        socks5_service,
        shutdown_rx.clone(),
        supervisor,
    );

    #[cfg(feature = "http3")]
    super::listeners::spawn_http3_listener(
        listeners.http3,
        state.proxy_engine.clone(),
        shutdown_rx,
        supervisor,
    );
}

async fn wait_for_shutdown(
    shutdown_tx: watch::Sender<bool>,
    mut supervisor: super::supervisor::RuntimeSupervisor,
    shutdown_grace: Duration,
) {
    super::shutdown::wait_for_signal().await;

    tracing::info!("Shutdown signal received; stopping listeners");
    let _ = shutdown_tx.send(true);

    supervisor.drain(shutdown_grace).await;

    tracing::info!("Proxy stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> crate::config::Config {
        crate::config::Config::default()
    }

    #[test]
    fn is_loopback_host_recognises_loopback_forms() {
        for host in ["127.0.0.1", "localhost", "::1", "[::1]", "127.5.5.5"] {
            assert!(is_loopback_host(host), "{host} should be loopback");
        }
    }

    #[test]
    fn is_loopback_host_rejects_wildcard_and_lan() {
        for host in ["0.0.0.0", "::", "[::]", "192.168.1.5", "10.0.0.2"] {
            assert!(!is_loopback_host(host), "{host} should not be loopback");
        }
    }

    #[test]
    fn security_summary_warns_when_non_loopback_bind_has_no_admin_token() {
        let mut config = base_config();
        config.bind_host = "192.168.1.5".to_string();
        config.admin_token = None;
        let lines = security_summary_lines(&config, "192.168.1.5");
        let admin_line = lines
            .iter()
            .find(|(_, msg)| msg.starts_with("Admin auth"))
            .expect("admin auth line present");
        assert_eq!(admin_line.0, Severity::Warn, "{}", admin_line.1);
        assert!(admin_line.1.contains("reachable from the network"));
    }

    #[test]
    fn security_summary_is_quiet_when_loopback_bind_has_no_admin_token() {
        let mut config = base_config();
        config.bind_host = "127.0.0.1".to_string();
        config.admin_token = None;
        let lines = security_summary_lines(&config, "localhost");
        let admin_line = lines
            .iter()
            .find(|(_, msg)| msg.starts_with("Admin auth"))
            .expect("admin auth line present");
        assert_eq!(admin_line.0, Severity::Info, "{}", admin_line.1);
    }

    #[test]
    fn security_summary_confirms_token_when_configured() {
        let mut config = base_config();
        config.bind_host = "0.0.0.0".to_string();
        config.admin_token = Some("secret".to_string());
        let lines = security_summary_lines(&config, "localhost");
        let admin_line = lines
            .iter()
            .find(|(_, msg)| msg.starts_with("Admin auth"))
            .expect("admin auth line present");
        assert_eq!(admin_line.0, Severity::Info, "{}", admin_line.1);
        assert!(admin_line.1.contains("token required"));
    }

    #[test]
    fn security_summary_reports_bind_exposure() {
        let mut config = base_config();
        config.bind_host = "0.0.0.0".to_string();
        let lines = security_summary_lines(&config, "localhost");
        let bind_line = lines
            .iter()
            .find(|(_, msg)| msg.starts_with("Bind host"))
            .expect("bind host line present");
        assert_eq!(bind_line.0, Severity::Info);
        assert!(bind_line.1.contains("reachable from other machines"));

        let mut loopback_config = base_config();
        loopback_config.bind_host = "127.0.0.1".to_string();
        let loopback_lines = security_summary_lines(&loopback_config, "localhost");
        let loopback_bind_line = loopback_lines
            .iter()
            .find(|(_, msg)| msg.starts_with("Bind host"))
            .expect("bind host line present");
        assert!(loopback_bind_line.1.contains("loopback only"));
    }

    #[test]
    fn security_summary_reports_mitm_status() {
        let mut config = base_config();
        config.mitm.enabled = true;
        let lines = security_summary_lines(&config, "localhost");
        assert!(
            lines
                .iter()
                .any(|(_, msg)| msg.starts_with("HTTPS MITM") && msg.contains("enabled"))
        );

        let mut disabled = base_config();
        disabled.mitm.enabled = false;
        let disabled_lines = security_summary_lines(&disabled, "localhost");
        assert!(
            disabled_lines
                .iter()
                .any(|(_, msg)| msg.starts_with("HTTPS MITM") && msg.contains("disabled"))
        );
    }

    #[test]
    fn security_summary_reports_insecure_upstream_status_from_env() {
        // Not exercising the OPROXY_INSECURE_UPSTREAM=1 branch here: it's a
        // process-wide env var and other tests in this binary run in
        // parallel, so mutating it here would be racy. Default (unset) is
        // covered, matching the "safe by default" behaviour that matters
        // most for this banner.
        let config = base_config();
        let lines = security_summary_lines(&config, "localhost");
        assert!(
            lines
                .iter()
                .any(|(_, msg)| msg.starts_with("Upstream TLS") && msg.contains("enabled"))
        );
    }
}
