use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use hyper::body::Incoming;
use hyper::{Request, Response};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::sync::watch;
use tokio::time::timeout;

use crate::transport::TransportContext;
use crate::transport::lifecycle::wait_for_shutdown;
use crate::transport::tls::mitm_intercept;

/// First byte of a TLS record carrying a handshake (ClientHello). Used to tell
/// a real TLS connection from a cleartext CONNECT tunnel (e.g. `ws://`).
const TLS_HANDSHAKE_RECORD: u8 = 0x16;

struct ConnectTarget {
    authority: String,
    hostname: String,
    address: String,
}

enum TunnelOutcome {
    Connected { bytes_up: u64, bytes_down: u64 },
    Unreachable(String),
    TimedOut,
}

impl ConnectTarget {
    async fn resolve(req: &Request<Incoming>, context: &TransportContext) -> Self {
        let authority = req
            .uri()
            .authority()
            .map(ToString::to_string)
            .unwrap_or_default();
        let raw_address = if authority.contains(':') {
            authority.clone()
        } else {
            format!("{authority}:443")
        };
        let (hostname, port) = raw_address.split_once(':').unwrap_or((&raw_address, "443"));
        let hostname = hostname.to_string();
        let port = port.to_string();
        let address = context
            .dns_overrides
            .read()
            .await
            .get(&hostname)
            .filter(|entry| entry.enabled)
            .map_or(raw_address, |entry| format!("{}:{port}", entry.ip));
        Self {
            hostname,
            authority,
            address,
        }
    }
}

fn connect_request_context(
    target: &ConnectTarget,
    scheme: &str,
) -> crate::middleware::RequestContext {
    crate::middleware::RequestContext {
        method: "CONNECT".to_string(),
        uri: format!("{scheme}://{}", target.authority),
        host: target.authority.clone(),
        ..Default::default()
    }
}

fn record_tunnel_outcome(
    session_manager: &crate::session::SharedSessionManager,
    session_id: String,
    target: &ConnectTarget,
    started_at: std::time::Instant,
    outcome: TunnelOutcome,
) {
    let (status, body, bytes_up, bytes_down) = match outcome {
        TunnelOutcome::Connected {
            bytes_up,
            bytes_down,
        } => (
            200,
            format!("↑{} ↓{}", fmt_bytes(bytes_up), fmt_bytes(bytes_down)),
            bytes_up,
            bytes_down,
        ),
        TunnelOutcome::Unreachable(error) => (502, format!("upstream unreachable: {error}"), 0, 0),
        TunnelOutcome::TimedOut => (504, "upstream connect timed out".to_string(), 0, 0),
    };
    session_manager.record_response_with_metrics(
        session_id.clone(),
        crate::middleware::ResponseContext {
            status,
            body: bytes::Bytes::from(body),
            request_uri: format!("https://{}", target.authority),
            session_id: Some(session_id),
            ..Default::default()
        },
        crate::session::InspectionMetrics {
            latency_ms: started_at.elapsed().as_millis() as u64,
            request_size_bytes: bytes_up as usize,
            response_size_bytes: bytes_down as usize,
            status_code: status,
            ..Default::default()
        },
    );
}

/// Wraps a stream so that a previously-read prefix is replayed before the inner
/// bytes. Lets us peek the first byte to detect TLS and then hand the *whole*
/// stream (prefix included) to either the MITM interceptor or a plain tunnel.
struct PrefixedIo<IO> {
    prefix: Vec<u8>,
    pos: usize,
    inner: IO,
}

impl<IO> PrefixedIo<IO> {
    fn new(prefix: Vec<u8>, inner: IO) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<IO: AsyncRead + Unpin> AsyncRead for PrefixedIo<IO> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            let start = self.pos;
            buf.put_slice(&self.prefix[start..start + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for PrefixedIo<IO> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub async fn handle_connect(
    req: Request<Incoming>,
    context: TransportContext,
    peer: Option<std::net::SocketAddr>,
    mut shutdown: watch::Receiver<bool>,
) -> Response<Body> {
    let sm = context.session_manager.clone();
    let engine = context.engine.clone();
    let connections = context.connections.clone();
    let connect_timeout = context.connect_timeout;
    let handshake_timeout = context.handshake_timeout;

    let target = ConnectTarget::resolve(&req, &context).await;

    let is_mitm = engine.mitm_enabled && engine.ca.is_some();
    let session_id = uuid::Uuid::new_v4().to_string();
    if !is_mitm {
        sm.record_request(
            session_id.clone(),
            connect_request_context(&target, "https"),
        );
    }

    let on_upgrade = hyper::upgrade::on(req);
    let start = std::time::Instant::now();

    connections.spawn_tracked("connect-tunnel", peer, async move {
        let tunnel = async {
            match on_upgrade.await {
                Ok(upgraded) => {
                    // When MITM is enabled, peek the first byte: a TLS ClientHello
                    // (0x16) is intercepted; anything else is a cleartext CONNECT
                    // tunnel (e.g. a ws:// upgrade) that must be tunnelled as-is,
                    // not fed to the TLS acceptor (which would consume the first
                    // bytes and break the connection). The peeked byte is replayed
                    // via PrefixedIo so no data is lost.
                    let stream = {
                        let mut io = hyper_util::rt::TokioIo::new(upgraded);
                        if engine.mitm_enabled
                            && engine.ca.is_some()
                        {
                            // Read the first available chunk (not a single byte) so
                            // hyper's Upgraded read-ahead is captured whole, then
                            // replay it via PrefixedIo. Only the first byte is used
                            // for detection: 0x16 = TLS handshake record.
                            let mut peek = vec![0u8; 8192];
                            let n = match io.read(&mut peek).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => n,
                            };
                            peek.truncate(n);
                            let is_tls = peek[0] == TLS_HANDSHAKE_RECORD;
                            let prefixed = PrefixedIo::new(peek, io);
                            if is_tls && let Some(ca) = engine.ca.clone() {
                                mitm_intercept(
                                    prefixed,
                                    target.hostname.clone(),
                                    target.authority.clone(),
                                    engine.clone(),
                                    ca,
                                    handshake_timeout,
                                )
                                .await;
                                return;
                            }
                            // MITM mode: TLS detection skipped the initial record_request.
                            // Record the cleartext tunnel as a CONNECT session now so it
                            // appears in the dashboard (e.g. h2c gRPC, ws://). Labelled
                            // http:// — this tunnel is explicitly NOT TLS.
                            sm.record_request(
                                session_id.clone(),
                                connect_request_context(&target, "http"),
                            );
                            tracing::debug!(host=%target.hostname, "CONNECT tunnel is not TLS; tunnelling without MITM");
                            prefixed
                        } else {
                            PrefixedIo::new(Vec::new(), io)
                        }
                    };
                    let outcome = match timeout(
                        connect_timeout,
                        tokio::net::TcpStream::connect(&target.address),
                    )
                    .await
                    {
                        Ok(Ok(mut upstream)) => {
                            let mut io = stream;
                            let result =
                                tokio::io::copy_bidirectional(&mut io, &mut upstream).await;
                            let (to_server, to_client) = result.unwrap_or((0, 0));
                            TunnelOutcome::Connected {
                                bytes_up: to_server,
                                bytes_down: to_client,
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::error!(error=%e, addr=%target.address, "CONNECT upstream unreachable");
                            TunnelOutcome::Unreachable(e.to_string())
                        }
                        Err(_) => {
                            tracing::error!(addr=%target.address, timeout_secs=connect_timeout.as_secs(), "CONNECT upstream timed out");
                            TunnelOutcome::TimedOut
                        }
                    };
                    record_tunnel_outcome(&sm, session_id.clone(), &target, start, outcome);
                }
                Err(e) => tracing::error!(error=%e, "CONNECT upgrade failed"),
            }
        };
        tokio::pin!(tunnel);
        tokio::select! {
            _ = &mut tunnel => {}
            _ = wait_for_shutdown(&mut shutdown) => {
                tracing::debug!(host=%target.authority, "CONNECT tunnel stopped by shutdown");
            }
        }
    });

    Response::builder()
        .status(200)
        .body(Body::empty())
        .expect("static 200 response is always valid")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prefixed_io_replays_prefix_then_inner() {
        // The peeked first byte(s) must be replayed ahead of the rest of the
        // stream so the downstream tunnel/MITM sees the complete handshake.
        let inner: &[u8] = b"GET / HTTP/1.1\r\n";
        let mut s = PrefixedIo::new(vec![0x16, 0x03], inner);
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        assert_eq!(&out[..2], &[0x16, 0x03], "prefix replayed first");
        assert_eq!(&out[2..], inner, "inner bytes follow intact");
    }

    #[tokio::test]
    async fn prefixed_io_empty_prefix_is_passthrough() {
        let inner: &[u8] = b"hello";
        let mut s = PrefixedIo::new(Vec::new(), inner);
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        assert_eq!(&out, inner);
    }

    // Reproduces the MITM CONNECT path: peek the first byte of a TLS ClientHello,
    // then complete a real rustls handshake through PrefixedIo. Guards against the
    // wrapper corrupting the TLS stream.
    #[tokio::test]
    async fn prefixed_io_completes_a_real_tls_handshake() {
        use tokio::io::AsyncWriteExt;
        use tokio_rustls::{TlsAcceptor, TlsConnector};

        crate::transport::tls::install_default_crypto_provider();

        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).expect("params");
        let key = rcgen::KeyPair::generate().expect("key");
        let cert = params.self_signed(&key).expect("cert");
        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let key_der: rustls::pki_types::PrivateKeyDer<'static> =
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into();

        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .expect("server cfg");
        let acceptor = TlsAcceptor::from(std::sync::Arc::new(server_config));

        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert_der).unwrap();
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(std::sync::Arc::new(client_config));

        let (client_io, mut server_io) = tokio::io::duplex(64 * 1024);

        let server = tokio::spawn(async move {
            let mut first = [0u8; 1];
            server_io.read_exact(&mut first).await.expect("peek byte");
            assert_eq!(
                first[0], 0x16,
                "ClientHello begins with a TLS handshake record"
            );
            let prefixed = PrefixedIo::new(first.to_vec(), server_io);
            let mut tls = acceptor.accept(prefixed).await.expect("server handshake");
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await.expect("read app data");
            tls.write_all(&buf).await.expect("echo");
            tls.flush().await.expect("flush");
        });

        let domain = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut tls = connector
            .connect(domain, client_io)
            .await
            .expect("client handshake");
        tls.write_all(b"ping").await.unwrap();
        tls.flush().await.unwrap();
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping", "app data survives the prefixed handshake");
        server.await.unwrap();
    }
}
