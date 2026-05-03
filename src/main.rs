use std::sync::Arc;
use tokio::sync::RwLock;
use std::net::SocketAddr;
use std::collections::HashMap;
use tower::ServiceExt as _;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use tracing_appender::non_blocking::WorkerGuard;

mod core;
mod middleware;
mod session;
mod certs;
mod config;
mod api;
mod storage;
mod management;
mod setup;
mod export;
mod har;
mod diff;
mod webhooks;
mod transport;

use crate::api::ApiHandler;
use crate::core::engine::ProxyEngine;
use crate::middleware::chain::MiddlewareChain;
use crate::middleware::plugins::routing::{RoutingMiddleware, ThrottlingMiddleware, ThrottlingConfig};
use crate::middleware::plugins::inspection::InspectionMiddleware;
use crate::middleware::plugins::modification::ModificationMiddleware;
use crate::middleware::plugins::rewrite::RewriteMiddleware;
use crate::middleware::plugins::breakpoints::{BreakpointMiddleware, BreakpointManager};
use crate::middleware::plugins::capture_filter::{CaptureFilterMiddleware, CaptureFilterConfig};
use crate::middleware::plugins::dns_override::DnsOverrideMiddleware;
use crate::middleware::plugins::header_map::HeaderMapMiddleware;
use crate::middleware::plugins::jwt_inspector::JwtInspectorMiddleware;
use crate::middleware::plugins::graphql_inspector::GraphQLInspectorMiddleware;
use crate::middleware::plugins::grpc_inspector::GrpcInspectorMiddleware;
use crate::middleware::plugins::mock::MockMiddleware;
use crate::middleware::plugins::lua_engine::LuaEngineMiddleware;

// Shared state threaded through every axum handler and the proxy engine.
pub(crate) struct AppState {
    pub(crate) proxy_engine: Arc<ProxyEngine>,
    pub(crate) middleware_chain: Arc<RwLock<MiddlewareChain>>,
    pub(crate) routing_table: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) throttling_config: Arc<RwLock<ThrottlingConfig>>,
    pub(crate) dns_overrides: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) map_local: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) capture_filter: Arc<RwLock<CaptureFilterConfig>>,
    pub(crate) session_manager: crate::session::SharedSessionManager,
    pub(crate) breakpoint_manager: Arc<BreakpointManager>,
    pub(crate) api_handler: Arc<ApiHandler>,
    pub(crate) storage_path: std::path::PathBuf,
    pub(crate) started_at: std::time::Instant,
    pub(crate) config: crate::config::Config,
    pub(crate) webhooks: crate::webhooks::SharedWebhooks,
    pub(crate) mock_rules: crate::middleware::plugins::mock::SharedMockRules,
    pub(crate) lua_scripts: crate::middleware::plugins::lua_engine::SharedLuaScripts,
}

fn setup_logging(config: &crate::config::Config) -> WorkerGuard {
    let file_appender = tracing_appender::rolling::daily(&config.log.dir, &config.log.file);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let level: tracing::Level = config.log.level.parse().unwrap_or(tracing::Level::INFO);

    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env().add_directive(level.into()))
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::fmt::layer().with_writer(non_blocking))
        .init();

    guard
}

#[derive(Debug, thiserror::Error)]
enum StartupError {
    #[error("Invalid bind address '{addr}': {source}")]
    InvalidAddr { addr: String, source: std::net::AddrParseError },
    #[error("Failed to bind listener on {addr}: {source}")]
    BindFailed { addr: String, source: std::io::Error },
    #[error("Failed to initialise certificate authority: {0}")]
    CaInit(String),
}

#[tokio::main]
async fn main() -> Result<(), StartupError> {
    let config = crate::config::Config::load();
    let _guard = setup_logging(&config);
    let session_manager = Arc::new(crate::session::SessionManager::new(config.max_sessions));

    let storage_path = config.storage_path.clone();

    let _ = std::fs::create_dir_all(&storage_path);

    let routing_table = Arc::new(RwLock::new(storage::load_routes(&storage_path)));
    let throttling_config = Arc::new(RwLock::new(storage::load_throttle(&storage_path)));
    let dns_overrides = Arc::new(RwLock::new(storage::load_dns_overrides(&storage_path)));
    let initial_rewrites = storage::load_rewrites(&storage_path);

    let capture_filter = Arc::new(RwLock::new(storage::load_capture_filter(&storage_path)));

    let mut chain = MiddlewareChain::new();
    // CaptureFilter runs first: injects skip-recording header for filtered hosts.
    chain.add_middleware(Arc::new(CaptureFilterMiddleware::new(capture_filter.clone())));
    // DNS override must run before RoutingMiddleware so the host rewrite is visible to it.
    chain.add_middleware(Arc::new(DnsOverrideMiddleware { overrides: dns_overrides.clone() }));
    let map_local = Arc::new(tokio::sync::RwLock::new(storage::load_map_local(&storage_path)));
    let routing_mw = Arc::new({
        let mut mw = RoutingMiddleware::new(routing_table.clone());
        mw.map_local = map_local.clone();
        mw
    });
    chain.add_middleware(routing_mw);
    chain.add_middleware(Arc::new(ThrottlingMiddleware { config: throttling_config.clone() }));

    let rewrite_middleware = Arc::new(RewriteMiddleware::new(initial_rewrites));
    chain.add_middleware(rewrite_middleware.clone());

    let header_map_middleware = Arc::new(HeaderMapMiddleware::new(
        storage::load_header_maps(&storage_path),
    ));
    chain.add_middleware(header_map_middleware.clone());

    let breakpoint_manager = Arc::new(BreakpointManager::new());
    for rule in storage::load_breakpoints(&storage_path) {
        breakpoint_manager.add_rule(rule).await;
    }
    chain.add_middleware(Arc::new(BreakpointMiddleware::new(breakpoint_manager.clone())));
    // Inspector plugins run BEFORE InspectionMiddleware so they can set x-oproxy-jwt /
    // x-oproxy-graphql / x-oproxy-grpc headers that InspectionMiddleware reads on the
    // same on_request pass and stores into the session's inspector_data.
    chain.add_middleware(Arc::new(JwtInspectorMiddleware));
    chain.add_middleware(Arc::new(GraphQLInspectorMiddleware));
    chain.add_middleware(Arc::new(GrpcInspectorMiddleware));
    chain.add_middleware(Arc::new(InspectionMiddleware::new(session_manager.clone())));
    let modification_middleware = Arc::new(ModificationMiddleware::new(
        storage::load_modifications(&storage_path),
    ));
    chain.add_middleware(modification_middleware.clone());
    // Mock and Lua come after InspectionMiddleware so the request is recorded before
    // they short-circuit it (StopAndReturn).  The session captures the original request;

    let middleware_chain = Arc::new(RwLock::new(chain));

    // CA is always initialised so the cert is downloadable regardless of mitm_enabled.
    // mitm_enabled only controls CONNECT interception.
    let ca = Arc::new(
        crate::certs::CertificateAuthority::new(&config.mitm.root_ca_path)
            .await
            .map_err(|e| StartupError::CaInit(e.to_string()))?,
    );

    let hot_cfg = storage::load_hot_config(&storage_path);
    let effective_max_body = hot_cfg.max_body_bytes.unwrap_or(config.max_body_bytes);
    let upstream_proxy = storage::load_upstream_proxy(&storage_path);
    let proxy_engine = Arc::new(ProxyEngine::new(
        middleware_chain.clone(),
        Some(ca.clone()),
        config.mitm.enabled,
        config.timeout_secs,
        effective_max_body,
        config.pool_max_idle_per_host,
        config.pool_idle_timeout_secs,
        upstream_proxy.as_deref().or(config.upstream_proxy.as_deref()),
    ));

    let api_handler = Arc::new(ApiHandler::new(
        session_manager.clone(),
        rewrite_middleware.clone(),
        breakpoint_manager.clone(),
        header_map_middleware.clone(),
        modification_middleware.clone(),
    ));

    let webhooks_shared = {
        let hooks = storage::load_webhooks(&storage_path);
        let shared = Arc::new(tokio::sync::RwLock::new(hooks));
        let dispatcher = crate::webhooks::WebhookDispatcher::new(shared.clone());
        dispatcher.spawn(session_manager.subscribe(), session_manager.clone());
        shared
    };
    let mock_rules_shared = Arc::new(tokio::sync::RwLock::new(storage::load_mock_rules(&storage_path)));
    let lua_scripts_shared = Arc::new(tokio::sync::RwLock::new(storage::load_lua_scripts(&storage_path)));

    // Wire Mock and Lua into the middleware chain now that their shared state is ready.
    {
        let mut chain = middleware_chain.write().await;
        chain.add_middleware(Arc::new(MockMiddleware::new(mock_rules_shared.clone())));
        chain.add_middleware(Arc::new(LuaEngineMiddleware::new(lua_scripts_shared.clone())));
    }

    let state = Arc::new(AppState {
        proxy_engine,
        middleware_chain,
        routing_table,
        throttling_config,
        dns_overrides: dns_overrides.clone(),
        map_local: map_local.clone(),
        capture_filter: capture_filter.clone(),
        session_manager,
        breakpoint_manager,
        api_handler,
        storage_path,
        started_at: std::time::Instant::now(),
        config: config.clone(),
        webhooks: webhooks_shared,
        mock_rules: mock_rules_shared,
        lua_scripts: lua_scripts_shared,
    });

    let app = management::management_router(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            management::proxy_dispatch_layer,
        ));

    let addr_str = format!("{}:{}", config.bind_host, config.port);
    let addr: SocketAddr = addr_str.parse()
        .map_err(|e| StartupError::InvalidAddr { addr: addr_str.clone(), source: e })?;
    let listener = tokio::net::TcpListener::bind(addr).await
        .map_err(|e| StartupError::BindFailed { addr: addr_str, source: e })?;
    println!("oproxy listening on http://{}", addr);

    // Optional HTTPS listener: binds on https_port (if configured) and terminates TLS
    // using a certificate generated for "localhost" by the proxy's own CA.
    // Browsers can use this port as an HTTPS proxy without needing stunnel/socat.
    let tls_listener_state: Option<(tokio::net::TcpListener, tokio_rustls::TlsAcceptor)> =
        if let Some(https_port) = config.https_port {
            match ca.get_certificate_for_domain("localhost").await {
                Ok((cert_der, key_der)) => {
                    let cert_chain = vec![rustls::Certificate(cert_der)];
                    let private_key = rustls::PrivateKey(key_der);
                    match rustls::ServerConfig::builder()
                        .with_safe_defaults()
                        .with_no_client_auth()
                        .with_single_cert(cert_chain, private_key)
                    {
                        Ok(tls_cfg) => {
                            let tls_addr_str = format!("{}:{}", config.bind_host, https_port);
                            let tls_addr: SocketAddr = match tls_addr_str.parse() {
                                Ok(a) => a,
                                Err(e) => return Err(StartupError::InvalidAddr { addr: tls_addr_str, source: e }),
                            };
                            match tokio::net::TcpListener::bind(tls_addr).await {
                                Ok(tls_l) => {
                                    println!("oproxy HTTPS listener on https://{}", tls_addr);
                                    Some((tls_l, tokio_rustls::TlsAcceptor::from(Arc::new(tls_cfg))))
                                }
                                Err(e) => {
                                    tracing::warn!(error=%e, "Failed to bind HTTPS listener, continuing without it");
                                    None
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error=%e, "Failed to build TLS config for HTTPS listener");
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error=%e, "Failed to generate localhost cert for HTTPS listener");
                    None
                }
            }
        } else {
            None
        };

    // Optional SOCKS5 listener.
    if let Some(socks5_port) = config.socks5_port {
        let socks5_addr_str = format!("{}:{}", config.bind_host, socks5_port);
        match tokio::net::TcpListener::bind(&socks5_addr_str).await {
            Ok(socks5_listener) => {
                println!("oproxy SOCKS5 listener on socks5://{}", socks5_addr_str);
                let eng_s5 = state.proxy_engine.clone();
                let dns_s5 = dns_overrides.clone();
                tokio::spawn(async move {
                    loop {
                        let (mut stream, _peer) = match socks5_listener.accept().await {
                            Ok(pair) => pair,
                            Err(e) => { tracing::warn!(error=%e, "SOCKS5 accept error"); continue; }
                        };
                        let engine = eng_s5.clone();
                        let dns = dns_s5.clone();
                        tokio::spawn(async move {
                            let target = match crate::transport::socks5::handshake(&mut stream).await {
                                Ok(t) => t,
                                Err(e) => { tracing::debug!(error=%e, "SOCKS5 handshake failed"); return; }
                            };
                            // Apply DNS override
                            let resolved_host = {
                                let ovr = dns.read().await;
                                ovr.get(&target.host).cloned().unwrap_or_else(|| target.host.clone())
                            };
                            let resolved = crate::transport::socks5::Socks5Target {
                                host: resolved_host,
                                port: target.port,
                            };
                            if engine.mitm_enabled {
                                if let Some(ca) = engine.ca.clone() {
                                    // MITM path: intercept TLS same as HTTP CONNECT.
                                    // TcpStream implements AsyncRead+AsyncWrite directly.
                                    mitm_intercept(stream, resolved.host.clone(), engine.clone(), ca).await;
                                } else {
                                    if let Err(e) = crate::transport::socks5::tunnel(stream, &resolved).await {
                                        tracing::debug!(error=%e, "SOCKS5 tunnel error");
                                    }
                                }
                            } else {
                                if let Err(e) = crate::transport::socks5::tunnel(stream, &resolved).await {
                                    tracing::debug!(error=%e, "SOCKS5 tunnel error");
                                }
                            }
                        });
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error=%e, port=socks5_port, "Failed to bind SOCKS5 listener");
            }
        }
    }

    println!("Press Ctrl-C to stop.");

    let shutdown = async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler failed");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = sigterm.recv() => {},
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    };
    tokio::pin!(shutdown);

    // Use hyper directly so we can call .with_upgrades() and intercept CONNECT at the
    // raw service level. CONNECT must be handled here because hyper only delivers the raw
    // TCP socket (via OnUpgrade) to the same service function that returned the 200 —
    // routing it through axum's middleware stack severs that link.
    let session_manager = state.session_manager.clone();
    let proxy_engine = state.proxy_engine.clone();
    let inspect_ws_frames = config.inspect_ws_frames;

    // Spawn the optional HTTPS accept loop. It shares the same service logic as HTTP
    // but accepts TLS-wrapped TCP streams from the secondary listener.
    if let Some((tls_tcp, tls_acceptor)) = tls_listener_state {
        let app_tls = app.clone();
        let sm_tls = session_manager.clone();
        let eng_tls = proxy_engine.clone();
        let dns_tls = dns_overrides.clone();
        tokio::spawn(async move {
            loop {
                let (tcp_stream, _peer) = match tls_tcp.accept().await {
                    Ok(pair) => pair,
                    Err(e) => { tracing::warn!(error=%e, "HTTPS accept error"); continue; }
                };
                match tls_acceptor.accept(tcp_stream).await {
                    Ok(tls_stream) => {
                        let io = hyper_util::rt::TokioIo::new(tls_stream);
                        let app = app_tls.clone();
                        let sm = sm_tls.clone();
                        let engine = eng_tls.clone();
                        let dns = dns_tls.clone();
                        tokio::spawn(async move {
                            let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                                let app = app.clone();
                                let sm = sm.clone();
                                let engine = engine.clone();
                                let dns = dns.clone();
                                async move {
                                    if req.method() == hyper::Method::CONNECT {
                                        return Ok::<_, std::convert::Infallible>(handle_connect(req, sm, engine, dns).await);
                                    }
                                    if is_websocket_upgrade(&req) {
                                        let sid = uuid::Uuid::new_v4().to_string();
                                        return Ok::<_, std::convert::Infallible>(
                                            handle_websocket(req, sm, sid, inspect_ws_frames).await
                                        );
                                    }
                                    let req = req.map(axum::body::Body::new);
                                    Ok(app.oneshot(req).await.unwrap_or_else(|e| match e {}))
                                }
                            });
                            hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                                .serve_connection_with_upgrades(io, svc)
                                .await
                                .ok();
                        });
                    }
                    Err(e) => tracing::debug!(error=%e, "HTTPS TLS handshake failed"),
                }
            }
        });
    }

    loop {
        let (stream, peer) = tokio::select! {
            res = listener.accept() => match res {
                Ok(pair) => pair,
                Err(e) => { tracing::warn!(error=%e, "Accept error"); continue; }
            },
            _ = &mut shutdown => {
                tracing::info!("Shutdown signal received — stopping proxy");
                println!("Proxy stopped.");
                break;
            }
        };
        let _ = peer;
        let io = hyper_util::rt::TokioIo::new(stream);
        let app = app.clone();
        let sm = session_manager.clone();
        let engine = proxy_engine.clone();
        let dns = dns_overrides.clone();

        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                let app = app.clone();
                let sm = sm.clone();
                let engine = engine.clone();
                let dns = dns.clone();
                async move {
                    if req.method() == hyper::Method::CONNECT {
                        return Ok::<_, std::convert::Infallible>(handle_connect(req, sm, engine, dns).await);
                    }
                    if is_websocket_upgrade(&req) {
                        let sid = uuid::Uuid::new_v4().to_string();
                        return Ok::<_, std::convert::Infallible>(
                            handle_websocket(req, sm, sid, inspect_ws_frames).await
                        );
                    }
                    let req = req.map(axum::body::Body::new);
                    Ok(app.oneshot(req).await.unwrap_or_else(|e| match e {}))
                }
            });

            hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                .serve_connection_with_upgrades(io, svc)
                .await
                .ok();
        });
    }
    Ok(())
}

fn is_websocket_upgrade(req: &hyper::Request<hyper::body::Incoming>) -> bool {
    req.headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

/// Handles a raw CONNECT tunnel: records it in the session log and splices
/// the client socket to the upstream TCP connection.
async fn handle_connect(
    req: hyper::Request<hyper::body::Incoming>,
    sm: crate::session::SharedSessionManager,
    engine: Arc<ProxyEngine>,
    dns_overrides: Arc<RwLock<HashMap<String, String>>>,
) -> hyper::Response<axum::body::Body> {
    let host = req.uri().authority().map(|a| a.to_string()).unwrap_or_default();
    // Apply DNS override before connecting: replace IP while keeping original port.
    let addr = {
        let raw = if host.contains(':') { host.clone() } else { format!("{}:443", host) };
        let ovr = dns_overrides.read().await;
        if !ovr.is_empty() {
            let (hostname, port_part) = raw.split_once(':').unwrap_or((&raw, "443"));
            if let Some(ip) = ovr.get(hostname) {
                format!("{}:{}", ip, port_part)
            } else {
                raw
            }
        } else {
            raw
        }
    };
    let hostname = host.split(':').next().unwrap_or(&host).to_string();

    // In MITM mode each decrypted request gets its own session via InspectionMiddleware;
    // recording the outer CONNECT tunnel would produce confusing "MITM intercepted" entries.
    let is_mitm = engine.mitm_enabled && engine.ca.is_some();
    let session_id = uuid::Uuid::new_v4().to_string();
    if !is_mitm {
        sm.record_request(session_id.clone(), crate::middleware::RequestContext {
            method: "CONNECT".to_string(),
            uri: format!("https://{}", host),
            headers: std::collections::HashMap::new(),
            body: String::new(),
            host: host.clone(),
            body_bytes: None,
        });
    }

    let on_upgrade = hyper::upgrade::on(req);
    let start = std::time::Instant::now();

    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                if engine.mitm_enabled {
                    if let Some(ca) = engine.ca.clone() {
                        mitm_intercept(hyper_util::rt::TokioIo::new(upgraded), hostname.clone(), engine.clone(), ca).await;
                        return;
                    }
                }
                match tokio::net::TcpStream::connect(&addr).await {
                    Ok(mut upstream) => {
                        let mut io = hyper_util::rt::TokioIo::new(upgraded);
                        let result = tokio::io::copy_bidirectional(&mut io, &mut upstream).await;
                        let (to_server, to_client) = result.unwrap_or((0, 0));
                        sm.record_response_with_metrics(
                            session_id.clone(),
                            crate::middleware::ResponseContext {
                                status: 200,
                                headers: std::collections::HashMap::new(),
                                body: format!("↑{} ↓{}", fmt_bytes(to_server), fmt_bytes(to_client)),
                                request_uri: format!("https://{}", host),
                                session_id: Some(session_id),
                                ttfb_ms: 0,
                                body_ms: 0,
                                body_bytes: None,
                            },
                            crate::session::InspectionMetrics {
                                latency_ms: start.elapsed().as_millis() as u64,
                                request_size_bytes: to_server as usize,
                                response_size_bytes: to_client as usize,
                                status_code: 200,
                                ttfb_ms: 0,
                                body_ms: 0,
                                ..Default::default()
                            },
                        );
                    }
                    Err(e) => {
                        tracing::error!(error=%e, addr=%addr, "CONNECT upstream unreachable");
                        sm.record_response_with_metrics(
                            session_id.clone(),
                            crate::middleware::ResponseContext {
                                status: 502,
                                headers: std::collections::HashMap::new(),
                                body: format!("upstream unreachable: {}", e),
                                request_uri: format!("https://{}", host),
                                session_id: Some(session_id),
                                ttfb_ms: 0,
                                body_ms: 0,
                                body_bytes: None,
                            },
                            crate::session::InspectionMetrics {
                                latency_ms: start.elapsed().as_millis() as u64,
                                request_size_bytes: 0,
                                response_size_bytes: 0,
                                status_code: 502,
                                ttfb_ms: 0,
                                body_ms: 0,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            Err(e) => tracing::error!(error=%e, "CONNECT upgrade failed"),
        }
    });

    hyper::Response::builder()
        .status(200)
        .body(axum::body::Body::empty())
        .unwrap()
}

/// Perform a MITM TLS interception on any async I/O stream.
///
/// Used by both the HTTP CONNECT upgrade path (`hyper::upgrade::Upgraded` wrapped
/// in `TokioIo`) and the SOCKS5 path (raw `TcpStream`).  The caller is responsible
/// for wrapping `hyper::upgrade::Upgraded` in `hyper_util::rt::TokioIo` before
/// calling; `TcpStream` can be passed directly.
async fn mitm_intercept<IO>(
    io: IO,
    hostname: String,
    engine: Arc<ProxyEngine>,
    ca: Arc<crate::certs::CertificateAuthority>,
) where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (cert_der, key_der) = match ca.get_certificate_for_domain(&hostname).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!(error = %e, host = %hostname, "MITM cert generation failed");
            return;
        }
    };

    let cert_chain = vec![rustls::Certificate(cert_der)];
    let private_key = rustls::PrivateKey(key_der);
    let server_config = match rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert_chain, private_key)
    {
        Ok(cfg) => std::sync::Arc::new(cfg),
        Err(e) => {
            tracing::error!(error = %e, host = %hostname, "MITM TLS ServerConfig failed");
            return;
        }
    };

    let acceptor = tokio_rustls::TlsAcceptor::from(server_config);
    let tls_stream = match acceptor.accept(io).await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, host = %hostname, "MITM TLS accept failed (client may not trust CA)");
            return;
        }
    };

    let tls_io = hyper_util::rt::TokioIo::new(tls_stream);
    let engine_ref = engine.clone();
    let host_ref = hostname.clone();

    if let Err(e) = hyper::server::conn::http1::Builder::new()
        .serve_connection(
            tls_io,
            hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
                let eng = engine_ref.clone();
                let h = host_ref.clone();
                async move {
                    // Tell the engine to forward upstream over HTTPS.
                    if let Ok(v) = axum::http::HeaderValue::from_str(&format!("https://{}", h)) {
                        req.headers_mut().insert(
                            axum::http::header::HeaderName::from_static("x-oproxy-destination"),
                            v,
                        );
                    }
                    let req = req.map(axum::body::Body::new);
                    Ok::<_, std::convert::Infallible>(eng.handle_request(req).await)
                }
            }),
        )
        .await
    {
        tracing::debug!(error = %e, host = %hostname, "MITM connection closed");
    }
}

/// Async reader that drains a byte prefix before falling through to the inner reader.
/// Used to replay bytes already consumed from a TCP stream during header parsing.
struct LeadingBytesReader<R> {
    prefix: std::io::Cursor<Vec<u8>>,
    inner: R,
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for LeadingBytesReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let me = &mut *self;
        let pos = me.prefix.position() as usize;
        let data = me.prefix.get_ref();
        if pos < data.len() {
            let to_read = (data.len() - pos).min(buf.remaining());
            buf.put_slice(&data[pos..pos + to_read]);
            me.prefix.set_position((pos + to_read) as u64);
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut me.inner).poll_read(cx, buf)
    }
}

/// Reads one RFC 6455 WebSocket frame.
/// Returns `(opcode, decoded_payload, original_raw_bytes)`.
/// Raw bytes are forwarded intact (preserving client masking); decoded is for logging.
async fn read_ws_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<(u8, Vec<u8>, Vec<u8>)> {
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; 2];
    reader.read_exact(&mut header).await?;
    let b0 = header[0];
    let b1 = header[1];
    let opcode = b0 & 0x0F;
    let masked = (b1 & 0x80) != 0;
    let len7 = (b1 & 0x7F) as usize;
    let mut raw = vec![b0, b1];
    let payload_len = match len7 {
        126 => {
            let mut ext = [0u8; 2];
            reader.read_exact(&mut ext).await?;
            raw.extend_from_slice(&ext);
            u16::from_be_bytes(ext) as usize
        }
        127 => {
            let mut ext = [0u8; 8];
            reader.read_exact(&mut ext).await?;
            raw.extend_from_slice(&ext);
            (u64::from_be_bytes(ext) as usize).min(16 * 1024 * 1024)
        }
        n => n,
    };
    let mask_key = if masked {
        let mut key = [0u8; 4];
        reader.read_exact(&mut key).await?;
        raw.extend_from_slice(&key);
        Some(key)
    } else {
        None
    };
    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload).await?;
    raw.extend_from_slice(&payload);
    let decoded = if let Some(key) = mask_key {
        payload.iter().enumerate().map(|(i, &b)| b ^ key[i % 4]).collect()
    } else {
        payload
    };
    Ok((opcode, decoded, raw))
}

async fn relay_ws_frames<R, W>(
    mut reader: R,
    mut writer: W,
    sm: crate::session::SharedSessionManager,
    session_id: String,
    direction: crate::session::WsDirection,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use tokio::io::AsyncWriteExt;
    loop {
        let (opcode, decoded, raw) = match read_ws_frame(&mut reader).await {
            Ok(f) => f,
            Err(_) => break,
        };
        if writer.write_all(&raw).await.is_err() {
            break;
        }
        let payload_len = decoded.len();
        let (payload_text, payload_hex) = if opcode == 0x1 {
            let s = String::from_utf8_lossy(&decoded[..decoded.len().min(512)]).into_owned();
            (Some(s), None)
        } else {
            let chunk = &decoded[..decoded.len().min(64)];
            let mut hex = String::with_capacity(chunk.len() * 2);
            for b in chunk { use std::fmt::Write as _; let _ = write!(hex, "{:02x}", b); }
            (None, if hex.is_empty() { None } else { Some(hex) })
        };
        sm.append_ws_frame(&session_id, crate::session::WsFrame {
            timestamp: chrono::Utc::now(),
            direction: direction.clone(),
            opcode,
            payload_len,
            payload_text,
            payload_hex,
        });
        if opcode == 0x8 { break; }
    }
}

/// Proxies a plain WebSocket upgrade (ws://) by tunnelling via raw TCP.
async fn handle_websocket(
    req: hyper::Request<hyper::body::Incoming>,
    sm: crate::session::SharedSessionManager,
    session_id: String,
    inspect_frames: bool,
) -> hyper::Response<axum::body::Body> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let uri = req.uri().clone();
    let headers = req.headers().clone();

    let host_header = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let target_host = uri
        .host()
        .map(|s| s.to_string())
        .unwrap_or_else(|| host_header.split(':').next().unwrap_or("").to_string());
    let port: u16 = uri.port_u16().unwrap_or(80);
    let addr = format!("{}:{}", target_host, port);

    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    let mut raw_req = format!("GET {} HTTP/1.1\r\n", path_and_query);
    for (name, value) in &headers {
        if let Ok(v) = value.to_str() {
            raw_req.push_str(&format!("{}: {}\r\n", name, v));
        }
    }
    raw_req.push_str("\r\n");

    let mut upstream = match tokio::net::TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error=%e, addr=%addr, "WS upstream unreachable");
            return hyper::Response::builder()
                .status(502)
                .body(axum::body::Body::from("WebSocket upstream unreachable"))
                .unwrap();
        }
    };

    if let Err(e) = upstream.write_all(raw_req.as_bytes()).await {
        tracing::warn!(error=%e, "WS handshake send failed");
        return hyper::Response::builder()
            .status(502)
            .body(axum::body::Body::from("WS handshake send failed"))
            .unwrap();
    }

    // Read upstream 101 response headers (until \r\n\r\n).
    let mut header_buf: Vec<u8> = Vec::with_capacity(1024);
    let mut tmp = [0u8; 512];
    'read: loop {
        match upstream.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                header_buf.extend_from_slice(&tmp[..n]);
                for i in 0..header_buf.len().saturating_sub(3) {
                    if &header_buf[i..i + 4] == b"\r\n\r\n" {
                        break 'read;
                    }
                }
                if header_buf.len() > 16_384 {
                    break;
                }
            }
        }
    }

    let header_str = String::from_utf8_lossy(&header_buf);
    let first_line = header_str.lines().next().unwrap_or("");
    if !first_line.contains(" 101 ") {
        tracing::warn!(response=%first_line, addr=%addr, "WS upstream rejected upgrade");
        return hyper::Response::builder()
            .status(502)
            .body(axum::body::Body::from("Upstream did not switch protocols"))
            .unwrap();
    }

    // Build 101 response forwarding upstream headers to the client.
    let mut builder = hyper::Response::builder().status(101);
    for line in header_str.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(": ") {
            builder = builder.header(name, value);
        }
    }

    // Record WS session so append_ws_frame can find it by id.
    let mut req_headers_map = std::collections::HashMap::new();
    for (k, v) in &headers {
        if let Ok(v) = v.to_str() {
            req_headers_map.insert(k.to_string(), v.to_string());
        }
    }
    sm.record_request(session_id.clone(), crate::middleware::RequestContext {
        method: "WS".to_string(),
        uri: format!("ws://{}:{}{}", target_host, port, path_and_query),
        headers: req_headers_map,
        body: String::new(),
        host: target_host.clone(),
        body_bytes: None,
    });

    // Bytes read past \r\n\r\n belong to the WS data stream.
    let header_end = header_buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(header_buf.len());
    let leftover: Vec<u8> = header_buf[header_end..].to_vec();

    let on_upgrade = hyper::upgrade::on(req);
    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let mut client_io = hyper_util::rt::TokioIo::new(upgraded);
                if !inspect_frames {
                    if !leftover.is_empty() {
                        use tokio::io::AsyncWriteExt;
                        let _ = client_io.write_all(&leftover).await;
                    }
                    if let Err(e) = tokio::io::copy_bidirectional(&mut client_io, &mut upstream).await {
                        tracing::debug!(error=%e, "WS tunnel closed");
                    }
                    return;
                }
                let (client_read, client_write) = tokio::io::split(client_io);
                let (server_tcp_read, server_tcp_write) = upstream.into_split();
                let server_read = LeadingBytesReader {
                    prefix: std::io::Cursor::new(leftover),
                    inner: server_tcp_read,
                };
                let sm_c = sm.clone();
                let sid_c = session_id.clone();
                let mut task_c = tokio::spawn(relay_ws_frames(
                    client_read, server_tcp_write, sm_c, sid_c,
                    crate::session::WsDirection::ClientToServer,
                ));
                let mut task_s = tokio::spawn(relay_ws_frames(
                    server_read, client_write, sm, session_id,
                    crate::session::WsDirection::ServerToClient,
                ));
                tokio::select! {
                    _ = &mut task_c => { task_s.abort(); }
                    _ = &mut task_s => { task_c.abort(); }
                }
            }
            Err(e) => tracing::debug!(error=%e, "WS client upgrade failed"),
        }
    });

    builder
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| {
            hyper::Response::builder()
                .status(500)
                .body(axum::body::Body::empty())
                .unwrap()
        })
}

fn fmt_bytes(n: u64) -> String {
    if n < 1024 { format!("{n}B") }
    else if n < 1_048_576 { format!("{:.1}KB", n as f64 / 1024.0) }
    else { format!("{:.1}MB", n as f64 / 1_048_576.0) }
}
