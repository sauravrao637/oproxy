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
    let config = crate::config::Config::load();
    let _logging_guard = super::logging::setup_logging(&config);

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

fn log_security_summary(config: &crate::config::Config, ui_host: &str) {
    if config.mitm.enabled {
        tracing::info!(
            "HTTPS MITM     enabled - download and trust the root CA at http://{ui_host}:{}/admin/ca",
            config.port
        );
    } else {
        tracing::info!("HTTPS MITM     disabled - HTTPS is tunnelled without decryption");
    }
    if config
        .admin_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
    {
        tracing::info!(
            "Admin auth     token required - sign in at http://{ui_host}:{}/login",
            config.port
        );
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
