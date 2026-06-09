use crate::middleware::chain::MiddlewareChain;
use crate::middleware::{
    MiddlewareAction, RequestContext, ResponseContext, header_value, remove_header,
};
use bytes::Bytes;
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

use crate::core::decompression::decoded_response_body;

fn display_request_uri(
    uri: &axum::http::Uri,
    mitm_destination: Option<&str>,
    host: &str,
) -> String {
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

    if let Some(destination) = mitm_destination {
        let base = destination.trim_end_matches('/');
        return format!("{}{}", base, path_and_query);
    }

    if uri.scheme().is_some() && uri.authority().is_some() {
        return uri.to_string();
    }

    format!("http://{}{}", host, path_and_query)
}

pub struct ProxyEngine {
    /// (http_client, streaming_client) — pair wrapped for upstream proxy hot-reload.
    clients: tokio::sync::RwLock<(Client, Client)>,
    pub middleware_chain: Arc<RwLock<MiddlewareChain>>,
    pub ca: Option<Arc<crate::certs::CertificateAuthority>>,
    pub mitm_enabled: bool,
    short_circuit_session_manager: Arc<RwLock<Option<crate::session::SharedSessionManager>>>,
    max_body_bytes: Arc<AtomicUsize>,
    /// Retained so hot-reload can rebuild clients with same base settings.
    timeout_secs: u64,
    pool_max_idle_per_host: usize,
    pool_idle_timeout_secs: u64,
    pub(crate) bind_port: u16,
    pub(crate) bind_host: String,
    /// LAN/host IPs of this proxy, resolved once at construction. Used for
    /// self-proxy loop detection when bound to a wildcard address. Cached to
    /// avoid a per-request UDP socket syscall in `detect_lan_ip`.
    self_lan_hosts: Vec<String>,
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
        let http = http.build().unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to build HTTP client with configured settings; \
                falling back to default client — timeout, pool, and upstream-proxy settings are NOT applied");
            Client::new()
        });
        let streaming = streaming.build().unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to build streaming HTTP client with configured settings; \
                falling back to default client — timeout, pool, and upstream-proxy settings are NOT applied");
            Client::new()
        });
        (http, streaming)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        middleware_chain: Arc<RwLock<MiddlewareChain>>,
        ca: Option<Arc<crate::certs::CertificateAuthority>>,
        mitm_enabled: bool,
        bind_port: u16,
        bind_host: String,
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
            short_circuit_session_manager: Arc::new(RwLock::new(None)),
            max_body_bytes: Arc::new(AtomicUsize::new(max_body_bytes)),
            timeout_secs,
            pool_max_idle_per_host,
            pool_idle_timeout_secs,
            bind_port,
            bind_host,
            self_lan_hosts: [
                crate::setup::public_lan_ip_for_setup(),
                crate::setup::detect_lan_ip(),
            ]
            .into_iter()
            .flatten()
            .map(|h| h.to_ascii_lowercase())
            .collect(),
        }
    }

    pub async fn set_short_circuit_session_manager(
        &self,
        session_manager: crate::session::SharedSessionManager,
    ) {
        *self.short_circuit_session_manager.write().await = Some(session_manager);
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

    fn is_self_proxy(&self, target_url: &str) -> bool {
        let Ok(url) = reqwest::Url::parse(target_url) else {
            return false;
        };
        let Some(host) = url.host_str() else {
            return false;
        };
        let host_clean = host.to_ascii_lowercase();

        let target_port = url
            .port()
            .unwrap_or_else(|| if url.scheme() == "https" { 443 } else { 80 });

        if target_port != self.bind_port {
            return false;
        }

        // 1. Loopback and wildcard hosts
        if matches!(
            host_clean.as_str(),
            "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "[::]"
        ) {
            return true;
        }

        // 2. Direct match with configured bind_host
        let bind_host_clean = self
            .bind_host
            .split(':')
            .next()
            .unwrap_or(&self.bind_host)
            .trim()
            .to_ascii_lowercase();
        if host_clean == bind_host_clean {
            return true;
        }
        // 3. If bound to wildcard, check matching LAN hosts
        if matches!(bind_host_clean.as_str(), "0.0.0.0" | "::" | "[::]")
            && self.self_lan_hosts.contains(&host_clean)
        {
            return true;
        }

        false
    }

    async fn record_short_circuit_response(&self, res_ctx: &ResponseContext) {
        let Some(session_id) = res_ctx.session_id.clone() else {
            return;
        };
        let Some(session_manager) = self.short_circuit_session_manager.read().await.clone() else {
            return;
        };
        let Some(session) = session_manager.get_session(&session_id) else {
            session_manager.record_response(session_id, res_ctx.clone());
            return;
        };
        if session.response.is_some() {
            return;
        }

        let latency_ms = (chrono::Utc::now() - session.timestamp).num_milliseconds() as u64;
        let response_size_bytes = if res_ctx.body.is_empty() {
            header_value(&res_ctx.headers, "content-length")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0)
        } else {
            res_ctx.body.len()
        };
        let metrics = crate::session::InspectionMetrics {
            latency_ms,
            request_size_bytes: session.request.body.len(),
            response_size_bytes,
            status_code: res_ctx.status,
            ttfb_ms: res_ctx.ttfb_ms,
            body_ms: res_ctx.body_ms,
            ..Default::default()
        };
        let tags = res_ctx.tags.clone();
        session_manager.record_response_with_metrics(session_id.clone(), res_ctx.clone(), metrics);
        if !tags.is_empty() {
            let mut merged = session.tags.clone();
            for tag in tags {
                if !merged.iter().any(|existing| existing == &tag) {
                    merged.push(tag);
                }
            }
            session_manager
                .annotate(&session_id, None, Some(merged))
                .await;
        }
    }

    pub async fn handle_request(self: Arc<Self>, req: Request<Body>) -> Response {
        self.handle_request_with_destination(req, None).await
    }

    /// Like [`handle_request`] but with an explicit upstream destination supplied by
    /// the MITM TLS layer. Passing it as a typed argument (rather than the former
    /// `x-oproxy-destination` request header) keeps it off the wire and prevents a
    /// client from spoofing the proxy target.
    #[instrument(skip(self, req, mitm_destination))]
    pub async fn handle_request_with_destination(
        self: Arc<Self>,
        req: Request<Body>,
        mitm_destination: Option<String>,
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
            return (
                StatusCode::BAD_GATEWAY,
                "CONNECT should be handled upstream",
            )
                .into_response();
        }

        let req_method = method.to_string();
        let req_uri = uri.to_string();
        let mut req_headers = crate::middleware::HeaderMap::new();
        for (name, value) in req.headers().iter() {
            req_headers.append(name.to_string(), value.to_str().unwrap_or("").to_string());
        }

        let display_uri = display_request_uri(&uri, mitm_destination.as_deref(), &host);

        let req_body_bytes =
            match axum::body::to_bytes(req.into_body(), self.max_body_bytes()).await {
                Ok(b) => b,
                Err(_) => {
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!(
                            "Request body exceeds the {} byte limit. \
                             Raise max_body_bytes in settings to increase it.",
                            self.max_body_bytes()
                        ),
                    )
                        .into_response();
                }
            };

        let mut req_ctx = RequestContext {
            method: req_method.clone(),
            uri: display_uri.clone(),
            headers: req_headers,
            body: req_body_bytes,
            host: host.clone(),
            // MITM supplies the upstream target up front; routing/dns middleware may
            // still override it during the chain.
            destination: mitm_destination,
            ..Default::default()
        };

        // Execute Request Middleware Chain
        {
            debug!("Executing request middleware chain");
            let chain = self.middleware_chain.read().await.clone();
            let action = chain.execute_request(&mut req_ctx).await;
            match action {
                MiddlewareAction::Continue => {}
                MiddlewareAction::StopAndReturn => {
                    // A middleware (Mock / map-local / Lua abort / breakpoint timeout) may
                    // have set a typed short-circuit response to return instead of forwarding.
                    if let Some(intercepted) = req_ctx.mock_response.take() {
                        let crate::middleware::InterceptedResponse {
                            status,
                            headers,
                            body: raw_body,
                            tags,
                        } = intercepted;
                        let mut res_ctx = ResponseContext {
                            status,
                            headers,
                            body: raw_body,
                            request_uri: display_uri.clone(),
                            session_id: req_ctx.session_id.clone(),
                            tags,
                            request_host: host.clone(),
                            request_method: req_method.clone(),
                            ..Default::default()
                        };
                        {
                            let chain = self.middleware_chain.read().await.clone();
                            let action = chain.execute_response(&mut res_ctx).await;
                            if action != MiddlewareAction::Continue {
                                return (StatusCode::FORBIDDEN, "Response stopped by middleware")
                                    .into_response();
                            }
                        }
                        self.record_short_circuit_response(&res_ctx).await;
                        let sc = StatusCode::from_u16(res_ctx.status).unwrap_or(StatusCode::OK);
                        let mut builder = Response::builder().status(sc);
                        for (k, v) in &res_ctx.headers {
                            builder = builder.header(k, v);
                        }
                        return builder.body(Body::from(res_ctx.body)).unwrap_or_else(|_| {
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

        // Upstream target + session id now travel as typed context fields, not headers.
        let destination = req_ctx.destination.take();
        let oproxy_session_id = req_ctx.session_id.clone();
        // Defensively drop any client-supplied `x-oproxy-*` headers so a malicious client
        // can never smuggle one through to the upstream target.
        req_ctx
            .headers
            .retain(|name, _| !name.trim().to_ascii_lowercase().starts_with("x-oproxy-"));
        // Instead of completely removing Accept-Encoding (which triggers WAF bot protection
        // on strict CDNs by creating a User-Agent / Accept-Encoding mismatch), we preserve it
        // but strip `zstd` since we don't have a manual zstd decoder. We rely on our manual
        // decompression block below for gzip/deflate/br.
        if let Some(mut ae) = req_ctx.headers.remove("accept-encoding") {
            ae = ae
                .replace(", zstd", "")
                .replace("zstd, ", "")
                .replace("zstd", "");
            if !ae.trim().is_empty() {
                req_ctx.headers.insert("accept-encoding".to_string(), ae);
            }
        }
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
            req_ctx.headers.remove(hdr);
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

        if self.is_self_proxy(&target_url) {
            tracing::warn!(url = %target_url, "Proxy loop detected: request forwarded to the proxy itself");
            let mut err_ctx = crate::middleware::ResponseContext {
                status: 502,
                headers: crate::middleware::HeaderMap::new(),
                body: bytes::Bytes::from(
                    "Proxy error: Proxy loop detected (request forwarded to the proxy itself)",
                ),
                request_uri: display_uri.clone(),
                session_id: oproxy_session_id,
                ttfb_ms: 0,
                request_host: host.clone(),
                request_method: req_method.clone(),
                ..Default::default()
            };
            {
                let chain = self.middleware_chain.read().await.clone();
                chain.execute_response(&mut err_ctx).await;
            }
            return (
                StatusCode::BAD_GATEWAY,
                "Proxy error: Proxy loop detected (request forwarded to the proxy itself)",
            )
                .into_response();
        }

        let mut target_headers = reqwest::header::HeaderMap::new();
        for (name, value) in &req_ctx.headers {
            // reqwest computes Content-Length automatically. Sending a mismatched length
            // (e.g. after a middleware modified the body) causes HTTP protocol errors.
            if name != "host"
                && name != "content-length"
                && let Ok(n) = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                && let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
            {
                // append (not insert) so multi-valued request headers survive.
                target_headers.append(n, v);
            }
        }

        // Use the no-timeout streaming client for all proxied requests.
        // Proxied traffic can be arbitrarily long (SSE, large downloads); the timeout
        // client (pool.0) is reserved for control-plane outbound calls via http_client().
        let client = self.clients.read().await.1.clone();

        let forward_method = match reqwest::Method::from_bytes(req_method.as_bytes()) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(method = %req_method, error = %e, "rejecting request with invalid HTTP method");
                return (StatusCode::BAD_REQUEST, "invalid HTTP method").into_response();
            }
        };
        let mut request_builder = client
            .request(forward_method, &target_url)
            .headers(target_headers);

        // Avoid attaching an empty body to methods like GET if the original request didn't specify one.
        // reqwest automatically adds `Content-Length: 0` if we call `.body()`, which strict servers reject.
        if !req_ctx.body.is_empty() || req_ctx.headers.contains_key("content-length") {
            request_builder = request_builder.body(reqwest::Body::from(req_ctx.body));
        }

        let net_start = Instant::now();
        let response = request_builder.send().await;

        match response {
            Ok(res) => {
                let ttfb_ms = net_start.elapsed().as_millis() as u64;
                let status = res.status().as_u16();
                let mut res_headers = crate::middleware::HeaderMap::new();
                for (name, value) in res.headers().iter() {
                    // append (not insert) so duplicate upstream headers such as
                    // multiple Set-Cookie survive instead of collapsing to the last one.
                    res_headers.append(name.to_string(), value.to_str().unwrap_or("").to_string());
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
                // Chunked transfer-encoded responses have no Content-Length, so
                // content_length defaults to 0 and would bypass the threshold,
                // buffering an arbitrarily large body. Stream them unconditionally.
                let is_chunked = res
                    .headers()
                    .get("transfer-encoding")
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.to_ascii_lowercase().contains("chunked"))
                    .unwrap_or(false);
                let force_stream = content_length > STREAM_THRESHOLD_BYTES || is_chunked;
                if content_type.contains("text/event-stream") || force_stream {
                    let mut res_ctx = ResponseContext {
                        status,
                        headers: res_headers.clone(),
                        request_uri: display_uri.clone(),
                        session_id: oproxy_session_id,
                        ttfb_ms,
                        request_host: host.clone(),
                        request_method: req_method.clone(),
                        ..Default::default()
                    };
                    {
                        let chain = self.middleware_chain.read().await.clone();
                        let action = chain.execute_response(&mut res_ctx).await;
                        if action != MiddlewareAction::Continue {
                            return (StatusCode::FORBIDDEN, "Response stopped by middleware")
                                .into_response();
                        }
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
                // The canonical decoded bytes are the single source of truth; binary vs. text
                // presentation (base64 for the UI) is handled at serialisation time.
                let res_body = decoded_response_body(&mut res_headers, &res_bytes);

                let mut res_ctx = ResponseContext {
                    status,
                    headers: res_headers,
                    body: res_body,
                    request_uri: display_uri.clone(),
                    session_id: oproxy_session_id,
                    ttfb_ms,
                    body_ms,
                    request_host: host.clone(),
                    request_method: req_method.clone(),
                    ..Default::default()
                };

                // Execute Response Middleware Chain
                {
                    debug!("Executing response middleware chain");
                    let chain = self.middleware_chain.read().await.clone();
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

                builder
                    .body(Body::from(res_ctx.body))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            }
            Err(e) => {
                error!(error = %e, "Proxy error");
                // Run on_response with a synthetic 502 so InspectionMiddleware records
                // the failed exchange instead of leaving it as a dangling "pending" session.
                let mut err_ctx = crate::middleware::ResponseContext {
                    status: 502,
                    headers: crate::middleware::HeaderMap::new(),
                    body: Bytes::from(format!("Proxy error: {}", e)),
                    request_uri: display_uri.clone(),
                    session_id: oproxy_session_id,
                    ttfb_ms: net_start.elapsed().as_millis() as u64,
                    request_host: host.clone(),
                    request_method: req_method.clone(),
                    ..Default::default()
                };
                {
                    let chain = self.middleware_chain.read().await.clone();
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
    use std::io::Write as _;

    #[test]
    fn display_request_uri_uses_mitm_destination_for_origin_form_requests() {
        let uri: Uri = "/login?next=1".parse().unwrap();

        assert_eq!(
            display_request_uri(&uri, Some("https://example.com"), "example.com"),
            "https://example.com/login?next=1"
        );
    }

    #[test]
    fn display_request_uri_preserves_absolute_forward_proxy_uri() {
        let uri: Uri = "http://api.example.test/v1?q=1".parse().unwrap();

        assert_eq!(
            display_request_uri(&uri, None, "api.example.test"),
            "http://api.example.test/v1?q=1"
        );
    }

    #[test]
    fn display_request_uri_keeps_root_path_for_mitm_requests() {
        let uri: Uri = "/".parse().unwrap();

        assert_eq!(
            display_request_uri(&uri, Some("https://example.com"), "example.com"),
            "https://example.com/"
        );
    }

    #[test]
    fn decoded_response_body_decodes_zlib_wrapped_deflate() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"hello-deflate-body").unwrap();
        let compressed = encoder.finish().unwrap();
        let mut headers = crate::middleware::HeaderMap::new();
        headers.insert("Content-Encoding".to_string(), "deflate".to_string());
        headers.insert("Content-Length".to_string(), compressed.len().to_string());

        let bytes = decoded_response_body(&mut headers, &Bytes::from(compressed));

        assert_eq!(&bytes[..], b"hello-deflate-body");
        assert!(!headers.contains_key("Content-Encoding"));
        assert!(!headers.contains_key("Content-Length"));
    }
}
