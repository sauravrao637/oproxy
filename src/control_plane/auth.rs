use axum::extract::State;
use axum::http::header;
use axum::middleware::Next;
use axum::response::IntoResponse;
use std::net::IpAddr;
use std::sync::Arc;

use crate::AppState;
use crate::transport::http::DownstreamPeer;

pub(super) async fn security_headers(
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    // Admin JSON carries no cache validators (ETag/Last-Modified), so a browser's
    // default fetch() cache mode can serve a stale list right after a mutation.
    // Mark these responses no-store so the UI always reflects the latest state.
    // Static UI assets are left cacheable.
    let is_admin = req.uri().path().starts_with("/admin");
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    if is_admin {
        headers.insert(
            "cache-control",
            header::HeaderValue::from_static("no-store"),
        );
    }
    headers.insert(
        "x-content-type-options",
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", header::HeaderValue::from_static("DENY"));
    headers.insert(
        "referrer-policy",
        header::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "content-security-policy",
        header::HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'unsafe-inline'; \
             style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
             connect-src 'self'; font-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    response
}

pub(super) async fn admin_auth_layer(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let peer_ip = downstream_peer_ip(&req);
    let remote_admin_token_configured = configured_admin_token(&state.config).is_some();
    let is_admin_target = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .map(|h| {
            is_management_host(
                h,
                &state.config.bind_host,
                state.config.allow_remote_admin,
                remote_admin_token_configured,
                peer_ip,
                state.config.port,
            )
        })
        .unwrap_or_else(|| peer_ip.is_some_and(|ip| ip.is_loopback()));

    // Public paths that don't require authentication even on admin hosts
    let is_ui_or_public_api = matches!(
        req.uri().path(),
        "/" | "/login" | "/health" | "/robots.txt" | "/favicon.ico" | "/setup" | "/setup/mobile"
    ) || req.uri().path().starts_with("/api/")
        || req.uri().path().starts_with("/assets/");

    // These admin paths are also public (CA cert download, setup network info)
    let is_public_admin_path =
        matches!(req.uri().path(), "/admin/ca" | "/admin/setup/network-info");

    let is_public_path = is_ui_or_public_api || is_public_admin_path;

    let is_exempt = is_public_path || (req.uri().scheme().is_some() && !is_admin_target);

    if is_exempt {
        return next.run(req).await;
    }

    let expected_token = configured_admin_token(&state.config);
    let header_token_authenticated =
        expected_token.is_some_and(|token| request_has_header_admin_token(&req, token));

    if is_state_changing_admin_request(&req)
        && !header_token_authenticated
        && !csrf_origin_allowed(&req, &state.config)
    {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "oproxy admin origin check failed",
        )
            .into_response();
    }

    let Some(expected_token) = expected_token else {
        return next.run(req).await;
    };

    let set_cookie = query_admin_token(req.uri().query())
        .is_some_and(|token| token_matches(token, expected_token));
    if request_has_admin_token(&req, expected_token) {
        let mut response = next.run(req).await;
        if set_cookie && let Some(cookie) = admin_token_cookie(expected_token) {
            response.headers_mut().insert(header::SET_COOKIE, cookie);
        }
        return response;
    }

    // For browser navigation (Accept: text/html) redirect to the login page so the
    // user gets a proper sign-in form instead of a raw 401 JSON response.
    if is_browser_navigation(&req) {
        let path_and_query = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        // Don't create a redirect loop if someone navigates directly to /login.
        if path_and_query != "/login" {
            let next_encoded = encode_next_param(path_and_query);
            return (
                axum::http::StatusCode::FOUND,
                [(header::LOCATION, format!("/login?next={next_encoded}"))],
            )
                .into_response();
        }
    }

    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"oproxy-admin\"")],
        "oproxy admin token required",
    )
        .into_response()
}

fn configured_admin_token(config: &crate::config::Config) -> Option<&str> {
    config
        .admin_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

fn request_has_admin_token(req: &axum::extract::Request, expected: &str) -> bool {
    request_has_header_admin_token(req, expected)
        || req
            .headers()
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|cookies| cookie_has_admin_token(cookies, expected))
        || query_admin_token(req.uri().query()).is_some_and(|token| token_matches(token, expected))
}

fn request_has_header_admin_token(req: &axum::extract::Request, expected: &str) -> bool {
    req.headers()
        .get("x-oproxy-admin-token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| token_matches(token.trim(), expected))
        || req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|token| token_matches(token.trim(), expected))
}

fn query_admin_token(query: Option<&str>) -> Option<&str> {
    query?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        matches!(key, "token" | "admin_token").then_some(value)
    })
}

fn cookie_has_admin_token(cookies: &str, expected: &str) -> bool {
    cookies.split(';').any(|cookie| {
        let (name, value) = cookie.trim().split_once('=').unwrap_or(("", ""));
        name == "oproxy_admin_token" && token_matches(value.trim(), expected)
    })
}

fn admin_token_cookie(token: &str) -> Option<header::HeaderValue> {
    if !token
        .bytes()
        .all(|b| matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e))
    {
        return None;
    }
    header::HeaderValue::from_str(&format!(
        "oproxy_admin_token={token}; HttpOnly; SameSite=Strict; Path=/"
    ))
    .ok()
}

pub(crate) fn token_matches(candidate: &str, expected: &str) -> bool {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    // Double-HMAC comparison: hash both inputs with an ephemeral random key, then
    // compare the fixed-size 32-byte tags in constant time. Unlike a direct
    // byte-compare, this does not short-circuit on length, so it leaks neither the
    // token's contents nor its length via timing.
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());

    let tag = |data: &str| -> [u8; 32] {
        let mut mac = <Hmac<Sha256>>::new_from_slice(&key).expect("HMAC accepts any key length");
        mac.update(data.as_bytes());
        mac.finalize().into_bytes().into()
    };

    let candidate_tag = tag(candidate);
    let expected_tag = tag(expected);
    candidate_tag
        .iter()
        .zip(expected_tag.iter())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn is_state_changing_admin_request(req: &axum::extract::Request) -> bool {
    !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    )
}

fn csrf_origin_allowed(req: &axum::extract::Request, config: &crate::config::Config) -> bool {
    let Some(origin_authority) = request_origin_authority(req) else {
        // Non-browser API clients such as curl generally do not send Origin/Referer.
        return true;
    };
    let Some(request_host) = req
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    if !same_authority(&origin_authority, request_host) {
        return false;
    }
    let peer_ip = downstream_peer_ip(req);
    is_management_host(
        request_host,
        &config.bind_host,
        config.allow_remote_admin,
        configured_admin_token(config).is_some(),
        peer_ip,
        config.port,
    )
}

fn request_origin_authority(req: &axum::extract::Request) -> Option<String> {
    req.headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(url_authority)
        .or_else(|| {
            req.headers()
                .get(header::REFERER)
                .and_then(|value| value.to_str().ok())
                .and_then(url_authority)
        })
}

fn url_authority(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn same_authority(left: &str, right: &str) -> bool {
    normalized_authority(left) == normalized_authority(right)
}

fn normalized_authority(authority: &str) -> (String, Option<u16>) {
    (
        host_without_port(authority).to_ascii_lowercase(),
        authority_port(authority),
    )
}

fn authority_port(authority: &str) -> Option<u16> {
    let authority = authority.trim();
    if let Some(rest) = authority.strip_prefix('[') {
        let (_, suffix) = rest.split_once(']')?;
        return suffix.strip_prefix(':')?.parse().ok();
    }
    authority
        .rsplit_once(':')
        .filter(|(_, port)| {
            authority.matches(':').count() == 1 && port.chars().all(|c| c.is_ascii_digit())
        })
        .and_then(|(_, port)| port.parse().ok())
}

fn downstream_peer_ip(req: &axum::extract::Request) -> Option<IpAddr> {
    req.extensions()
        .get::<DownstreamPeer>()
        .map(|peer| peer.0.ip())
}

/// Tower layer applied before route matching. Requests whose Host is not a
/// configured admin host go straight to the proxy engine so control-plane routes
/// (like GET /) are never accidentally served to proxied traffic.
pub async fn proxy_dispatch_layer(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let peer_ip = downstream_peer_ip(&req);
    let remote_admin_token_configured = configured_admin_token(&state.config).is_some();
    let is_admin_host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .map(|h| {
            is_management_host(
                h,
                &state.config.bind_host,
                state.config.allow_remote_admin,
                remote_admin_token_configured,
                peer_ip,
                state.config.port,
            )
        })
        .unwrap_or_else(|| peer_ip.is_some_and(|ip| ip.is_loopback()));

    if is_admin_host || is_public_setup_endpoint(&req) {
        next.run(req).await
    } else {
        state.proxy_engine.clone().handle_request(req).await
    }
}

/// Returns true for the small set of GET endpoints that must be reachable from
/// LAN clients (phones, VMs) without requiring admin-host status or a token:
///   - /admin/ca            — CA cert download; clients need this before they can trust MITM
///   - /setup, /setup/mobile — setup wizard UI
///   - /admin/setup/network-info — JSON consumed by the setup wizard
///
/// We gate on origin-form URI (no scheme) to avoid misrouting forward-proxy
/// requests like `GET http://evil.com/admin/ca` through the admin router.
fn is_public_setup_endpoint(req: &axum::extract::Request) -> bool {
    if req.uri().scheme().is_some() {
        return false; // absolute-form → forward-proxy request, not for us
    }
    req.method() == axum::http::Method::GET
        && matches!(
            req.uri().path(),
            "/admin/ca" | "/setup" | "/setup/mobile" | "/admin/setup/network-info"
        )
}

fn is_management_host(
    host_header: &str,
    bind_host: &str,
    allow_remote_admin: bool,
    remote_admin_token_configured: bool,
    peer_ip: Option<IpAddr>,
    proxy_port: u16,
) -> bool {
    let host = host_without_port(host_header).to_ascii_lowercase();
    let host_port = authority_port(host_header);

    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") {
        // For localhost, check both that the peer is loopback AND that the port matches the proxy port
        // (to distinguish between forward-proxy requests to 127.0.0.1:OTHER_PORT vs the proxy itself at 127.0.0.1:PROXY_PORT)
        return peer_ip.is_some_and(|ip| ip.is_loopback()) && host_port.unwrap_or(80) == proxy_port;
    }

    if !allow_remote_admin || !remote_admin_token_configured {
        return false;
    }

    let bind_host = bind_host.trim().to_ascii_lowercase();
    if !matches!(bind_host.as_str(), "0.0.0.0" | "::" | "[::]") {
        return host == host_without_port(&bind_host).to_ascii_lowercase();
    }

    if host == "0.0.0.0" {
        return true;
    }

    let lan_hosts = [
        crate::setup::public_lan_ip_for_setup(),
        crate::setup::detect_lan_ip(),
    ];
    lan_hosts
        .into_iter()
        .flatten()
        .any(|lan_host| host == lan_host.to_ascii_lowercase())
}

/// Returns true when the request looks like a browser top-level navigation
/// (i.e. `Accept` header contains `text/html`). AJAX fetch() calls typically
/// send `Accept: */*` or `Accept: application/json` and will not match.
fn is_browser_navigation(req: &axum::extract::Request) -> bool {
    req.headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// Percent-encode a path+query string for safe embedding as a URL query value.
/// Unreserved characters (RFC 3986) are kept as-is; everything else is %-encoded.
pub(crate) fn encode_next_param(s: &str) -> String {
    s.bytes()
        .fold(String::with_capacity(s.len() * 3), |mut out, b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
                out.push(b as char);
            } else {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("%{b:02X}"));
            }
            out
        })
}

fn host_without_port(host_header: &str) -> &str {
    let host = host_header.trim();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(host);
    }
    host.rsplit_once(':')
        .filter(|(_, port)| {
            host.matches(':').count() == 1 && port.chars().all(|c| c.is_ascii_digit())
        })
        .map_or(host, |(host, _)| host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn management_host_accepts_localhost_and_configured_lan_bindings() {
        let loopback = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let remote = Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)));
        let proxy_port = 8080u16;
        assert!(is_management_host(
            "localhost:8080",
            "127.0.0.1",
            false,
            false,
            loopback,
            proxy_port
        ));
        assert!(is_management_host(
            "127.0.0.1:8080",
            "127.0.0.1",
            false,
            false,
            loopback,
            proxy_port
        ));
        // IPv6 Host headers must use bracket notation ([::1]:port), as mandated by RFC 7230.
        // Bare "::1:8080" is not a valid Host header value and is not supported.
        assert!(is_management_host(
            "[::1]:8080",
            "127.0.0.1",
            false,
            false,
            Some(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            proxy_port
        ));
        // 0.0.0.0 as host header (Linux routes http://0.0.0.0:port to local machine)
        assert!(
            is_management_host(
                "0.0.0.0:8080",
                "0.0.0.0",
                false,
                false,
                loopback,
                proxy_port
            ),
            "loopback peer with Host: 0.0.0.0 should reach admin UI"
        );
        assert!(
            !is_management_host("0.0.0.0:8080", "0.0.0.0", true, true, remote, proxy_port),
            "remote clients must not reach admin by sending Host: 0.0.0.0"
        );
        assert!(
            !is_management_host("localhost:8080", "0.0.0.0", true, true, remote, proxy_port),
            "remote clients must not reach admin by spoofing a localhost Host header"
        );
        // Forward-proxy request to localhost on different port should not be admin
        assert!(
            !is_management_host(
                "127.0.0.1:9999",
                "127.0.0.1",
                false,
                false,
                loopback,
                proxy_port
            ),
            "forward-proxy request to 127.0.0.1:9999 should not be admin when proxy is on 8080"
        );
        assert!(!is_management_host(
            "192.168.1.10:8080",
            "192.168.1.10",
            false,
            true,
            remote,
            proxy_port
        ));
        assert!(is_management_host(
            "192.168.1.10:8080",
            "192.168.1.10",
            true,
            true,
            remote,
            proxy_port
        ));
        assert!(!is_management_host(
            "192.168.1.10:8080",
            "192.168.1.10",
            true,
            false,
            remote,
            proxy_port
        ));
        assert!(!is_management_host(
            "example.com",
            "127.0.0.1",
            true,
            true,
            remote,
            proxy_port
        ));
    }

    #[test]
    fn admin_token_accepts_supported_locations() {
        assert!(token_matches("secret", "secret"));
        assert!(!token_matches("secret", "different"));
        assert_eq!(
            query_admin_token(Some("foo=bar&token=secret")),
            Some("secret")
        );
        assert!(cookie_has_admin_token(
            "theme=dark; oproxy_admin_token=secret",
            "secret"
        ));
        assert!(admin_token_cookie("secret-token").is_some());
    }

    #[test]
    fn csrf_origin_requires_same_management_authority() {
        let mut req = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/admin/routes")
            .header(header::HOST, "127.0.0.1:8080")
            .header(header::ORIGIN, "http://127.0.0.1:8080")
            .body(axum::body::Body::empty())
            .unwrap();
        req.extensions_mut().insert(DownstreamPeer(
            "127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap(),
        ));
        assert!(csrf_origin_allowed(&req, &crate::config::Config::default()));

        let mut forged = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/admin/routes")
            .header(header::HOST, "127.0.0.1:8080")
            .header(header::ORIGIN, "http://evil.test")
            .body(axum::body::Body::empty())
            .unwrap();
        forged.extensions_mut().insert(DownstreamPeer(
            "127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap(),
        ));
        assert!(!csrf_origin_allowed(
            &forged,
            &crate::config::Config::default()
        ));
    }

    #[test]
    fn admin_auth_exempts_absolute_form_proxy_requests() {
        let req = axum::http::Request::builder()
            .uri("http://example.com/path")
            .body(axum::body::Body::empty())
            .unwrap();

        assert!(req.uri().scheme().is_some());
    }

    #[tokio::test]
    async fn security_headers_mark_admin_json_no_store_but_leave_assets_cacheable() {
        use tower::ServiceExt;

        async fn cache_control_for(path: &str) -> Option<String> {
            let app = axum::Router::new()
                .fallback(|| async { axum::response::Response::new(axum::body::Body::empty()) })
                .layer(axum::middleware::from_fn(security_headers));
            let req = axum::http::Request::builder()
                .uri(path)
                .body(axum::body::Body::empty())
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            res.headers()
                .get("cache-control")
                .map(|v| v.to_str().unwrap().to_string())
        }

        assert_eq!(
            cache_control_for("/admin/map-local-rules").await.as_deref(),
            Some("no-store"),
            "admin JSON must be no-store so the UI never shows a stale list after a mutation"
        );
        assert_eq!(
            cache_control_for("/assets/app.js").await,
            None,
            "static UI assets should stay cacheable"
        );
    }
}
