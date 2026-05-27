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
}

pub(crate) async fn run() -> Result<(), StartupError> {
    let config = crate::config::Config::load();
    let _logging_guard = super::logging::setup_logging(&config);

    let services = super::state::build_runtime_services(&config).await?;
    let timeouts = build_timeouts(&config);

    let listeners = bind_listeners(&config, &services.ca).await?;

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
    let http_service = build_http_service(state.clone(), config, timeouts, supervisor);
    let socks5_service = build_socks5_service(state, timeouts);

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
