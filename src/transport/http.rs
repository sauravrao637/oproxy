use std::convert::Infallible;
use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use hyper::body::Incoming;
use hyper::{Request, Response};
use tokio::sync::watch;
use tower::ServiceExt as _;

use crate::transport::TransportContext;
use crate::transport::connect::handle_connect;
use crate::transport::lifecycle::wait_for_shutdown;
use crate::transport::websocket::{handle_websocket, is_websocket_upgrade};

#[derive(Clone, Copy, Debug)]
pub struct DownstreamPeer(pub SocketAddr);

#[derive(Clone)]
pub struct ProxyHttpService {
    app: Router,
    context: TransportContext,
}

impl ProxyHttpService {
    pub fn new(app: Router, context: TransportContext) -> Self {
        Self { app, context }
    }

    async fn handle(
        self,
        req: Request<Incoming>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<Response<Body>, Infallible> {
        let peer: Option<SocketAddr> = req.extensions().get::<DownstreamPeer>().map(|p| p.0);
        if req.method() == hyper::Method::CONNECT {
            return Ok(handle_connect(req, self.context, peer, shutdown).await);
        }

        if is_websocket_upgrade(&req) {
            let session_id = uuid::Uuid::new_v4().to_string();
            return Ok(handle_websocket(req, self.context, session_id, peer, shutdown).await);
        }

        let req = req.map(Body::new);
        Ok(self.app.oneshot(req).await.unwrap_or_else(|e| match e {}))
    }
}

pub async fn serve_http_connection<IO>(
    io: IO,
    service: ProxyHttpService,
    mut shutdown: watch::Receiver<bool>,
    listener: &'static str,
    peer: Option<SocketAddr>,
) where
    IO: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let request_shutdown = shutdown.clone();
    let svc = hyper::service::service_fn(move |mut req: Request<Incoming>| {
        if let Some(peer) = peer {
            req.extensions_mut().insert(DownstreamPeer(peer));
        }
        let service = service.clone();
        let shutdown = request_shutdown.clone();
        async move { service.handle(req, shutdown).await }
    });

    let builder =
        hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
    let connection = builder.serve_connection_with_upgrades(io, svc);
    tokio::pin!(connection);
    tokio::select! {
        result = &mut connection => {
            if let Err(e) = result {
                tracing::debug!(error=%e, listener, "HTTP connection closed");
            }
        }
        _ = wait_for_shutdown(&mut shutdown) => {
            tracing::debug!(listener, "HTTP connection stopped by shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::RwLock;

    use crate::core::engine::ProxyEngine;
    use crate::transport::lifecycle::ConnectionSupervisor;

    fn test_service() -> ProxyHttpService {
        let app = Router::new().route("/ok", get(|| async { "transport-ok" }));
        let engine = Arc::new(ProxyEngine::new(
            Arc::new(RwLock::new(crate::middleware::chain::MiddlewareChain::new())),
            None,
            false,
            30,
            10 * 1024 * 1024,
            10,
            30,
            None,
        ));
        let context = TransportContext {
            session_manager: Arc::new(crate::session::SessionManager::new(10_000)),
            connections: ConnectionSupervisor::new(100),
            engine,
            dns_overrides: Arc::new(RwLock::new(HashMap::new())),
            inspect_ws_frames: true,
            connect_timeout: Duration::from_millis(50),
            handshake_timeout: Duration::from_millis(50),
        };
        ProxyHttpService::new(app, context)
    }

    async fn serve_once(service: ProxyHttpService) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            let _shutdown_tx = shutdown_tx;
            let (stream, _) = listener.accept().await.unwrap();
            let io = hyper_util::rt::TokioIo::new(stream);
            serve_http_connection(io, service, shutdown_rx, "test", None).await;
        });
        addr
    }

    async fn raw_request(addr: std::net::SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset && !out.is_empty() => {
                    break;
                }
                Err(e) => panic!("raw test client read failed: {e}"),
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn proxy_http_service_forwards_regular_requests_to_axum_app() {
        let addr = serve_once(test_service()).await;
        let response = raw_request(
            addr,
            "GET /ok HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;

        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("transport-ok"), "{response}");
    }

    #[tokio::test]
    async fn proxy_http_service_routes_connect_before_axum_fallback() {
        let unused_upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_port = unused_upstream.local_addr().unwrap().port();
        drop(unused_upstream);

        let addr = serve_once(test_service()).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "CONNECT 127.0.0.1:{upstream_port} HTTP/1.1\r\nHost: 127.0.0.1:{upstream_port}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut response = Vec::new();
        let mut buf = [0u8; 128];
        loop {
            let n = match stream.read(&mut buf).await {
                Ok(n) => n,
                Err(e)
                    if e.kind() == std::io::ErrorKind::ConnectionReset && !response.is_empty() =>
                {
                    break;
                }
                Err(e) => panic!("CONNECT test client read failed: {e}"),
            };
            if n == 0 {
                break;
            }
            response.extend_from_slice(&buf[..n]);
            if response.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    }
}
