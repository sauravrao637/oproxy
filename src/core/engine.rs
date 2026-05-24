use crate::middleware::chain::MiddlewareChain;
use crate::middleware::{MiddlewareAction, RequestContext, ResponseContext};
use base64::Engine as _;
use brotli::BrotliDecompress;
use bytes::Bytes;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;

// Responses larger than this are streamed rather than fully buffered.
const STREAM_THRESHOLD_BYTES: u64 = 512 * 1024; // 512 KB
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use std::time::Instant;
use tracing::{debug, error, info, instrument};

fn display_request_uri(
    uri: &axum::http::Uri,
    headers: &std::collections::HashMap<String, String>,
    host: &str,
) -> String {
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

    if let Some(destination) = headers.get("x-oproxy-destination") {
        let base = destination.trim_end_matches('/');
        return format!("{}{}", base, path_and_query);
    }

    if uri.scheme().is_some() && uri.authority().is_some() {
        return uri.to_string();
    }

    format!("http://{}{}", host, path_and_query)
}

fn header_value(headers: &std::collections::HashMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn remove_header(headers: &mut std::collections::HashMap<String, String>, name: &str) {
    let keys: Vec<_> = headers
        .keys()
        .filter(|header_name| header_name.eq_ignore_ascii_case(name))
        .cloned()
        .collect();
    for key in keys {
        headers.remove(&key);
    }
}

fn read_decoder_to_bytes<R: std::io::Read>(mut reader: R) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    reader.read_to_end(&mut out).ok().map(|_| out)
}

fn decode_deflate(bytes: &[u8]) -> Option<Vec<u8>> {
    // HTTP "deflate" is zlib-wrapped deflate per RFC 9110. Some servers send
    // raw deflate, so keep that fallback for interoperability.
    read_decoder_to_bytes(ZlibDecoder::new(bytes))
        .or_else(|| read_decoder_to_bytes(DeflateDecoder::new(bytes)))
}

fn decoded_response_body(
    res_headers: &mut std::collections::HashMap<String, String>,
    res_bytes: &Bytes,
) -> (String, Bytes) {
    let encoding = header_value(res_headers, "content-encoding")
        .unwrap_or_default()
        .to_lowercase();

    let decoded = if encoding.contains("gzip") {
        read_decoder_to_bytes(GzDecoder::new(&res_bytes[..]))
    } else if encoding.contains("deflate") {
        decode_deflate(res_bytes)
    } else if encoding.contains("br") {
        let mut out = Vec::new();
        BrotliDecompress(&mut &res_bytes[..], &mut out)
            .ok()
            .map(|_| out)
    } else {
        None
    };

    if let Some(out) = decoded {
        remove_header(res_headers, "content-encoding");
        remove_header(res_headers, "content-length");
        let body = String::from_utf8_lossy(&out).to_string();
        return (body, Bytes::from(out));
    }

    (
        String::from_utf8_lossy(res_bytes).to_string(),
        res_bytes.clone(),
    )
}

pub struct ProxyEngine {
    /// (http_client, streaming_client) — pair wrapped for upstream proxy hot-reload.
    clients: tokio::sync::RwLock<(Client, Client)>,
    pub middleware_chain: Arc<RwLock<MiddlewareChain>>,
    pub ca: Option<Arc<crate::certs::CertificateAuthority>>,
    pub mitm_enabled: bool,
    max_body_bytes: Arc<AtomicUsize>,
    /// Retained so hot-reload can rebuild clients with same base settings.
    timeout_secs: u64,
    pool_max_idle_per_host: usize,
    pool_idle_timeout_secs: u64,
}

impl ProxyEngine {
    fn build_clients(
        timeout_secs: u64,
        pool_max_idle: usize,
        pool_idle: std::time::Duration,
        upstream_proxy: Option<&str>,
    ) -> (Client, Client) {
        let mut http = Client::builder()
            .pool_max_idle_per_host(pool_max_idle)
            .pool_idle_timeout(pool_idle)
            .timeout(std::time::Duration::from_secs(timeout_secs));
        let mut streaming = Client::builder()
            .pool_max_idle_per_host(pool_max_idle)
            .pool_idle_timeout(pool_idle);
        if let Some(url) = upstream_proxy {
            if let Ok(p) = reqwest::Proxy::all(url) {
                http = http.proxy(p);
            }
            if let Ok(p) = reqwest::Proxy::all(url) {
                streaming = streaming.proxy(p);
            }
        }
        (
            http.build().unwrap_or_else(|_| Client::new()),
            streaming.build().unwrap_or_else(|_| Client::new()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        middleware_chain: Arc<RwLock<MiddlewareChain>>,
        ca: Option<Arc<crate::certs::CertificateAuthority>>,
        mitm_enabled: bool,
        timeout_secs: u64,
        max_body_bytes: usize,
        pool_max_idle_per_host: usize,
        pool_idle_timeout_secs: u64,
        upstream_proxy: Option<&str>,
    ) -> Self {
        let pool_idle = std::time::Duration::from_secs(pool_idle_timeout_secs);
        let clients = Self::build_clients(
            timeout_secs,
            pool_max_idle_per_host,
            pool_idle,
            upstream_proxy,
        );
        Self {
            clients: tokio::sync::RwLock::new(clients),
            middleware_chain,
            ca,
            mitm_enabled,
            max_body_bytes: Arc::new(AtomicUsize::new(max_body_bytes)),
            timeout_secs,
            pool_max_idle_per_host,
            pool_idle_timeout_secs,
        }
    }

    /// Returns a clone of the HTTP client (cheap — reqwest::Client is Arc-wrapped internally).
    pub async fn http_client(&self) -> Client {
        self.clients.read().await.0.clone()
    }

    /// Rebuilds both clients with a new upstream proxy URL. Pass None to disable proxy.
    pub async fn set_upstream_proxy(&self, proxy_url: Option<&str>) {
        let pool_idle = std::time::Duration::from_secs(self.pool_idle_timeout_secs);
        let new_clients = Self::build_clients(
            self.timeout_secs,
            self.pool_max_idle_per_host,
            pool_idle,
            proxy_url,
        );
        *self.clients.write().await = new_clients;
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
    pub async fn handle_request(self: Arc<Self>, req: Request<Body>) -> Response {
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
            return (
                StatusCode::BAD_GATEWAY,
                "CONNECT should be handled upstream",
            )
                .into_response();
        }

        let req_method = method.to_string();
        let req_uri = uri.to_string();
        let mut req_headers = std::collections::HashMap::new();
        for (name, value) in req.headers().iter() {
            req_headers.insert(name.to_string(), value.to_str().unwrap_or("").to_string());
        }

        let display_uri = display_request_uri(&uri, &req_headers, &host);

        let req_body_bytes = axum::body::to_bytes(req.into_body(), self.max_body_bytes())
            .await
            .unwrap_or_default();
        let req_body = String::from_utf8_lossy(&req_body_bytes).to_string();

        let mut req_ctx = RequestContext {
            method: req_method.clone(),
            uri: display_uri.clone(),
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
                    // Check if a mock response was embedded by MockMiddleware.
                    if let Some(mock_json) = req_ctx.headers.get("x-oproxy-mock-response")
                        && let Ok(v) = serde_json::from_str::<serde_json::Value>(mock_json)
                    {
                        let status = v["status"].as_u64().unwrap_or(200) as u16;
                        let mut headers = std::collections::HashMap::new();
                        if let Some(header_obj) = v["headers"].as_object() {
                            for (k, val) in header_obj {
                                if let Some(vs) = val.as_str() {
                                    headers.insert(k.to_string(), vs.to_string());
                                }
                            }
                        }
                        let body_text = v["body"].as_str().unwrap_or("");
                        let decoded_body = v["body_base64"].as_str().and_then(|encoded| {
                            base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .ok()
                        });
                        let raw_body =
                            decoded_body.unwrap_or_else(|| body_text.as_bytes().to_vec());
                        let content_type =
                            header_value(&headers, "content-type").unwrap_or_default();
                        let body = if is_binary_content_type(&content_type) {
                            base64::engine::general_purpose::STANDARD.encode(&raw_body)
                        } else {
                            String::from_utf8_lossy(&raw_body).to_string()
                        };
                        let mut res_ctx = ResponseContext {
                            status,
                            headers,
                            body,
                            request_uri: display_uri.clone(),
                            session_id: req_ctx.headers.get("x-oproxy-session-id").cloned(),
                            ttfb_ms: 0,
                            body_ms: 0,
                            body_bytes: Some(Bytes::from(raw_body)),
                        };
                        {
                            let chain = self.middleware_chain.read().await;
                            let action = chain.execute_response(&mut res_ctx).await;
                            if action != MiddlewareAction::Continue {
                                return (StatusCode::FORBIDDEN, "Response stopped by middleware")
                                    .into_response();
                            }
                        }
                        let sc = StatusCode::from_u16(res_ctx.status).unwrap_or(StatusCode::OK);
                        let mut builder = Response::builder().status(sc);
                        for (k, v) in &res_ctx.headers {
                            builder = builder.header(k, v);
                        }
                        let body = match res_ctx.body_bytes {
                            Some(bytes) => Body::from(bytes),
                            None => Body::from(res_ctx.body),
                        };
                        return builder.body(body).unwrap_or_else(|_| {
                            (StatusCode::INTERNAL_SERVER_ERROR, "mock error").into_response()
                        });
                    }
                    info!("Request stopped by middleware");
                    return (StatusCode::FORBIDDEN, "Request stopped by middleware")
                        .into_response();
                }
                MiddlewareAction::Pause => {
                    debug!("Request paused by breakpoint");
                    return (StatusCode::ACCEPTED, "Request paused by breakpoint").into_response();
                }
            }
        }

        // Strip internal proxy headers so they are never forwarded to the upstream target.
        // Read the session ID before removing it so we can pass it to ResponseContext for
        // exact session correlation in InspectionMiddleware::on_response.
        let destination = req_ctx.headers.remove("x-oproxy-destination");
        let oproxy_session_id = req_ctx.headers.remove("x-oproxy-session-id");
        // Strip inspector side-channel headers — set by inspector middlewares and read by
        // InspectionMiddleware; must never be forwarded to the upstream target.
        for hdr in &[
            "x-oproxy-jwt",
            "x-oproxy-graphql",
            "x-oproxy-grpc",
            "x-oproxy-mock-response",
            "x-oproxy-skip-recording",
            "x-oproxy-map-local-file",
        ] {
            req_ctx.headers.remove(*hdr);
        }
        // Remove Accept-Encoding so upstream always returns an uncompressed body that we
        // can store as readable UTF-8.  If the upstream ignores this and still sends a
        // compressed response we decompress it below before forwarding.
        req_ctx.headers.remove("accept-encoding");
        // Strip hop-by-hop headers — illegal in HTTP/2 and must not be forwarded.
        for hdr in &[
            "connection",
            "keep-alive",
            "proxy-connection",
            "transfer-encoding",
            "te",
            "trailer",
            "trailers",
            "upgrade",
        ] {
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
            // Reverse-proxy / origin-form request: keep the original request
            // target, not the display URI stored for capture.
            uri.path_and_query()
                .map(|pq| pq.as_str().to_string())
                .unwrap_or_else(|| {
                    let raw = req_uri.clone();
                    if raw.starts_with('/') {
                        raw
                    } else {
                        "/".to_string()
                    }
                })
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
                if let Ok(url) = reqwest::Url::parse(&base)
                    && let Some(dest_host) = url.host_str()
                {
                    let host_val = match url.port() {
                        Some(p) => format!("{}:{}", dest_host, p),
                        None => dest_host.to_string(),
                    };
                    req_ctx.headers.insert("host".to_string(), host_val);
                }
                format!("{}{}", base, path_and_query)
            }
            None => format!("http://{}{}", req_ctx.host, path_and_query),
        };
        debug!(url = %target_url, "Forwarding request");

        let mut target_headers = reqwest::header::HeaderMap::new();
        for (name, value) in &req_ctx.headers {
            if name != "host"
                && let Ok(n) = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                && let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
            {
                target_headers.insert(n, v);
            }
        }

        // If body_bytes is still Some, no middleware cleared it → forward original bytes intact.
        // If a middleware modified body and cleared body_bytes, forward the modified string.
        let forward_req_body = match req_ctx.body_bytes {
            Some(b) => reqwest::Body::from(b),
            None => reqwest::Body::from(req_ctx.body),
        };

        // SSE and other event-stream requests need no timeout — they stream indefinitely.
        let is_sse = req_ctx
            .headers
            .get("accept")
            .map(|v| v.contains("text/event-stream"))
            .unwrap_or(false);
        // Clone the client before any await — reqwest::Client is Arc-backed, clone is free.
        let client = {
            let pool = self.clients.read().await;
            if is_sse {
                pool.1.clone()
            } else {
                pool.0.clone()
            }
        };

        let net_start = Instant::now();
        let response = client
            .request(
                reqwest::Method::from_bytes(req_method.as_bytes()).unwrap(),
                &target_url,
            )
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

                let content_type = header_value(&res_headers, "content-type").unwrap_or_default();
                // Strip hop-by-hop response headers before sending back to client.
                for hdr in &[
                    "connection",
                    "keep-alive",
                    "proxy-connection",
                    "transfer-encoding",
                    "te",
                    "trailer",
                    "trailers",
                    "upgrade",
                ] {
                    remove_header(&mut res_headers, hdr);
                }

                // Streaming path: text/event-stream (SSE) or large response above threshold.
                // Check Content-Length if present; stream when body is too large to buffer.
                let content_length = res.content_length().unwrap_or(0);
                let force_stream = content_length > STREAM_THRESHOLD_BYTES;
                if content_type.contains("text/event-stream") || force_stream {
                    let mut res_ctx = ResponseContext {
                        status,
                        headers: res_headers.clone(),
                        body: String::new(),
                        request_uri: display_uri.clone(),
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
                        while let Ok(Some(chunk)) = r.chunk().await {
                            yield Ok::<_, reqwest::Error>(chunk);
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
                let (res_body, res_body_bytes_canonical) =
                    decoded_response_body(&mut res_headers, &res_bytes);

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
                    request_uri: display_uri.clone(),
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
                            return (StatusCode::FORBIDDEN, "Response stopped by middleware")
                                .into_response();
                        }
                        MiddlewareAction::Pause => {
                            debug!("Response paused by breakpoint");
                            return (StatusCode::ACCEPTED, "Response paused by breakpoint")
                                .into_response();
                        }
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
                    request_uri: display_uri.clone(),
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
        || ct.starts_with("font/")
        || ct == "application/octet-stream"
        || ct == "application/pdf"
        || ct == "application/wasm"
        || ct == "application/zip"
        || ct == "application/gzip"
        || ct == "application/x-tar"
        || ct == "application/x-gzip"
        || ct == "application/msgpack"
        || ct == "application/x-msgpack"
        || ct == "application/cbor"
        || ct == "application/protobuf"
        || ct == "application/x-protobuf"
        || ct == "application/vnd.google.protobuf"
}

#[cfg(test)]
mod tests {
    use super::{decoded_response_body, display_request_uri};
    use axum::http::Uri;
    use bytes::Bytes;
    use flate2::{Compression, write::ZlibEncoder};
    use std::collections::HashMap;
    use std::io::Write as _;

    #[test]
    fn display_request_uri_uses_mitm_destination_for_origin_form_requests() {
        let uri: Uri = "/login?next=1".parse().unwrap();
        let mut headers = HashMap::new();
        headers.insert(
            "x-oproxy-destination".to_string(),
            "https://example.com".to_string(),
        );

        assert_eq!(
            display_request_uri(&uri, &headers, "example.com"),
            "https://example.com/login?next=1"
        );
    }

    #[test]
    fn display_request_uri_preserves_absolute_forward_proxy_uri() {
        let uri: Uri = "http://api.example.test/v1?q=1".parse().unwrap();
        let headers = HashMap::new();

        assert_eq!(
            display_request_uri(&uri, &headers, "api.example.test"),
            "http://api.example.test/v1?q=1"
        );
    }

    #[test]
    fn display_request_uri_keeps_root_path_for_mitm_requests() {
        let uri: Uri = "/".parse().unwrap();
        let mut headers = HashMap::new();
        headers.insert(
            "x-oproxy-destination".to_string(),
            "https://example.com".to_string(),
        );

        assert_eq!(
            display_request_uri(&uri, &headers, "example.com"),
            "https://example.com/"
        );
    }

    #[test]
    fn decoded_response_body_decodes_zlib_wrapped_deflate() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"hello-deflate-body").unwrap();
        let compressed = encoder.finish().unwrap();
        let mut headers = HashMap::new();
        headers.insert("Content-Encoding".to_string(), "deflate".to_string());
        headers.insert("Content-Length".to_string(), compressed.len().to_string());

        let (body, bytes) = decoded_response_body(&mut headers, &Bytes::from(compressed));

        assert_eq!(body, "hello-deflate-body");
        assert_eq!(&bytes[..], b"hello-deflate-body");
        assert!(!headers.contains_key("Content-Encoding"));
        assert!(!headers.contains_key("Content-Length"));
    }
}
