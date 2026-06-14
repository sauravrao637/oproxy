//! Pure request/response wire helpers: HTTP protocol inference, header
//! sanitisation, and upstream URL/target construction. These functions hold
//! no `ProxyEngine` state, so they live apart from the engine proper to keep
//! that module focused on request handling.

use crate::core::forward::{ApplicationProtocol, BodyMode};
use crate::middleware::{header_value, remove_header};

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

/// Human-readable label for an HTTP version, used for protocol observability.
pub fn protocol_label(v: axum::http::Version) -> &'static str {
    match v {
        axum::http::Version::HTTP_09 => "HTTP/0.9",
        axum::http::Version::HTTP_10 => "HTTP/1.0",
        axum::http::Version::HTTP_11 => "HTTP/1.1",
        axum::http::Version::HTTP_2 => "HTTP/2",
        axum::http::Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/?",
    }
}

pub(super) fn infer_application_protocol(
    headers: &crate::middleware::HeaderMap,
) -> ApplicationProtocol {
    let content_type = header_value(headers, "content-type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.starts_with("application/grpc") {
        ApplicationProtocol::Grpc
    } else if content_type.starts_with("text/event-stream") {
        ApplicationProtocol::Sse
    } else if content_type.contains("graphql") {
        ApplicationProtocol::Graphql
    } else if content_type.contains("json") {
        ApplicationProtocol::Json
    } else if is_binary_content_type(&content_type) {
        ApplicationProtocol::Binary
    } else {
        ApplicationProtocol::Http
    }
}

pub(super) fn infer_body_mode(
    method: &axum::http::Method,
    headers: &crate::middleware::HeaderMap,
) -> BodyMode {
    if header_value(headers, "upgrade")
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
    {
        return BodyMode::Frames;
    }
    if infer_application_protocol(headers) == ApplicationProtocol::Grpc {
        return BodyMode::StreamMessages;
    }
    if (method == axum::http::Method::GET
        || method == axum::http::Method::HEAD
        || method == axum::http::Method::OPTIONS)
        && !headers.contains_key("content-length")
        && !headers.contains_key("transfer-encoding")
    {
        return BodyMode::Empty;
    }
    let chunked = header_value(headers, "transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);
    let large = header_value(headers, "content-length")
        .and_then(|v| v.parse::<u64>().ok())
        .map(|len| len > super::STREAM_THRESHOLD_BYTES)
        .unwrap_or(false);
    if chunked || large {
        BodyMode::StreamBytes
    } else {
        BodyMode::Full
    }
}

/// Drops client-supplied `x-oproxy-*` headers (side-channel spoofing) and
/// hop-by-hop headers — illegal in HTTP/2 and never forwarded. Exception:
/// `te: trailers` is required by gRPC and explicitly allowed in HTTP/2 requests
/// (RFC 7540 §8.1.2.2), so `te` is stripped only for any other value.
/// Shared by the buffered and streaming forward paths.
pub(super) fn sanitize_forwarded_request_headers(headers: &mut crate::middleware::HeaderMap) {
    headers.retain(|name, _| !name.trim().to_ascii_lowercase().starts_with("x-oproxy-"));
    for hdr in &[
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "trailer",
        "trailers",
        "upgrade",
    ] {
        headers.remove(hdr);
    }
    headers.retain(|name, value| name != "te" || value.trim().eq_ignore_ascii_case("trailers"));
}

/// Strips hop-by-hop headers from an upstream response before it is replayed
/// to the downstream client. Shared by the buffered and streaming paths.
pub(super) fn strip_hop_by_hop_response_headers(headers: &mut crate::middleware::HeaderMap) {
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
        remove_header(headers, hdr);
    }
}

pub(super) fn request_scheme(uri: &axum::http::Uri, mitm_destination: Option<&str>) -> String {
    if let Some(destination) = mitm_destination
        && let Some((scheme, _)) = destination.split_once("://")
    {
        return scheme.to_string();
    }
    uri.scheme_str().unwrap_or("http").to_string()
}

pub(super) fn display_request_uri(
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

pub(super) fn upstream_path(uri: &axum::http::Uri, fallback: &str) -> String {
    uri.path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| {
            if fallback.starts_with('/') {
                fallback.to_string()
            } else {
                "/".to_string()
            }
        })
}

pub(super) fn target_url(
    destination: Option<&str>,
    request_host: &str,
    path: &str,
    headers: &mut crate::middleware::HeaderMap,
) -> String {
    let Some(destination) = destination else {
        return format!("http://{request_host}{path}");
    };

    let destination = destination.trim_end_matches('/');
    let base = if destination.starts_with("http://") || destination.starts_with("https://") {
        destination.to_string()
    } else {
        format!("http://{destination}")
    };

    if let Ok(url) = reqwest::Url::parse(&base)
        && let Some(host) = url.host_str()
    {
        let host = url
            .port()
            .map(|port| format!("{host}:{port}"))
            .unwrap_or_else(|| host.to_string());
        headers.insert("host".to_string(), host);
    }

    format!("{base}{path}")
}

pub(super) fn upstream_headers(
    headers: &crate::middleware::HeaderMap,
) -> reqwest::header::HeaderMap {
    let mut upstream = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        if name == "host" || name == "content-length" {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            upstream.append(name, value);
        }
    }
    upstream
}
