use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::core::engine::ProxyEngine;

/// Install the rustls crypto provider used by oproxy.
///
/// rustls 0.23 cannot auto-select a provider when multiple backends are present
/// in the dependency graph. That happens in `--all-features` builds because
/// reqwest/tokio-rustls pull aws-lc-rs while quinn pulls ring. Installing one
/// provider explicitly keeps runtime startup and lib tests deterministic.
pub(crate) fn install_default_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

pub fn is_tls_port(port: u16) -> bool {
    matches!(port, 443 | 8443 | 4443)
}

/// ALPN protocols advertised by the MITM interceptor, in preference order.
/// `h2` first so HTTP/2 (and therefore gRPC / RFC 8441 WebSockets) can be
/// negotiated, with `http/1.1` as the universal fallback.
pub fn mitm_alpn_protocols() -> Vec<Vec<u8>> {
    vec![b"h2".to_vec(), b"http/1.1".to_vec()]
}

/// Builds the per-host MITM `rustls::ServerConfig` with ALPN advertised.
/// Extracted so the ALPN/protocol policy is unit-testable without a live
/// TLS handshake.
pub fn build_mitm_server_config(
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
) -> Result<Arc<rustls::ServerConfig>, rustls::Error> {
    install_default_crypto_provider();
    let cert_chain = vec![rustls::pki_types::CertificateDer::from(cert_der)];
    let private_key: rustls::pki_types::PrivateKeyDer<'static> =
        rustls::pki_types::PrivatePkcs8KeyDer::from(key_der).into();
    let mut cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, private_key)?;
    cfg.alpn_protocols = mitm_alpn_protocols();
    Ok(Arc::new(cfg))
}

pub async fn mitm_intercept<IO>(
    io: IO,
    hostname: String,
    authority: String,
    engine: Arc<ProxyEngine>,
    ca: Arc<crate::certs::CertificateAuthority>,
    handshake_timeout: Duration,
) where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Some(tls_stream) = accept_mitm_tls(io, &hostname, &ca, handshake_timeout).await else {
        return;
    };

    let negotiated = tls_stream
        .get_ref()
        .1
        .alpn_protocol()
        .map(|protocol| String::from_utf8_lossy(protocol).into_owned())
        .unwrap_or_else(|| "http/1.1".to_string());
    tracing::debug!(host = %hostname, alpn = %negotiated, "MITM TLS established");

    serve_intercepted_http(tls_stream, hostname, authority, engine, negotiated).await;
}

async fn accept_mitm_tls<IO>(
    io: IO,
    hostname: &str,
    ca: &crate::certs::CertificateAuthority,
    handshake_timeout: Duration,
) -> Option<tokio_rustls::server::TlsStream<IO>>
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (cert_der, key_der) = ca
        .get_certificate_for_domain(hostname)
        .await
        .map_err(|error| {
            tracing::error!(%error, host = %hostname, "MITM cert generation failed");
        })
        .ok()?;
    let server_config = build_mitm_server_config(cert_der, key_der)
        .map_err(|error| {
            tracing::error!(%error, host = %hostname, "MITM TLS ServerConfig failed");
        })
        .ok()?;
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config);
    match timeout(handshake_timeout, acceptor.accept(io)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::debug!(error = %e, host = %hostname, "MITM TLS accept failed (client may not trust CA)");
            return None;
        }
        Err(_) => {
            tracing::debug!(host = %hostname, timeout_secs = handshake_timeout.as_secs(), "MITM TLS accept timed out");
            return None;
        }
    }
    .into()
}

async fn serve_intercepted_http<IO>(
    tls_stream: tokio_rustls::server::TlsStream<IO>,
    hostname: String,
    authority: String,
    engine: Arc<ProxyEngine>,
    negotiated: String,
) where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let tls_io = hyper_util::rt::TokioIo::new(tls_stream);
    let engine_ref = engine.clone();
    // Forward to the FULL CONNECT authority (host:port). Using just the hostname
    // here dropped non-443 ports, sending every intercepted request to :443 and
    // 502-ing for origins on other ports. The cert above still uses the bare
    // hostname for SNI/CN.
    let dest_ref = format!("https://{}", authority);
    // One downstream identity per intercepted connection so multiplexed
    // h2 streams to the same MITM'd origin group together.
    let conn = crate::transport::http::DownstreamConn::new();

    let svc = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
        let eng = engine_ref.clone();
        let dest = dest_ref.clone();
        req.extensions_mut().insert(conn.clone());
        async move {
            let req = req.map(axum::body::Body::new);
            Ok::<_, std::convert::Infallible>(
                eng.handle_request_with_destination(req, Some(dest)).await,
            )
        }
    });

    // hyper's auto-builder detects h2 vs h1.1 from the byte stream (the h2 PRI *
    // preface), NOT from the TLS ALPN result. Clients like gRPC that negotiate h2
    // via ALPN but then connect through an HTTP proxy sometimes skip the preface,
    // causing the auto-builder to serve h1.1 even though ALPN said h2. When we
    // know from ALPN that h2 was negotiated, use the h2-specific builder directly
    // so those clients get a proper h2 connection. Fall back to auto (with upgrade
    // support for WebSocket) for http/1.1 and unknown.
    if negotiated == "h2" {
        let builder =
            hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new());
        if let Err(e) = builder.serve_connection(tls_io, svc).await {
            tracing::debug!(error = %e, host = %hostname, "MITM h2 connection closed");
        }
    } else {
        let builder =
            hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
        if let Err(e) = builder.serve_connection_with_upgrades(tls_io, svc).await {
            tracing::debug!(error = %e, host = %hostname, "MITM connection closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn self_signed_der() -> (Vec<u8>, Vec<u8>) {
        let params =
            rcgen::CertificateParams::new(vec!["example.com".to_string()]).expect("cert params");
        let key = rcgen::KeyPair::generate().expect("keypair");
        let cert = params.self_signed(&key).expect("self-signed cert");
        (cert.der().to_vec(), key.serialize_der())
    }

    #[test]
    fn mitm_alpn_prefers_h2_then_http11() {
        // Order matters: rustls offers these to the client in preference order,
        // so h2 must come first for gRPC / RFC 8441 to be reachable.
        assert_eq!(
            mitm_alpn_protocols(),
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn mitm_server_config_advertises_h2_and_http11_alpn() {
        let (cert_der, key_der) = self_signed_der();
        let cfg = build_mitm_server_config(cert_der, key_der)
            .expect("server config should build from a valid self-signed cert");
        // The interceptor must advertise ALPN so HTTP/2 can be negotiated;
        // previously this list was empty and every site was forced to h1.
        assert_eq!(
            cfg.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }
}
