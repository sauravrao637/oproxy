/// HTTP/3 (QUIC) listener.
///
/// Compiled only with the `http3` Cargo feature. The listener accepts QUIC
/// connections, demultiplexes h3 request streams, normalises each request into
/// the same proxy-engine pipeline as the TCP listeners, and streams the
/// response back. Alt-Svc is injected at the engine level.
#[cfg(feature = "http3")]
mod inner {
    use std::{net::SocketAddr, sync::Arc};

    use axum::body::Body;
    use axum::http::Request;
    use bytes::Bytes;
    use futures_util::StreamExt;
    use h3::server::RequestStream;
    use tokio::sync::watch;

    use crate::{core::engine::ProxyEngine, transport::lifecycle::wait_for_shutdown};

    type H3RequestStream = RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>;

    // ── TLS / QUIC cert resolver ────────────────────────────────────────────

    /// Sync bridge between the async `CertificateAuthority` and the synchronous
    /// `rustls::server::ResolvesServerCert` trait required by quinn.
    struct QuicCertResolver {
        ca: Arc<crate::certs::CertificateAuthority>,
    }

    impl std::fmt::Debug for QuicCertResolver {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("QuicCertResolver").finish()
        }
    }

    impl rustls::server::ResolvesServerCert for QuicCertResolver {
        fn resolve(
            &self,
            client_hello: rustls::server::ClientHello<'_>,
        ) -> Option<Arc<rustls::sign::CertifiedKey>> {
            let sni = client_hello
                .server_name()
                .unwrap_or("localhost")
                .to_string();
            // Bridge async cert issuance to the synchronous rustls trait using
            // block_in_place — only safe on multi-thread tokio workers. On a
            // current-thread runtime block_in_place panics, so fail the handshake
            // gracefully instead of taking the process down.
            let on_multi_thread_runtime = tokio::runtime::Handle::try_current()
                .map(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
                .unwrap_or(false);
            if !on_multi_thread_runtime {
                tracing::warn!(
                    "HTTP/3 certificate resolution requires a multi-thread tokio runtime; \
                     refusing QUIC handshake"
                );
                return None;
            }
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.ca.get_certificate_for_domain(&sni))
            });
            let (cert_der, key_der) = result.ok()?;
            let cert = rustls::pki_types::CertificateDer::from(cert_der);
            let key_der = rustls::pki_types::PrivatePkcs8KeyDer::from(key_der);
            let key = rustls::crypto::ring::sign::any_supported_type(
                &rustls::pki_types::PrivateKeyDer::Pkcs8(key_der),
            )
            .ok()?;
            Some(Arc::new(rustls::sign::CertifiedKey::new(vec![cert], key)))
        }
    }

    /// Build a `quinn::ServerConfig` for MITM HTTP/3.
    pub fn build_quic_server_config(
        ca: Arc<crate::certs::CertificateAuthority>,
    ) -> Result<quinn::ServerConfig, Box<dyn std::error::Error + Send + Sync>> {
        let resolver = Arc::new(QuicCertResolver { ca });
        let mut tls_cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver);
        tls_cfg.alpn_protocols = vec![b"h3".to_vec()];
        let quic_cfg = quinn::crypto::rustls::QuicServerConfig::try_from(tls_cfg)?;
        Ok(quinn::ServerConfig::with_crypto(Arc::new(quic_cfg)))
    }

    // ── Listener ────────────────────────────────────────────────────────────

    /// Bind a UDP socket for the HTTP/3 QUIC listener.
    pub async fn bind_h3_listener(
        bind_host: &str,
        port: u16,
        ca: Arc<crate::certs::CertificateAuthority>,
    ) -> Result<quinn::Endpoint, Box<dyn std::error::Error + Send + Sync>> {
        let server_cfg = build_quic_server_config(ca)?;
        let addr: SocketAddr = format!("{}:{}", bind_host, port).parse()?;
        let endpoint = quinn::Endpoint::server(server_cfg, addr)?;
        Ok(endpoint)
    }

    /// Accept QUIC connections and serve each request stream through the proxy engine.
    pub async fn run_h3_listener(
        endpoint: quinn::Endpoint,
        engine: Arc<ProxyEngine>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        loop {
            let incoming = tokio::select! {
                conn = endpoint.accept() => match conn {
                    Some(c) => c,
                    None => {
                        tracing::info!("HTTP/3 endpoint closed");
                        break;
                    }
                },
                _ = wait_for_shutdown(&mut shutdown) => {
                    tracing::info!("HTTP/3 listener stopped");
                    endpoint.close(0u32.into(), b"server shutdown");
                    break;
                }
            };

            let engine = engine.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                handle_quic_connection(incoming, engine, shutdown).await;
            });
        }
    }

    async fn handle_quic_connection(
        incoming: quinn::Incoming,
        engine: Arc<ProxyEngine>,
        shutdown: watch::Receiver<bool>,
    ) {
        let conn = match incoming.await {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error=%e, "QUIC connection setup failed");
                return;
            }
        };

        let h3_conn = h3_quinn::Connection::new(conn);
        let h3_server: h3::server::Connection<_, Bytes> =
            match h3::server::builder().build(h3_conn).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error=%e, "h3 server connection setup failed");
                    return;
                }
            };

        accept_h3_requests(h3_server, engine, shutdown).await;
    }

    async fn accept_h3_requests(
        mut server: h3::server::Connection<h3_quinn::Connection, Bytes>,
        engine: Arc<ProxyEngine>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let conn_identity = crate::transport::http::DownstreamConn::new();
        loop {
            let resolver = tokio::select! {
                res = server.accept() => res,
                _ = wait_for_shutdown(&mut shutdown) => break,
            };
            let resolver = match resolver {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(error=%e, "h3 accept error");
                    break;
                }
            };

            let engine = engine.clone();
            let conn_identity = conn_identity.clone();
            tokio::spawn(async move {
                let (req, stream) = match resolver.resolve_request().await {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::debug!(error=%e, "h3 resolve_request failed");
                        return;
                    }
                };
                handle_h3_request(req, stream, engine, conn_identity).await;
            });
        }
    }

    async fn handle_h3_request(
        req: Request<()>,
        mut stream: H3RequestStream,
        engine: Arc<ProxyEngine>,
        conn_identity: crate::transport::http::DownstreamConn,
    ) {
        let (parts, _) = req.into_parts();
        let Some(body_bytes) = read_request_body(&mut stream, engine.max_body_bytes()).await else {
            return;
        };

        let mut axum_req = match build_axum_request(parts, body_bytes) {
            Some(r) => r,
            None => {
                let resp = axum::http::Response::builder()
                    .status(400)
                    .body(())
                    .expect("static 400 response is always valid");
                let _ = stream.send_response(resp).await;
                let _ = stream.finish().await;
                return;
            }
        };
        // Attach the per-connection identity so the engine records connection_id /
        // stream_id for this h3 stream.
        axum_req.extensions_mut().insert(conn_identity);

        let response = engine.handle_request_with_destination(axum_req, None).await;

        send_h3_response(&mut stream, response).await;
    }

    async fn read_request_body(
        stream: &mut H3RequestStream,
        max_body_bytes: usize,
    ) -> Option<Bytes> {
        let mut body = Vec::new();
        while let Ok(Some(chunk)) = stream.recv_data().await {
            use bytes::Buf;
            let mut chunk = chunk;
            let chunk = chunk.copy_to_bytes(chunk.remaining());
            if body.len().saturating_add(chunk.len()) > max_body_bytes {
                tracing::warn!(max_body_bytes, "h3 request body exceeded limit");
                let response = axum::http::Response::builder()
                    .status(413)
                    .body(())
                    .expect("static 413 response is always valid");
                let _ = stream.send_response(response).await;
                let _ = stream.finish().await;
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        Some(Bytes::from(body))
    }

    async fn send_h3_response(stream: &mut H3RequestStream, response: axum::response::Response) {
        let (resp_parts, resp_body) = response.into_parts();
        let mut h3_resp_builder = axum::http::Response::builder().status(resp_parts.status);
        for (k, v) in &resp_parts.headers {
            // Skip hop-by-hop headers that aren't valid in h3.
            let name = k.as_str().to_lowercase();
            if matches!(
                name.as_str(),
                "transfer-encoding" | "connection" | "keep-alive" | "upgrade"
            ) {
                continue;
            }
            h3_resp_builder = h3_resp_builder.header(k, v);
        }
        let h3_response = h3_resp_builder.body(()).unwrap_or_else(|_| {
            axum::http::Response::builder()
                .status(500)
                .body(())
                .expect("static 500 response is always valid")
        });

        if stream.send_response(h3_response).await.is_err() {
            return;
        }

        // Pipe the axum response body to h3 DATA frames.
        let mut body_stream = resp_body.into_data_stream();
        while let Some(chunk) = body_stream.next().await {
            match chunk {
                Ok(data) if !data.is_empty() => {
                    if stream.send_data(data).await.is_err() {
                        return;
                    }
                }
                _ => break,
            }
        }
        let _ = stream.finish().await;
    }

    fn build_axum_request(
        parts: axum::http::request::Parts,
        body: Bytes,
    ) -> Option<axum::http::Request<Body>> {
        let mut builder = axum::http::Request::builder()
            .method(parts.method.as_str())
            .version(axum::http::Version::HTTP_3)
            .uri(parts.uri.to_string());

        for (k, v) in &parts.headers {
            if k.as_str().starts_with(':') {
                continue;
            }
            builder = builder.header(k, v);
        }

        builder.body(Body::from(body)).ok()
    }
}

#[cfg(feature = "http3")]
pub use inner::{bind_h3_listener, run_h3_listener};
