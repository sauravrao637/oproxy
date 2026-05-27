use std::net::SocketAddr;
use std::sync::Arc;

use crate::certs::CertificateAuthority;
use crate::config::Config;

use super::StartupError;

use std::time::Duration;

use tokio::sync::watch;
use tokio::time::timeout;

use crate::transport::http::{ProxyHttpService, serve_http_connection};
use crate::transport::lifecycle::wait_for_shutdown;
use crate::transport::socks5::ProxySocks5Service;

use super::supervisor::RuntimeSupervisor;

pub(super) async fn bind_http_listener(
    config: &Config,
) -> Result<tokio::net::TcpListener, StartupError> {
    let addr_str = format!("{}:{}", config.bind_host, config.port);
    let addr: SocketAddr = addr_str.parse().map_err(|e| StartupError::InvalidAddr {
        addr: addr_str.clone(),
        source: e,
    })?;
    let listener =
        tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| StartupError::BindFailed {
                addr: addr_str,
                source: e,
            })?;
    println!("oproxy listening on http://{}", addr);
    Ok(listener)
}

pub(super) async fn bind_https_listener(
    config: &Config,
    ca: &Arc<CertificateAuthority>,
) -> Result<Option<(tokio::net::TcpListener, tokio_rustls::TlsAcceptor)>, StartupError> {
    let Some(https_port) = config.https_port else {
        return Ok(None);
    };

    match ca.get_certificate_for_domain("localhost").await {
        Ok((cert_der, key_der)) => {
            let cert_chain = vec![rustls::pki_types::CertificateDer::from(cert_der)];
            let private_key: rustls::pki_types::PrivateKeyDer<'static> =
                rustls::pki_types::PrivatePkcs8KeyDer::from(key_der).into();
            match rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert_chain, private_key)
            {
                Ok(tls_cfg) => {
                    let tls_addr_str = format!("{}:{}", config.bind_host, https_port);
                    let tls_addr: SocketAddr =
                        tls_addr_str
                            .parse()
                            .map_err(|e| StartupError::InvalidAddr {
                                addr: tls_addr_str,
                                source: e,
                            })?;
                    match tokio::net::TcpListener::bind(tls_addr).await {
                        Ok(tls_l) => {
                            println!("oproxy HTTPS listener on https://{}", tls_addr);
                            Ok(Some((
                                tls_l,
                                tokio_rustls::TlsAcceptor::from(Arc::new(tls_cfg)),
                            )))
                        }
                        Err(e) => {
                            tracing::warn!(error=%e, "Failed to bind HTTPS listener, continuing without it");
                            Ok(None)
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error=%e, "Failed to build TLS config for HTTPS listener");
                    Ok(None)
                }
            }
        }
        Err(e) => {
            tracing::warn!(error=%e, "Failed to generate localhost cert for HTTPS listener");
            Ok(None)
        }
    }
}

pub(super) async fn bind_socks5_listener(
    config: &Config,
) -> Result<Option<tokio::net::TcpListener>, StartupError> {
    let Some(socks5_port) = config.socks5_port else {
        return Ok(None);
    };

    let addr_str = format!("{}:{}", config.bind_host, socks5_port);
    let addr: SocketAddr = addr_str.parse().map_err(|e| StartupError::InvalidAddr {
        addr: addr_str.clone(),
        source: e,
    })?;
    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            println!("oproxy SOCKS5 listener on socks5://{}", addr);
            Ok(Some(listener))
        }
        Err(e) => {
            tracing::warn!(error=%e, port=socks5_port, "Failed to bind SOCKS5 listener, continuing without it");
            Ok(None)
        }
    }
}

pub(super) fn spawn_http_listener(
    listener: tokio::net::TcpListener,
    http_service: ProxyHttpService,
    shutdown_rx: watch::Receiver<bool>,
    supervisor: &mut RuntimeSupervisor,
) {
    let connections = supervisor.connections();
    let mut shutdown_rx = shutdown_rx;
    supervisor.spawn_listener("http", async move {
        loop {
            let (stream, peer) = match tokio::select! {
                res = listener.accept() => res,
                _ = wait_for_shutdown(&mut shutdown_rx) => {
                    tracing::info!("HTTP listener stopped");
                    break;
                }
            } {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error=%e, "HTTP accept error");
                    continue;
                }
            };
            tracing::debug!(peer = %peer, "new connection");
            let Some(conn_permit) = connections.try_acquire("http", Some(peer)) else {
                continue;
            };
            let io = hyper_util::rt::TokioIo::new(stream);
            let service = http_service.clone();
            let conn_shutdown = shutdown_rx.clone();

            connections.spawn_connection("http", Some(peer), conn_permit, async move {
                serve_http_connection(io, service, conn_shutdown, "http", Some(peer)).await;
            });
        }
    });
}

pub(super) fn spawn_https_listener(
    tls_listener_state: Option<(tokio::net::TcpListener, tokio_rustls::TlsAcceptor)>,
    http_service: ProxyHttpService,
    shutdown_rx: watch::Receiver<bool>,
    handshake_timeout: Duration,
    supervisor: &mut RuntimeSupervisor,
) {
    let Some((tls_tcp, tls_acceptor)) = tls_listener_state else {
        return;
    };

    let connections = supervisor.connections();
    let mut shutdown_rx = shutdown_rx;

    supervisor.spawn_listener("https", async move {
        loop {
            let (tcp_stream, peer) = match tokio::select! {
                res = tls_tcp.accept() => res,
                _ = wait_for_shutdown(&mut shutdown_rx) => {
                    tracing::info!("HTTPS listener stopped");
                    break;
                }
            } {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error=%e, "HTTPS accept error");
                    continue;
                }
            };
            let Some(conn_permit) = connections.try_acquire("https", Some(peer)) else {
                continue;
            };
            match timeout(handshake_timeout, tls_acceptor.accept(tcp_stream)).await {
                Ok(Ok(tls_stream)) => {
                    let io = hyper_util::rt::TokioIo::new(tls_stream);
                    let service = http_service.clone();
                    let conn_shutdown = shutdown_rx.clone();
                    connections.spawn_connection("https", Some(peer), conn_permit, async move {
                        serve_http_connection(io, service, conn_shutdown, "https", Some(peer))
                            .await;
                    });
                }
                Ok(Err(e)) => tracing::debug!(error=%e, "HTTPS TLS handshake failed"),
                Err(_) => tracing::debug!("HTTPS TLS handshake timed out"),
            }
        }
    });
}

pub fn spawn_socks5_listener(
    listener: Option<tokio::net::TcpListener>,
    service: ProxySocks5Service,
    shutdown_rx: watch::Receiver<bool>,
    supervisor: &mut RuntimeSupervisor,
) {
    let Some(socks5_listener) = listener else {
        return;
    };

    let connections = supervisor.connections();
    supervisor.spawn_listener("socks5", async move {
        loop {
            let mut shutdown_rx = shutdown_rx.clone();

            let (stream, peer) = match tokio::select! {
                res = socks5_listener.accept() => res,
                _ = wait_for_shutdown(&mut shutdown_rx) => {
                    tracing::info!("SOCKS5 listener stopped");
                    break;
                }
            } {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error=%e, "SOCKS5 accept error");
                    continue;
                }
            };
            let Some(conn_permit) = connections.try_acquire("socks5", Some(peer)) else {
                continue;
            };

            let conn_service = service.clone();
            connections.spawn_connection("socks5", Some(peer), conn_permit, async move {
                conn_service.serve_connection(stream, shutdown_rx).await;
            });
        }
    });
}
