use std::collections::HashMap;

use axum::body::Body;
use hyper::body::Incoming;
use hyper::{Request, Response};
use tokio::sync::watch;
use tokio::time::timeout;

use crate::transport::TransportContext;
use crate::transport::lifecycle::wait_for_shutdown;
use crate::transport::tls::mitm_intercept;

pub async fn handle_connect(
    req: Request<Incoming>,
    context: TransportContext,
    peer: Option<std::net::SocketAddr>,
    mut shutdown: watch::Receiver<bool>,
) -> Response<Body> {
    let sm = context.session_manager.clone();
    let engine = context.engine.clone();
    let dns_overrides = context.dns_overrides.clone();
    let connections = context.connections.clone();
    let connect_timeout = context.connect_timeout;
    let handshake_timeout = context.handshake_timeout;

    let host = req
        .uri()
        .authority()
        .map(|a| a.to_string())
        .unwrap_or_default();
    let addr = {
        let raw = if host.contains(':') {
            host.clone()
        } else {
            format!("{}:443", host)
        };
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

    let is_mitm = engine.mitm_enabled && engine.ca.is_some();
    let session_id = uuid::Uuid::new_v4().to_string();
    if !is_mitm {
        sm.record_request(
            session_id.clone(),
            crate::middleware::RequestContext {
                method: "CONNECT".to_string(),
                uri: format!("https://{}", host),
                headers: HashMap::new(),
                body: String::new(),
                host: host.clone(),
                body_bytes: None,
            },
        );
    }

    let on_upgrade = hyper::upgrade::on(req);
    let start = std::time::Instant::now();

    connections.spawn_tracked("connect-tunnel", peer, async move {
        let tunnel = async {
            match on_upgrade.await {
                Ok(upgraded) => {
                    if engine.mitm_enabled
                        && let Some(ca) = engine.ca.clone()
                    {
                        mitm_intercept(
                            hyper_util::rt::TokioIo::new(upgraded),
                            hostname.clone(),
                            engine.clone(),
                            ca,
                            handshake_timeout,
                        )
                        .await;
                        return;
                    }
                    match timeout(connect_timeout, tokio::net::TcpStream::connect(&addr)).await {
                        Ok(Ok(mut upstream)) => {
                            let mut io = hyper_util::rt::TokioIo::new(upgraded);
                            let result =
                                tokio::io::copy_bidirectional(&mut io, &mut upstream).await;
                            let (to_server, to_client) = result.unwrap_or((0, 0));
                            sm.record_response_with_metrics(
                                session_id.clone(),
                                crate::middleware::ResponseContext {
                                    status: 200,
                                    headers: HashMap::new(),
                                    body: format!(
                                        "↑{} ↓{}",
                                        fmt_bytes(to_server),
                                        fmt_bytes(to_client)
                                    ),
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
                        Ok(Err(e)) => {
                            tracing::error!(error=%e, addr=%addr, "CONNECT upstream unreachable");
                            sm.record_response_with_metrics(
                                session_id.clone(),
                                crate::middleware::ResponseContext {
                                    status: 502,
                                    headers: HashMap::new(),
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
                        Err(_) => {
                            tracing::error!(addr=%addr, timeout_secs=connect_timeout.as_secs(), "CONNECT upstream timed out");
                            sm.record_response_with_metrics(
                                session_id.clone(),
                                crate::middleware::ResponseContext {
                                    status: 504,
                                    headers: HashMap::new(),
                                    body: "upstream connect timed out".to_string(),
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
                                    status_code: 504,
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
        };
        tokio::pin!(tunnel);
        tokio::select! {
            _ = &mut tunnel => {}
            _ = wait_for_shutdown(&mut shutdown) => {
                tracing::debug!(host=%host, "CONNECT tunnel stopped by shutdown");
            }
        }
    });

    Response::builder().status(200).body(Body::empty()).unwrap()
}

fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n}B")
    } else if n < 1_048_576 {
        format!("{:.1}KB", n as f64 / 1024.0)
    } else {
        format!("{:.1}MB", n as f64 / 1_048_576.0)
    }
}
