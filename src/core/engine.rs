use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::io::Read as _;
use tokio::sync::RwLock;
use bytes::Bytes;
use crate::middleware::chain::MiddlewareChain;
use crate::middleware::{RequestContext, ResponseContext, MiddlewareAction};
use flate2::read::GzDecoder;
use brotli::BrotliDecompress;
use base64::Engine as _;
use reqwest::Client;

// Responses larger than this are streamed rather than fully buffered.
const STREAM_THRESHOLD_BYTES: u64 = 512 * 1024; // 512 KB
use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    body::Body,
};
use tracing::{info, debug, error, instrument};
use std::time::Instant;

pub struct ProxyEngine {
    pub http_client: Client,
    pub streaming_client: Client, // no timeout — used for SSE / long-lived streams
    pub middleware_chain: Arc<RwLock<MiddlewareChain>>,
    pub ca: Option<Arc<crate::certs::CertificateAuthority>>,
    pub mitm_enabled: bool,
    max_body_bytes: Arc<AtomicUsize>,
}

impl ProxyEngine {
    pub fn new(
        middleware_chain: Arc<RwLock<MiddlewareChain>>,
        ca: Option<Arc<crate::certs::CertificateAuthority>>,
        mitm_enabled: bool,
        timeout_secs: u64,
        max_body_bytes: usize,
        pool_max_idle_per_host: usize,
        pool_idle_timeout_secs: u64,
    ) -> Self {
        let pool_idle = std::time::Duration::from_secs(pool_idle_timeout_secs);
        Self {
            http_client: Client::builder()
                .pool_max_idle_per_host(pool_max_idle_per_host)
                .pool_idle_timeout(pool_idle)
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build()
                .unwrap_or_else(|_| Client::new()),
            streaming_client: Client::builder()
                .pool_max_idle_per_host(pool_max_idle_per_host)
                .pool_idle_timeout(pool_idle)
                .build()
                .unwrap_or_else(|_| Client::new()),
            middleware_chain,
            ca,
            mitm_enabled,
            max_body_bytes: Arc::new(AtomicUsize::new(max_body_bytes)),
        }
    }

    /// Returns the current max body buffer size.
    pub fn max_body_bytes(&self) -> usize {
        self.max_body_bytes.load(Ordering::Relaxed)
    }

    /// Hot-updates the max body buffer size without restarting.
    pub fn set_max_body_bytes(&self, v: usize) {
        self.max_body_bytes.store(v, Ordering::Relaxed);
    }

    #[instrument(skip(self, req))]
    pub async fn handle_request(
        self: Arc<Self>,
        req: Request<Body>,
    ) -> Response {
        let start = Instant::now();
        let method = req.method().clone();
        let uri = req.uri().clone();
        let host = req
            .headers()
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        debug!(method = %method, uri = %uri, host = %host, "Processing request");

        // CONNECT is handled at the hyper service level in main.rs (before axum
        // middleware) so it never reaches here.  Return BAD_GATEWAY as a safety net.
        if method == axum::http::Method::CONNECT {
            return (StatusCode::BAD_GATEWAY, "CONNECT should be handled upstream").into_response();
        }

        let req_method = method.to_string();
        let req_uri = uri.to_string();
        let mut req_headers = std::collections::HashMap::new();
        for (name, value) in req.headers().iter() {
            req_headers.insert(name.to_string(), value.to_str().unwrap_or("").to_string());
        }
        
        let req_body_bytes = axum::body::to_bytes(req.into_body(), self.max_body_bytes())
            .await
            .unwrap_or_default();
        let req_body = String::from_utf8_lossy(&req_body_bytes).to_string();

        let mut req_ctx = RequestContext {
            method: req_method.clone(),
            uri: req_uri.clone(),
            headers: req_headers,
            body: req_body,
            host: host.clone(),
            // Store original bytes; middlewares clear this when they modify body.
            body_bytes: Some(req_body_bytes.clone()),
        };

        // Execute Request Middleware Chain
        {
            debug!("Executing request middleware chain");
            let chain = self.middleware_chain.read().await;
            let action = chain.execute_request(&mut req_ctx).await;
            match action {
                MiddlewareAction::Continue => {}
                MiddlewareAction::StopAndReturn => {
                    info!("Request stopped by middleware");
                    return (StatusCode::FORBIDDEN, "Request stopped by middleware").into_response()
                },
                MiddlewareAction::Pause => {
                    debug!("Request paused by breakpoint");
                    return (StatusCode::ACCEPTED, "Request paused by breakpoint").into_response()
                },
            }
        }

        // Strip internal proxy headers so they are never forwarded to the upstream target.
        // Read the session ID before removing it so we can pass it to ResponseContext for
        // exact session correlation in InspectionMiddleware::on_response.
        let destination = req_ctx.headers.remove("x-oproxy-destination");
        let oproxy_session_id = req_ctx.headers.remove("x-oproxy-session-id");
        // Remove Accept-Encoding so upstream always returns an uncompressed body that we
        // can store as readable UTF-8.  If the upstream ignores this and still sends a
        // compressed response we decompress it below before forwarding.
        req_ctx.headers.remove("accept-encoding");
        // Strip hop-by-hop headers — illegal in HTTP/2 and must not be forwarded.
        for hdr in &["connection", "keep-alive", "proxy-connection", "transfer-encoding",
                     "te", "trailer", "trailers", "upgrade"] {
            req_ctx.headers.remove(*hdr);
        }

        // In forward-proxy mode the browser sends an absolute URI as the request
        // target (e.g. GET http://api.example.com/path HTTP/1.1).  Concatenating
        // that onto the routing destination produces a malformed URL like
        // "https://dest.comhttp://api.example.com/path".
        //
        // We use the *typed* Uri object (preserved from before body consumption)
        // rather than string prefix matching, because http crate versions differ
        // in how to_string() serialises the scheme separator.  If the Uri has an
        // authority component it is an absolute URI; extract only path+query.
        let path_and_query: String = if uri.authority().is_some() {
            uri.path_and_query()
                .map(|pq| pq.as_str().to_string())
                .unwrap_or_else(|| "/".to_string())
        } else {
            // Reverse-proxy / origin-form request: URI is already a bare path.
            req_ctx.uri.clone()
        };

        let target_url = match destination {
            Some(ref dest) => {
                // Normalise: if the user entered a destination without a scheme
                // (e.g. "localhost:3000") reqwest would receive a relative URL and
                // fail with "relative URL without a base". Prepend http:// in that case.
                let base = dest.trim_end_matches('/');
                let base = if base.starts_with("http://") || base.starts_with("https://") {
                    base.to_string()
                } else {
                    format!("http://{}", base)
                };
                // Rewrite the Host header to match the remapped destination so the
                // upstream's virtual-host / SNI routing works correctly.
                if let Ok(url) = reqwest::Url::parse(&base) {
                    if let Some(dest_host) = url.host_str() {
                        let host_val = match url.port() {
                            Some(p) => format!("{}:{}", dest_host, p),
                            None => dest_host.to_string(),
                        };
                        req_ctx.headers.insert("host".to_string(), host_val);
                    }
                }
                format!("{}{}", base, path_and_query)
            }
            None => format!("http://{}{}", req_ctx.host, path_and_query),
        };
        debug!(url = %target_url, "Forwarding request");

        let mut target_headers = reqwest::header::HeaderMap::new();
        for (name, value) in &req_ctx.headers {
            if name != "host" {
                if let Ok(n) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) {
                    if let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
                        target_headers.insert(n, v);
                    }
                }
            }
        }

        // If body_bytes is still Some, no middleware cleared it → forward original bytes intact.
        // If a middleware modified body and cleared body_bytes, forward the modified string.
        let forward_req_body = match req_ctx.body_bytes {
            Some(b) => reqwest::Body::from(b),
            None => reqwest::Body::from(req_ctx.body),
        };

        // SSE and other event-stream requests need no timeout — they stream indefinitely.
        let is_sse = req_ctx.headers.get("accept")
            .map(|v| v.contains("text/event-stream"))
            .unwrap_or(false);
        let client = if is_sse { &self.streaming_client } else { &self.http_client };

        let net_start = Instant::now();
        let response = client
            .request(reqwest::Method::from_bytes(req_method.as_bytes()).unwrap(), &target_url)
            .headers(target_headers)
            .body(forward_req_body)
            .send()
            .await;

        match response {
            Ok(res) => {
                let ttfb_ms = net_start.elapsed().as_millis() as u64;
                let status = res.status().as_u16();
                let mut res_headers = std::collections::HashMap::new();
                for (name, value) in res.headers().iter() {
                    res_headers.insert(name.to_string(), value.to_str().unwrap_or("").to_string());
                }

                let content_type = res_headers.get("content-type").cloned().unwrap_or_default();

                // Streaming path: text/event-stream (SSE) or large response above threshold.
                // Check Content-Length if present; stream when body is too large to buffer.
                let content_length = res.content_length().unwrap_or(0);
                let force_stream = content_length > STREAM_THRESHOLD_BYTES;
                if content_type.contains("text/event-stream") || force_stream {
                    let mut res_ctx = ResponseContext {
                        status,
                        headers: res_headers.clone(),
                        body: String::new(),
                        request_uri: req_uri.clone(),
                        session_id: oproxy_session_id,
                        ttfb_ms,
                        body_ms: 0,
                        body_bytes: None,
                    };
                    {
                        let chain = self.middleware_chain.read().await;
                        chain.execute_response(&mut res_ctx).await;
                    }
                    let status_code = StatusCode::from_u16(res_ctx.status)
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    let mut builder = Response::builder().status(status_code);
                    for (name, value) in &res_ctx.headers {
                        builder = builder.header(name, value);
                    }
                    let stream_body = axum::body::Body::from_stream(async_stream::stream! {
                        let mut r = res;
                        loop {
                            match r.chunk().await {
                                Ok(Some(chunk)) => yield Ok::<_, reqwest::Error>(chunk),
                                _ => break,
                            }
                        }
                    });
                    return builder
                        .body(stream_body)
                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
                }

                let body_start = Instant::now();
                let res_bytes = res.bytes().await.unwrap_or_default();
                let body_ms = body_start.elapsed().as_millis() as u64;

                // Decompress gzip/deflate if the upstream ignored our stripped Accept-Encoding.
                // On success strip Content-Encoding and Content-Length so they match the decoded body.
                // Also keep the canonical bytes (decoded if gzip, raw otherwise) so we can forward
                // binary responses intact when no middleware modified the body.
                let encoding = res_headers.get("content-encoding").cloned().unwrap_or_default().to_lowercase();
                let (res_body, res_body_bytes_canonical) = if encoding.contains("gzip") || encoding.contains("deflate") {
                    let mut out = Vec::new();
                    let decoded = GzDecoder::new(&res_bytes[..]).read_to_end(&mut out).is_ok();
                    if decoded {
                        res_headers.remove("content-encoding");
                        res_headers.remove("content-length");
                        let s = String::from_utf8_lossy(&out).to_string();
                        let b = Bytes::from(out);
                        (s, b)
                    } else {
                        (String::from_utf8_lossy(&res_bytes).to_string(), res_bytes.clone())
                    }
                } else if encoding.contains("br") {
                    let mut out = Vec::new();
                    if BrotliDecompress(&mut &res_bytes[..], &mut out).is_ok() {
                        res_headers.remove("content-encoding");
                        res_headers.remove("content-length");
                        let s = String::from_utf8_lossy(&out).to_string();
                        let b = Bytes::from(out);
                        (s, b)
                    } else {
                        (String::from_utf8_lossy(&res_bytes).to_string(), res_bytes.clone())
                    }
                } else {
                    (String::from_utf8_lossy(&res_bytes).to_string(), res_bytes.clone())
                };

                // For binary content types, replace the lossy UTF-8 string with a
                // base64 representation so the UI can render it (e.g. display images).
                let res_body = if is_binary_content_type(&content_type) {
                    base64::engine::general_purpose::STANDARD.encode(&res_body_bytes_canonical)
                } else {
                    res_body
                };

                let mut res_ctx = ResponseContext {
                    status,
                    headers: res_headers,
                    body: res_body,
                    request_uri: req_uri.clone(),
                    session_id: oproxy_session_id,
                    ttfb_ms,
                    body_ms,
                    body_bytes: Some(res_body_bytes_canonical.clone()),
                };

                // Execute Response Middleware Chain
                {
                    debug!("Executing response middleware chain");
                    let chain = self.middleware_chain.read().await;
                    let action = chain.execute_response(&mut res_ctx).await;
                    match action {
                        MiddlewareAction::Continue => {}
                        MiddlewareAction::StopAndReturn => {
                            info!("Response stopped by middleware");
                            return (StatusCode::FORBIDDEN, "Response stopped by middleware").into_response()
                        },
                        MiddlewareAction::Pause => {
                            debug!("Response paused by breakpoint");
                            return (StatusCode::ACCEPTED, "Response paused by breakpoint").into_response()
                        },
                    }
                }

                let status_code = StatusCode::from_u16(res_ctx.status)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                
                let log_uri = req_uri.clone();
                info!(
                    method = %req_method, 
                    uri = %log_uri, 
                    status = status_code.as_u16(), 
                    latency_ms = start.elapsed().as_millis(), 
                    "Request completed"
                );

                let mut builder = Response::builder().status(status_code);
                for (name, value) in &res_ctx.headers {
                    builder = builder.header(name, value);
                }

                // If body_bytes is still Some, middleware didn't modify the body → forward intact bytes.
                let forward_res_body = match res_ctx.body_bytes {
                    Some(b) => Body::from(b),
                    None => Body::from(res_ctx.body),
                };

                builder
                    .body(forward_res_body)
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            }
            Err(e) => {
                error!(error = %e, "Proxy error");
                // Run on_response with a synthetic 502 so InspectionMiddleware records
                // the failed exchange instead of leaving it as a dangling "pending" session.
                let mut err_ctx = crate::middleware::ResponseContext {
                    status: 502,
                    headers: std::collections::HashMap::new(),
                    body: format!("Proxy error: {}", e),
                    request_uri: req_uri.clone(),
                    session_id: oproxy_session_id,
                    ttfb_ms: net_start.elapsed().as_millis() as u64,
                    body_ms: 0,
                    body_bytes: None,
                };
                {
                    let chain = self.middleware_chain.read().await;
                    chain.execute_response(&mut err_ctx).await;
                }
                (StatusCode::BAD_GATEWAY, "Error forwarding request").into_response()
            }
        }
    }

}

pub fn is_binary_content_type(ct: &str) -> bool {
    let ct = ct.split(';').next().unwrap_or("").trim();
    ct.starts_with("image/")
        || ct.starts_with("audio/")
        || ct.starts_with("video/")
        || ct == "application/octet-stream"
        || ct == "application/pdf"
        || ct == "application/wasm"
        || ct == "font/woff"
        || ct == "font/woff2"
}
