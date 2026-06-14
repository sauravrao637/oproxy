use crate::core::forward::{ProtocolContext, WireProtocol};
use crate::middleware::chain::MiddlewareChain;
use crate::middleware::{
    MiddlewareAction, RequestContext, ResponseContext, header_value, remove_header,
};
use bytes::Bytes;
use futures_util::StreamExt;
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
use tracing::{debug, info, instrument};

use crate::core::decompression::decoded_response_body;
use http_body_util::{BodyExt as _, Full};
use std::future::ready;

/// Downstream connection/stream identity captured at request entry,
/// bundled so it can be threaded through the streaming path in one argument.
struct StreamIdentity {
    connection_id: Option<String>,
    stream_id: Option<u64>,
    downstream_protocol: Option<String>,
    remote_addr: Option<String>,
}

struct StreamRequest {
    method: String,
    uri: axum::http::Uri,
    display_uri: String,
    headers: crate::middleware::HeaderMap,
    host: String,
    destination: Option<String>,
    started_at: Instant,
    identity: StreamIdentity,
    protocol_context: ProtocolContext,
}

struct RequestMetadata<'a> {
    uri: &'a str,
    host: &'a str,
    method: &'a str,
}

struct PreparedUpstream {
    url: String,
    headers: reqwest::header::HeaderMap,
    method: reqwest::Method,
    session_id: Option<String>,
    protocol_context: Option<ProtocolContext>,
}

/// Derived, owned view of an incoming request's head: method, target, headers,
/// downstream connection identity, and protocol. Built once by
/// [`ProxyEngine::build_request_head`] before the body is touched, then consumed
/// by either the streaming or the buffered forward path.
struct RequestHead {
    method: axum::http::Method,
    uri: axum::http::Uri,
    host: String,
    remote_addr: Option<String>,
    connection_id: Option<String>,
    stream_id: Option<u64>,
    downstream_protocol: Option<String>,
    req_method: String,
    req_uri: String,
    req_headers: crate::middleware::HeaderMap,
    display_uri: String,
    protocol_context: ProtocolContext,
}

mod wire;
use wire::{
    display_request_uri, infer_application_protocol, infer_body_mode, request_scheme,
    sanitize_forwarded_request_headers, strip_hop_by_hop_response_headers, target_url,
    upstream_headers, upstream_path,
};
pub use wire::{is_binary_content_type, protocol_label};

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
    /// self-proxy loop detection when bound to a wildcard address.
    self_lan_hosts: Vec<String>,
    /// If set, injected as `alt-svc` on every forwarded response to advertise
    /// the HTTP/3 listener. Built once from `Config.http3_port` at startup.
    /// Set once during startup via [`ProxyEngine::set_alt_svc_header`]; a
    /// `OnceLock` so it can be set through the `Arc` without the fragile
    /// `Arc::get_mut` pattern (which silently no-ops if any clone exists).
    alt_svc_header: std::sync::OnceLock<String>,
}

/// Construction parameters for [`ProxyEngine::new`]. Grouping these into a
/// struct keeps call sites self-documenting (named fields rather than a long
/// positional argument list).
///
/// # Examples
///
/// ```no_run
/// use std::sync::Arc;
/// use tokio::sync::RwLock;
///
/// use oproxy::core::engine::{ProxyEngine, ProxyEngineConfig};
/// use oproxy::middleware::chain::MiddlewareChain;
///
/// let engine = ProxyEngine::new(ProxyEngineConfig {
///     middleware_chain: Arc::new(RwLock::new(MiddlewareChain::new())),
///     ca: None,
///     mitm_enabled: false,
///     bind_port: 8080,
///     bind_host: "127.0.0.1".to_string(),
///     timeout_secs: 30,
///     max_body_bytes: 10 * 1024 * 1024,
///     pool_max_idle_per_host: 10,
///     pool_idle_timeout_secs: 30,
///     upstream_proxy: None,
/// });
/// assert_eq!(engine.max_body_bytes(), 10 * 1024 * 1024);
/// ```
pub struct ProxyEngineConfig {
    pub middleware_chain: Arc<RwLock<MiddlewareChain>>,
    pub ca: Option<Arc<crate::certs::CertificateAuthority>>,
    pub mitm_enabled: bool,
    pub bind_port: u16,
    pub bind_host: String,
    pub timeout_secs: u64,
    pub max_body_bytes: usize,
    pub pool_max_idle_per_host: usize,
    pub pool_idle_timeout_secs: u64,
    pub upstream_proxy: Option<String>,
}

#[cfg(test)]
impl Default for ProxyEngineConfig {
    /// Lightweight defaults for tests: an empty middleware chain, MITM off, and
    /// a localhost bind. Production constructs every field explicitly.
    fn default() -> Self {
        Self {
            middleware_chain: Arc::new(RwLock::new(MiddlewareChain::new())),
            ca: None,
            mitm_enabled: false,
            bind_port: 8080,
            bind_host: "127.0.0.1".to_string(),
            timeout_secs: 30,
            max_body_bytes: 10 * 1024 * 1024,
            pool_max_idle_per_host: 10,
            pool_idle_timeout_secs: 30,
            upstream_proxy: None,
        }
    }
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
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .no_gzip()
            .no_deflate()
            .no_brotli()
            .no_zstd();
        let mut streaming = Client::builder()
            .pool_max_idle_per_host(pool_max_idle)
            .pool_idle_timeout(pool_idle)
            .no_gzip()
            .no_deflate()
            .no_brotli()
            .no_zstd()
            .redirect(reqwest::redirect::Policy::none());
        // Optionally skip upstream TLS verification (e.g. forwarding to origins
        // with self-signed certs while debugging, like mitmproxy's --ssl-insecure).
        // Off by default; enabled via OPROXY_INSECURE_UPSTREAM=1. Read here (rather
        // than threaded through the constructor) so every existing call site and
        // the hot-reload path pick it up without signature churn.
        let insecure_upstream = std::env::var("OPROXY_INSECURE_UPSTREAM")
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        if insecure_upstream {
            tracing::warn!(
                "OPROXY_INSECURE_UPSTREAM is set: upstream TLS certificate verification is DISABLED"
            );
            http = http.danger_accept_invalid_certs(true);
            streaming = streaming.danger_accept_invalid_certs(true);
        }
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

    pub fn new(config: ProxyEngineConfig) -> Self {
        let ProxyEngineConfig {
            middleware_chain,
            ca,
            mitm_enabled,
            bind_port,
            bind_host,
            timeout_secs,
            max_body_bytes,
            pool_max_idle_per_host,
            pool_idle_timeout_secs,
            upstream_proxy,
        } = config;
        let pool_idle = std::time::Duration::from_secs(pool_idle_timeout_secs);
        let clients = Self::build_clients(
            timeout_secs,
            pool_max_idle_per_host,
            pool_idle,
            upstream_proxy.as_deref(),
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
            self_lan_hosts: crate::setup::public_lan_ip_for_setup()
                .map(|h| vec![h.to_ascii_lowercase()])
                .unwrap_or_default(),
            alt_svc_header: std::sync::OnceLock::new(),
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

    /// Sets the `alt-svc` header value advertised on forwarded responses
    /// (HTTP/3 discovery). Callable through the `Arc` during startup; setting it
    /// more than once logs and keeps the first value.
    pub fn set_alt_svc_header(&self, value: String) {
        if self.alt_svc_header.set(value).is_err() {
            tracing::warn!("alt_svc_header was already set; keeping the first value");
        }
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

    async fn respond_to_intercepted(
        &self,
        intercepted: crate::middleware::InterceptedResponse,
        request_uri: &str,
        session_id: Option<String>,
        request_host: &str,
        request_method: &str,
        protocol_context: Option<ProtocolContext>,
    ) -> Response {
        let crate::middleware::InterceptedResponse {
            status,
            headers,
            body,
            tags,
            served_mock,
        } = intercepted;
        let mut response = ResponseContext {
            status,
            headers,
            body,
            request_uri: request_uri.to_string(),
            session_id,
            tags,
            request_host: request_host.to_string(),
            request_method: request_method.to_string(),
            protocol_context,
            ..Default::default()
        };

        let chain = self.middleware_chain.read().await.clone();
        if chain.execute_response(&mut response).await != MiddlewareAction::Continue {
            return (StatusCode::FORBIDDEN, "Response stopped by middleware").into_response();
        }

        self.record_short_circuit_response(&response).await;
        if let (Some(session_id), Some(served_mock)) = (response.session_id.as_deref(), served_mock)
        {
            self.append_session_event(
                session_id,
                crate::session::SessionEvent::MockServed {
                    rule_id: served_mock.rule_id,
                    behavior: served_mock.behavior,
                },
            )
            .await;
        }

        let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK);
        let mut builder = Response::builder().status(status);
        for (name, value) in &response.headers {
            builder = builder.header(name, value);
        }
        builder
            .body(Body::from(response.body))
            .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "mock error").into_response())
    }

    async fn execute_request_middleware(
        &self,
        request: &mut RequestContext,
        metadata: RequestMetadata<'_>,
    ) -> Result<(), Response> {
        let chain = self.middleware_chain.read().await.clone();
        match chain.execute_request(request).await {
            MiddlewareAction::Continue => Ok(()),
            MiddlewareAction::StopAndReturn => {
                if let Some(intercepted) = request.mock_response.take() {
                    return Err(self
                        .respond_to_intercepted(
                            intercepted,
                            metadata.uri,
                            request.session_id.clone(),
                            metadata.host,
                            metadata.method,
                            request.protocol_context.clone(),
                        )
                        .await);
                }
                info!("Request stopped by middleware");
                Err((StatusCode::FORBIDDEN, "Request stopped by middleware").into_response())
            }
        }
    }

    async fn execute_response_middleware(
        &self,
        response: &mut ResponseContext,
    ) -> Result<(), Response> {
        let chain = self.middleware_chain.read().await.clone();
        match chain.execute_response(response).await {
            MiddlewareAction::Continue => Ok(()),
            MiddlewareAction::StopAndReturn => {
                info!("Response stopped by middleware");
                Err((StatusCode::FORBIDDEN, "Response stopped by middleware").into_response())
            }
        }
    }

    fn response_builder(&self, response: &ResponseContext) -> axum::http::response::Builder {
        let status =
            StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut builder = Response::builder().status(status);
        for (name, value) in &response.headers {
            builder = builder.header(name, value);
        }
        if let Some(alt_svc) = self.alt_svc_header.get() {
            builder = builder.header("alt-svc", alt_svc);
        }
        builder
    }

    fn prepare_upstream(
        &self,
        request: &mut RequestContext,
        uri: &axum::http::Uri,
        fallback_uri: &str,
    ) -> Result<PreparedUpstream, Box<Response>> {
        let destination = request.destination.take();
        sanitize_forwarded_request_headers(&mut request.headers);
        let path = upstream_path(uri, fallback_uri);
        let url = target_url(
            destination.as_deref(),
            &request.host,
            &path,
            &mut request.headers,
        );
        if self.is_self_proxy(&url) {
            tracing::warn!(%url, "Proxy loop detected");
            return Err(Box::new(
                (
                    StatusCode::BAD_GATEWAY,
                    "Proxy error: Proxy loop detected (request forwarded to the proxy itself)",
                )
                    .into_response(),
            ));
        }
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|error| {
            tracing::warn!(method = %request.method, %error, "rejecting invalid HTTP method");
            Box::new((StatusCode::BAD_REQUEST, "invalid HTTP method").into_response())
        })?;
        Ok(PreparedUpstream {
            url,
            headers: upstream_headers(&request.headers),
            method,
            session_id: request.session_id.clone(),
            protocol_context: request.protocol_context.clone(),
        })
    }

    async fn record_forward_failure(
        &self,
        error: &reqwest::Error,
        metadata: RequestMetadata<'_>,
        session_id: Option<String>,
        protocol_context: Option<ProtocolContext>,
        ttfb_ms: u64,
    ) -> Response {
        let mut source_chain = error.to_string();
        let mut source = std::error::Error::source(error);
        while let Some(next) = source {
            source_chain.push_str(&format!(": {next}"));
            source = next.source();
        }
        tracing::error!(error = %source_chain, "Proxy error");
        let mut response = ResponseContext {
            status: 502,
            body: Bytes::from(format!("Proxy error: {error}")),
            request_uri: metadata.uri.to_string(),
            session_id,
            ttfb_ms,
            request_host: metadata.host.to_string(),
            request_method: metadata.method.to_string(),
            protocol_context,
            ..Default::default()
        };
        let chain = self.middleware_chain.read().await.clone();
        chain.execute_response(&mut response).await;
        (StatusCode::BAD_GATEWAY, "Error forwarding request").into_response()
    }

    async fn append_session_event(&self, session_id: &str, event: crate::session::SessionEvent) {
        let Some(session_manager) = self.short_circuit_session_manager.read().await.clone() else {
            return;
        };
        session_manager.append_event(session_id, event);
    }

    pub(crate) async fn record_socks5_tunnel_opened(
        &self,
        host: &str,
        port: u16,
    ) -> Option<String> {
        let session_manager = self.short_circuit_session_manager.read().await.clone()?;
        let id = uuid::Uuid::new_v4().to_string();
        let uri = format!("socks5://{host}:{port}");
        let connection_id = format!("socks5:{id}");
        let request = RequestContext {
            method: "CONNECT".to_string(),
            uri,
            host: host.to_string(),
            protocol_context: Some(
                ProtocolContext::socks5_tunnel()
                    .with_identity(Some(connection_id.clone()), Some(1)),
            ),
            downstream_protocol: Some(WireProtocol::Socks5.label().to_string()),
            connection_id: Some(connection_id),
            stream_id: Some(1),
            ..Default::default()
        };
        session_manager.record_request_with_source(
            id.clone(),
            request,
            crate::session::SessionSource::Proxy,
        );
        session_manager.append_event(&id, crate::session::SessionEvent::TunnelOpened);
        Some(id)
    }

    pub(crate) async fn record_socks5_tunnel_closed(
        &self,
        session_id: &str,
        bytes_up: u64,
        bytes_down: u64,
    ) {
        let Some(session_manager) = self.short_circuit_session_manager.read().await.clone() else {
            return;
        };
        session_manager.flush().await;
        let Some(session) = session_manager.get_session(session_id) else {
            return;
        };
        session_manager.append_event(
            session_id,
            crate::session::SessionEvent::TunnelClosed {
                bytes_up,
                bytes_down,
            },
        );
        let body = Bytes::from(format!("up={bytes_up} down={bytes_down}"));
        let response = ResponseContext {
            status: 200,
            headers: crate::middleware::HeaderMap::new(),
            body: body.clone(),
            request_uri: session.request.uri.clone(),
            session_id: Some(session_id.to_string()),
            request_host: session.request.host.clone(),
            request_method: session.request.method.clone(),
            protocol: Some(WireProtocol::Socks5.label().to_string()),
            protocol_context: session.protocol_context.clone(),
            ..Default::default()
        };
        let metrics = crate::session::InspectionMetrics {
            latency_ms: (chrono::Utc::now() - session.timestamp)
                .num_milliseconds()
                .max(0) as u64,
            request_size_bytes: bytes_up as usize,
            response_size_bytes: bytes_down as usize,
            status_code: 200,
            protocol: Some(WireProtocol::Socks5.label().to_string()),
            ..Default::default()
        };
        session_manager.record_response_with_metrics(session_id.to_string(), response, metrics);
    }

    pub(crate) async fn record_socks5_mock_served(
        &self,
        host: &str,
        port: u16,
        rule_id: String,
        behavior: String,
    ) {
        let Some(session_manager) = self.short_circuit_session_manager.read().await.clone() else {
            return;
        };
        let id = uuid::Uuid::new_v4().to_string();
        let request = RequestContext {
            method: "CONNECT".to_string(),
            uri: format!("socks5://{host}:{port}"),
            host: host.to_string(),
            protocol_context: Some(ProtocolContext::socks5_tunnel()),
            downstream_protocol: Some(WireProtocol::Socks5.label().to_string()),
            ..Default::default()
        };
        session_manager.record_request_with_source(
            id.clone(),
            request,
            crate::session::SessionSource::Proxy,
        );
        session_manager.append_event(
            &id,
            crate::session::SessionEvent::MockServed { rule_id, behavior },
        );
    }

    /// Derives the [`RequestHead`] from the incoming request before its body is
    /// read. Pure with respect to `self` (only reads config-free helpers), so it
    /// is cheap and safe to run for every request, including the CONNECT
    /// safety-net case that returns immediately afterwards.
    fn build_request_head(
        &self,
        req: &Request<Body>,
        mitm_destination: Option<&str>,
    ) -> RequestHead {
        let method = req.method().clone();
        let uri = req.uri().clone();
        // HTTP/2 and HTTP/3 carry the target authority in the `:authority`
        // pseudo-header (exposed via the request URI), not a `Host` header. Fall
        // back to the URI authority so forward-proxied h2/h3 requests resolve a
        // real upstream instead of "unknown" (which 502s).
        let host = req
            .headers()
            .get("host")
            .and_then(|h| h.to_str().ok())
            .map(str::to_string)
            .or_else(|| req.uri().authority().map(|a| a.to_string()))
            .unwrap_or_else(|| "unknown".to_string());

        debug!(method = %method, uri = %uri, host = %host, "Processing request");

        // Capture downstream connection identity exactly once per request —
        // `next_stream()` mutates the per-connection counter. The downstream protocol
        // is the negotiated request version (h2 requests arrive as HTTP/2 regardless
        // of the upstream leg).
        let remote_addr = req
            .extensions()
            .get::<crate::transport::http::DownstreamPeer>()
            .map(|p| p.0.to_string());
        let (connection_id, stream_id, downstream_protocol) = {
            let conn = req
                .extensions()
                .get::<crate::transport::http::DownstreamConn>();
            (
                conn.map(|c| c.id.clone()),
                conn.map(|c| c.next_stream()),
                Some(protocol_label(req.version()).to_string()),
            )
        };

        let req_method = method.to_string();
        let req_uri = uri.to_string();
        let mut req_headers = crate::middleware::HeaderMap::new();
        for (name, value) in req.headers().iter() {
            req_headers.append(name.to_string(), value.to_str().unwrap_or("").to_string());
        }

        let display_uri = display_request_uri(&uri, mitm_destination, &host);
        let protocol_context = ProtocolContext::http(
            req.version(),
            request_scheme(&uri, mitm_destination),
            infer_application_protocol(&req_headers),
            infer_body_mode(&method, &req_headers),
        )
        .with_identity(connection_id.clone(), stream_id);

        RequestHead {
            method,
            uri,
            host,
            remote_addr,
            connection_id,
            stream_id,
            downstream_protocol,
            req_method,
            req_uri,
            req_headers,
            display_uri,
            protocol_context,
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
        let RequestHead {
            method,
            uri,
            host,
            remote_addr,
            connection_id,
            stream_id,
            downstream_protocol,
            req_method,
            req_uri,
            req_headers,
            display_uri,
            protocol_context,
        } = self.build_request_head(&req, mitm_destination.as_deref());

        // CONNECT is handled at the hyper service level in main.rs (before axum
        // middleware) so it never reaches here.  Return BAD_GATEWAY as a safety net.
        if method == axum::http::Method::CONNECT {
            return (
                StatusCode::BAD_GATEWAY,
                "CONNECT should be handled upstream",
            )
                .into_response();
        }

        // Decide the forwarding class from the active plugins' body hints BEFORE
        // the body is buffered (head-only, per the streaming contract). Today
        // every plugin defaults to BodyHint::FullBody, so this resolves to
        // ForwardClass::Buffered and the body is buffered exactly as before; the
        // streaming branch is introduced when handle_request is split. Computing
        // it here makes the decision load-bearing and observable now.
        let forward_class = {
            let head_ctx = RequestContext {
                method: req_method.clone(),
                uri: display_uri.clone(),
                headers: req_headers.clone(),
                host: host.clone(),
                protocol_context: Some(protocol_context.clone()),
                ..Default::default()
            };
            self.middleware_chain
                .read()
                .await
                .capability_plan(&head_ctx, &protocol_context)
        };
        debug!(
            execution = ?forward_class.execution,
            forward_class = ?forward_class.forward_class,
            diagnostic = ?forward_class.diagnostic,
            protocol = protocol_context.downstream.label(),
            body_mode = ?protocol_context.body_mode,
            "Forwarding capability plan decided"
        );

        // Streaming class: forward without buffering the body. Reached only when
        // every active plugin declared a streaming-capable body hint (inspect-only
        // in v1), so no plugin needs the whole body and none can mutate it. The
        // move of `req`/`req_headers`/`uri` here is sound: this branch diverges
        // (returns), so the buffered path below still owns them when it runs.
        if forward_class.forward_class == crate::core::forward::ForwardClass::Streaming {
            return self
                .forward_stream(
                    req,
                    StreamRequest {
                        method: req_method,
                        uri,
                        display_uri,
                        headers: req_headers,
                        host,
                        destination: mitm_destination,
                        started_at: start,
                        identity: StreamIdentity {
                            connection_id,
                            stream_id,
                            downstream_protocol,
                            remote_addr,
                        },
                        protocol_context,
                    },
                )
                .await;
        }

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
            connection_id,
            stream_id,
            downstream_protocol,
            remote_addr,
            protocol_context: Some(protocol_context.clone()),
            ..Default::default()
        };

        debug!("Executing request middleware chain");
        if let Err(response) = self
            .execute_request_middleware(
                &mut req_ctx,
                RequestMetadata {
                    uri: &display_uri,
                    host: &host,
                    method: &req_method,
                },
            )
            .await
        {
            return response;
        }

        let upstream = match self.prepare_upstream(&mut req_ctx, &uri, &req_uri) {
            Ok(upstream) => upstream,
            Err(response) => return *response,
        };
        debug!(url = %upstream.url, "Forwarding request");

        // Use the no-timeout streaming client for all proxied requests.
        // Proxied traffic can be arbitrarily long (SSE, large downloads); the timeout
        // client (pool.0) is reserved for control-plane outbound calls via http_client().
        let client = self.clients.read().await.1.clone();

        let mut request_builder = client
            .request(upstream.method, &upstream.url)
            .headers(upstream.headers);

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
                // Record the negotiated upstream protocol for observability.
                let upstream_protocol = protocol_label(res.version()).to_string();
                let mut res_headers = crate::middleware::HeaderMap::new();
                for (name, value) in res.headers().iter() {
                    // append (not insert) so duplicate upstream headers such as
                    // multiple Set-Cookie survive instead of collapsing to the last one.
                    res_headers.append(name.to_string(), value.to_str().unwrap_or("").to_string());
                }

                let content_type = header_value(&res_headers, "content-type").unwrap_or_default();
                strip_hop_by_hop_response_headers(&mut res_headers);

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
                        session_id: upstream.session_id,
                        ttfb_ms,
                        request_host: host.clone(),
                        request_method: req_method.clone(),
                        protocol: Some(upstream_protocol.clone()),
                        protocol_context: req_ctx.protocol_context.clone(),
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
                    session_id: upstream.session_id,
                    ttfb_ms,
                    body_ms,
                    request_host: host.clone(),
                    request_method: req_method.clone(),
                    protocol: Some(upstream_protocol.clone()),
                    protocol_context: req_ctx.protocol_context.clone(),
                    ..Default::default()
                };

                debug!("Executing response middleware chain");
                if let Err(response) = self.execute_response_middleware(&mut res_ctx).await {
                    return response;
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

                let builder = self.response_builder(&res_ctx);

                // gRPC over HTTP/2 requires the response to end with a HEADERS frame
                // (trailers) carrying `grpc-status`, NOT with DATA+END_STREAM.  reqwest
                // silently discards upstream trailer frames, so we synthesise
                // `grpc-status: 0` for HTTP 200 gRPC responses.  Non-200 statuses are
                // left as plain bodies (the client already sees an error from the status
                // code).  TODO: surface the actual upstream grpc-status once reqwest
                // exposes trailer access (tracked upstream issue).
                let is_grpc = content_type.starts_with("application/grpc");
                if is_grpc && res_ctx.status == 200 {
                    let mut grpc_trailers = axum::http::HeaderMap::new();
                    grpc_trailers.insert(
                        axum::http::header::HeaderName::from_static("grpc-status"),
                        axum::http::header::HeaderValue::from_static("0"),
                    );
                    let body_with_trailers =
                        Full::new(res_ctx.body).with_trailers(ready(Some(Ok::<
                            _,
                            std::convert::Infallible,
                        >(
                            grpc_trailers
                        ))));
                    builder
                        .body(Body::new(body_with_trailers))
                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
                } else {
                    builder
                        .body(Body::from(res_ctx.body))
                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
                }
            }
            Err(error) => {
                self.record_forward_failure(
                    &error,
                    RequestMetadata {
                        uri: &display_uri,
                        host: &host,
                        method: &req_method,
                    },
                    upstream.session_id,
                    upstream.protocol_context,
                    net_start.elapsed().as_millis() as u64,
                )
                .await
            }
        }
    }

    /// Streaming-class forwarding: relays the request body upstream without
    /// buffering and streams the response straight back. Reached only when every
    /// active plugin declared a streaming-capable body hint, so the request chain
    /// runs head-only (no plugin needs or mutates the body) and inspection is
    /// limited to headers/metadata in v1 (streaming = inspect-only).
    async fn forward_stream(
        self: Arc<Self>,
        req: Request<Body>,
        request: StreamRequest,
    ) -> Response {
        let StreamRequest {
            method: req_method,
            uri,
            display_uri,
            headers: req_headers,
            host,
            destination: mitm_destination,
            started_at: start,
            identity,
            protocol_context,
        } = request;
        let req_uri = uri.to_string();
        let mut req_ctx = RequestContext {
            method: req_method.clone(),
            uri: display_uri.clone(),
            headers: req_headers,
            host: host.clone(),
            destination: mitm_destination,
            connection_id: identity.connection_id,
            stream_id: identity.stream_id,
            downstream_protocol: identity.downstream_protocol,
            remote_addr: identity.remote_addr,
            protocol_context: Some(protocol_context.clone()),
            ..Default::default()
        };

        if let Err(response) = self
            .execute_request_middleware(
                &mut req_ctx,
                RequestMetadata {
                    uri: &display_uri,
                    host: &host,
                    method: &req_method,
                },
            )
            .await
        {
            return response;
        }

        let upstream = match self.prepare_upstream(&mut req_ctx, &uri, &req_uri) {
            Ok(upstream) => upstream,
            Err(response) => return *response,
        };

        let client = self.clients.read().await.1.clone();

        // Collect stream observers now — after the request chain has set session_id
        // and other side-channel fields — so observers can capture that state.
        // Observers are created before send() so they can also tap the request body.
        let observers_arc = {
            let chain = self.middleware_chain.read().await;
            let observers = chain.stream_observers(&req_ctx);
            std::sync::Arc::new(tokio::sync::Mutex::new(observers))
        };

        // Relay request body as a stream, feeding each chunk to observers.
        let obs_for_req = observers_arc.clone();
        let body_stream = async_stream::stream! {
            let mut body = req.into_body().into_data_stream();
            while let Some(result) = body.next().await {
                match result {
                    Ok(bytes) => {
                        let mut next = Some(bytes);
                        let mut observers = obs_for_req.lock().await;
                        for observer in observers.iter_mut() {
                            let Some(bytes) = next.take() else { break };
                            next = observer.on_request_chunk(bytes).await;
                        }
                        drop(observers);
                        if let Some(bytes) = next
                            && !bytes.is_empty()
                        {
                            yield Ok::<_, axum::Error>(bytes);
                        }
                    }
                    Err(err) => yield Err(err),
                }
            }
        };
        let request_builder = client
            .request(upstream.method, &upstream.url)
            .headers(upstream.headers)
            .body(reqwest::Body::wrap_stream(body_stream));

        let net_start = Instant::now();
        match request_builder.send().await {
            Ok(res) => {
                let ttfb_ms = net_start.elapsed().as_millis() as u64;
                let status = res.status().as_u16();
                let upstream_protocol = protocol_label(res.version()).to_string();
                let mut res_headers = crate::middleware::HeaderMap::new();
                for (name, value) in res.headers().iter() {
                    res_headers.append(name.to_string(), value.to_str().unwrap_or("").to_string());
                }
                strip_hop_by_hop_response_headers(&mut res_headers);

                let mut res_ctx = ResponseContext {
                    status,
                    headers: res_headers,
                    request_uri: display_uri.clone(),
                    session_id: upstream.session_id,
                    ttfb_ms,
                    request_host: host.clone(),
                    request_method: req_method.clone(),
                    protocol: Some(upstream_protocol),
                    protocol_context: req_ctx.protocol_context.clone(),
                    // Signal to InspectionMiddleware that recording is deferred to
                    // the BodyObserver so that on_response skips size recording.
                    response_body_observer_pending: true,
                    ..Default::default()
                };

                if let Err(response) = self.execute_response_middleware(&mut res_ctx).await {
                    return response;
                }

                let encoded_response = header_value(&res_ctx.headers, "content-encoding")
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false);
                let content_type = header_value(&res_ctx.headers, "content-type")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let content_length = res.content_length();
                if encoded_response
                    && !content_type.contains("text/event-stream")
                    && content_length.is_none_or(|len| len <= self.max_body_bytes() as u64)
                {
                    let body_start = Instant::now();
                    // Read with the cap enforced DURING the read: chunked encoded
                    // responses carry no content-length, so buffering via
                    // `res.bytes()` first and checking the size afterwards would
                    // let a hostile upstream exhaust memory before the check runs.
                    let max = self.max_body_bytes();
                    let mut res = res;
                    let mut raw: Vec<u8> =
                        Vec::with_capacity(content_length.unwrap_or(0).min(64 * 1024) as usize);
                    let mut too_large = false;
                    while let Ok(Some(chunk)) = res.chunk().await {
                        if raw.len() + chunk.len() > max {
                            too_large = true;
                            break;
                        }
                        raw.extend_from_slice(&chunk);
                    }
                    if !too_large {
                        let raw = Bytes::from(raw);
                        let decoded = decoded_response_body(&mut res_ctx.headers, &raw);
                        res_ctx.body = decoded.clone();
                        res_ctx.body_ms = body_start.elapsed().as_millis() as u64;
                        res_ctx.response_body_observer_pending = false;

                        let mut observers = std::mem::take(&mut *observers_arc.lock().await);
                        for obs in &mut observers {
                            obs.on_response_head(&res_ctx, start).await;
                            let _ = obs.on_chunk(decoded.clone()).await;
                        }
                        for obs in observers {
                            obs.finish().await;
                        }

                        let builder = self.response_builder(&res_ctx);
                        return builder
                            .body(Body::from(decoded))
                            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
                    }
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!(
                            "encoded response exceeded max_body_bytes ({}) while normalizing content-encoding",
                            self.max_body_bytes()
                        ),
                    )
                        .into_response();
                }

                // Observers stay inside the shared Arc for the rest of the
                // exchange: for bidirectional streams (gRPC bidi) the response
                // head can arrive while the client is still sending request
                // messages, so stealing the observers here would silently stop
                // request-side frame inspection mid-stream. Both directions lock
                // the mutex per chunk instead; the response stream takes the
                // observers out only after the response body completes.
                {
                    let mut observers = observers_arc.lock().await;
                    for obs in observers.iter_mut() {
                        obs.on_response_head(&res_ctx, start).await;
                    }
                    // Observers may drop or resize frames (gRPC frame breakpoints),
                    // which would falsify a fixed content-length; the streamed body
                    // is delivered chunked instead.
                    if !observers.is_empty() {
                        remove_header(&mut res_ctx.headers, "content-length");
                    }
                }

                info!(
                    method = %req_method,
                    uri = %req_uri,
                    status = res_ctx.status,
                    latency_ms = start.elapsed().as_millis(),
                    "Streaming request completed"
                );

                let builder = self.response_builder(&res_ctx);
                let stream_body = axum::body::Body::from_stream(async_stream::stream! {
                    let mut r = res;
                    let observers_arc = observers_arc;
                    while let Ok(Some(chunk)) = r.chunk().await {
                        let mut next = Some(chunk);
                        {
                            // Lock held across on_chunk: a paused frame breakpoint
                            // intentionally pauses the whole exchange (both
                            // directions), matching buffered-path semantics.
                            let mut observers = observers_arc.lock().await;
                            for obs in observers.iter_mut() {
                                let Some(bytes) = next.take() else { break };
                                next = obs.on_chunk(bytes).await;
                            }
                        }
                        if let Some(bytes) = next
                            && !bytes.is_empty()
                        {
                            yield Ok::<_, reqwest::Error>(bytes);
                        }
                    }
                    // Response complete: take the observers out (a still-running
                    // request stream then sees an empty set) and finalize.
                    let observers = std::mem::take(&mut *observers_arc.lock().await);
                    for obs in observers {
                        obs.finish().await;
                    }
                });
                builder
                    .body(stream_body)
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            }
            Err(error) => {
                self.record_forward_failure(
                    &error,
                    RequestMetadata {
                        uri: &display_uri,
                        host: &host,
                        method: &req_method,
                    },
                    upstream.session_id,
                    upstream.protocol_context,
                    net_start.elapsed().as_millis() as u64,
                )
                .await
            }
        }
    }
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

    #[test]
    fn decoded_response_body_decodes_zstd() {
        let compressed = zstd::stream::encode_all(&b"hello-zstd-body"[..], 0).unwrap();
        let mut headers = crate::middleware::HeaderMap::new();
        headers.insert("Content-Encoding".to_string(), "zstd".to_string());
        headers.insert("Content-Length".to_string(), compressed.len().to_string());

        let bytes = decoded_response_body(&mut headers, &Bytes::from(compressed));

        assert_eq!(&bytes[..], b"hello-zstd-body");
        assert!(!headers.contains_key("Content-Encoding"));
        assert!(!headers.contains_key("Content-Length"));
    }

    #[test]
    fn decoded_response_body_strips_stale_zstd_header_for_plain_body() {
        let mut headers = crate::middleware::HeaderMap::new();
        headers.insert("Content-Encoding".to_string(), "zstd".to_string());
        headers.insert("Content-Length".to_string(), "15".to_string());

        let bytes = decoded_response_body(&mut headers, &Bytes::from_static(b"already decoded"));

        assert_eq!(&bytes[..], b"already decoded");
        assert!(!headers.contains_key("Content-Encoding"));
        assert!(!headers.contains_key("Content-Length"));
    }
}
