use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::core::engine::ProxyEngine;

pub fn is_tls_port(port: u16) -> bool {
    matches!(port, 443 | 8443 | 4443)
}

pub async fn mitm_intercept<IO>(
    io: IO,
    hostname: String,
    engine: Arc<ProxyEngine>,
    ca: Arc<crate::certs::CertificateAuthority>,
    handshake_timeout: Duration,
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

    let cert_chain = vec![rustls::pki_types::CertificateDer::from(cert_der)];
    let private_key: rustls::pki_types::PrivateKeyDer<'static> =
        rustls::pki_types::PrivatePkcs8KeyDer::from(key_der).into();
    let server_config = match rustls::ServerConfig::builder()
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
    let tls_stream = match timeout(handshake_timeout, acceptor.accept(io)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::debug!(error = %e, host = %hostname, "MITM TLS accept failed (client may not trust CA)");
            return;
        }
        Err(_) => {
            tracing::debug!(host = %hostname, timeout_secs = handshake_timeout.as_secs(), "MITM TLS accept timed out");
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
