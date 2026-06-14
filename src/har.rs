use crate::core::engine::is_binary_content_type;
use crate::middleware::{RequestContext, ResponseContext};

fn null_as_empty_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    use serde::Deserialize as _;
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}
/// HAR 1.2 (HTTP Archive) serialisation/deserialisation.
/// Spec: http://www.softwareishard.com/blog/har-12-spec/
///
/// oproxy-specific extension fields are prefixed with `_oproxy_` and are
/// preserved on import so sessions roundtrip without data loss.
use crate::session::{Exchange, InspectionMetrics, SessionSource};
use base64::Engine as _;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Render body bytes for the HAR `text` field: base64 for binary content types
/// (matching how binary bodies are presented elsewhere) and lossy UTF-8 otherwise.
fn har_body_text(body: &Bytes, mime_type: &str) -> String {
    if is_binary_content_type(mime_type) {
        base64::engine::general_purpose::STANDARD.encode(body)
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

/// Inverse of [`har_body_text`]: decode the HAR `text` field back into raw bytes.
fn har_body_bytes(text: String, mime_type: &str) -> Bytes {
    if is_binary_content_type(mime_type) {
        base64::engine::general_purpose::STANDARD
            .decode(text.as_bytes())
            .map(Bytes::from)
            .unwrap_or_else(|_| Bytes::from(text.into_bytes()))
    } else {
        Bytes::from(text.into_bytes())
    }
}

// ── HAR types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Har {
    pub log: HarLog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarLog {
    pub version: String,
    pub creator: HarCreator,
    pub entries: Vec<HarEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarCreator {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarEntry {
    /// ISO 8601 timestamp.
    #[serde(rename = "startedDateTime")]
    pub started_date_time: String,
    /// Total time in ms.
    pub time: f64,
    pub request: HarRequest,
    pub response: HarResponse,
    pub timings: HarTimings,
    #[serde(
        rename = "serverIPAddress",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub server_ip_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    #[serde(rename = "cache")]
    pub cache: HarCache,
    // oproxy extensions ──────────────────────────────────────────────────────
    #[serde(
        rename = "_oproxy_id",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub oproxy_id: Option<String>,
    #[serde(
        rename = "_oproxy_note",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub oproxy_note: Option<String>,
    #[serde(
        rename = "_oproxy_tags",
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_as_empty_vec"
    )]
    pub oproxy_tags: Vec<String>,
    #[serde(
        rename = "_oproxy_updated_at",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub oproxy_updated_at: Option<String>,
    #[serde(
        rename = "_oproxy_stream_id",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub oproxy_stream_id: Option<u64>,
    #[serde(
        rename = "_oproxy_downstream_protocol",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub oproxy_downstream_protocol: Option<String>,
    #[serde(
        rename = "_oproxy_protocol_context",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub oproxy_protocol_context: Option<crate::core::forward::ProtocolContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarRequest {
    pub method: String,
    pub url: String,
    #[serde(rename = "httpVersion")]
    pub http_version: String,
    pub cookies: Vec<HarCookie>,
    pub headers: Vec<HarNameValue>,
    #[serde(rename = "queryString")]
    pub query_string: Vec<HarNameValue>,
    #[serde(rename = "postData", default, skip_serializing_if = "Option::is_none")]
    pub post_data: Option<HarPostData>,
    #[serde(rename = "headersSize")]
    pub headers_size: i64,
    #[serde(rename = "bodySize")]
    pub body_size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarResponse {
    pub status: u16,
    #[serde(rename = "statusText")]
    pub status_text: String,
    #[serde(rename = "httpVersion")]
    pub http_version: String,
    pub cookies: Vec<HarCookie>,
    pub headers: Vec<HarNameValue>,
    pub content: HarContent,
    #[serde(rename = "redirectURL")]
    pub redirect_url: String,
    #[serde(rename = "headersSize")]
    pub headers_size: i64,
    #[serde(rename = "bodySize")]
    pub body_size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarTimings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl: Option<f64>,
    /// Time from connection established to first byte (HAR calls this "wait").
    #[serde(default)]
    pub wait: f64,
    /// Body download time.
    #[serde(default)]
    pub receive: f64,
    /// Time to send request (bytes already buffered, so typically 0 for proxies).
    #[serde(default)]
    pub send: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarPostData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarContent {
    pub size: i64,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarCache {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarNameValue {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarCookie {
    pub name: String,
    pub value: String,
}

// ── Conversion: Exchange → HarEntry ──────────────────────────────────────────

pub fn exchange_to_har_entry(ex: &Exchange) -> HarEntry {
    let request = exchange_request_to_har(&ex.request);
    let (response, time, timings) = exchange_response_to_har(ex);

    HarEntry {
        started_date_time: ex.timestamp.to_rfc3339(),
        time,
        request,
        response,
        timings,
        server_ip_address: None,
        connection: ex.connection_id.clone(),
        cache: HarCache::default(),
        oproxy_id: Some(ex.id.clone()),
        oproxy_note: ex.note.clone(),
        oproxy_tags: ex.tags.clone(),
        oproxy_updated_at: ex.updated_at.map(|time| time.to_rfc3339()),
        oproxy_stream_id: ex.stream_id,
        oproxy_downstream_protocol: ex.downstream_protocol.clone(),
        oproxy_protocol_context: ex.protocol_context.clone(),
    }
}

fn exchange_request_to_har(request: &RequestContext) -> HarRequest {
    let headers = request
        .headers
        .iter()
        .filter(|(k, _)| !should_skip_har_header(k))
        .map(|(k, v)| HarNameValue {
            name: k.clone(),
            value: v.clone(),
        })
        .collect();
    let post_data = if request.body.is_empty() {
        None
    } else {
        let mime_type = request
            .headers
            .get("content-type")
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        Some(HarPostData {
            mime_type,
            text: request.body_text().into_owned(),
        })
    };
    HarRequest {
        method: request.method.clone(),
        url: request.uri.clone(),
        http_version: "HTTP/1.1".to_string(),
        cookies: extract_request_cookies(request),
        headers,
        query_string: parse_query_string(&request.uri),
        post_data,
        headers_size: -1,
        body_size: request.body.len() as i64,
    }
}

fn exchange_response_to_har(ex: &Exchange) -> (HarResponse, f64, HarTimings) {
    let Some(response) = &ex.response else {
        return (empty_har_response(), 0.0, HarTimings::default());
    };
    let headers = response
        .headers
        .iter()
        .filter(|(k, _)| !should_skip_har_header(k))
        .map(|(k, v)| HarNameValue {
            name: k.clone(),
            value: v.clone(),
        })
        .collect();
    let mime_type = response
        .headers
        .get("content-type")
        .cloned()
        .unwrap_or_else(|| "text/plain".to_string());
    let har_response = HarResponse {
        status: response.status,
        status_text: http_status_text(response.status),
        http_version: ex
            .metrics
            .as_ref()
            .and_then(|m| m.protocol.clone())
            .unwrap_or_else(|| "HTTP/1.1".to_string()),
        cookies: extract_response_cookies(response),
        headers,
        content: HarContent {
            size: response.body.len() as i64,
            mime_type: mime_type.clone(),
            text: if response.body.is_empty() {
                None
            } else {
                Some(har_body_text(&response.body, &mime_type))
            },
        },
        redirect_url: response
            .headers
            .get("location")
            .cloned()
            .unwrap_or_default(),
        headers_size: -1,
        body_size: response.body.len() as i64,
    };
    let (time, timings) = ex.metrics.as_ref().map_or_else(
        || (0.0, HarTimings::default()),
        |metrics| {
            let setup = metrics.dns_ms.unwrap_or(0)
                + metrics.tcp_connect_ms.unwrap_or(0)
                + metrics.tls_ms.unwrap_or(0);
            (
                metrics.latency_ms as f64,
                HarTimings {
                    dns: metrics.dns_ms.map(|value| value as f64),
                    connect: metrics.tcp_connect_ms.map(|value| value as f64),
                    ssl: metrics.tls_ms.map(|value| value as f64),
                    wait: metrics.ttfb_ms.saturating_sub(setup) as f64,
                    receive: metrics.body_ms as f64,
                    send: 0.0,
                },
            )
        },
    );
    (har_response, time, timings)
}

fn empty_har_response() -> HarResponse {
    HarResponse {
        status: 0,
        status_text: String::new(),
        http_version: "HTTP/1.1".to_string(),
        cookies: vec![],
        headers: vec![],
        content: HarContent {
            size: 0,
            mime_type: "application/octet-stream".to_string(),
            text: None,
        },
        redirect_url: String::new(),
        headers_size: -1,
        body_size: -1,
    }
}

// ── Conversion: HarEntry → Exchange ──────────────────────────────────────────

pub fn har_entry_to_exchange(entry: &HarEntry) -> Exchange {
    let headers: crate::middleware::HeaderMap = entry
        .request
        .headers
        .iter()
        .map(|nv| (nv.name.clone(), nv.value.clone()))
        .collect();

    let body = entry
        .request
        .post_data
        .as_ref()
        .map(|p| p.text.clone())
        .unwrap_or_default();

    let host = extract_host(&entry.request.url);

    let id = entry
        .oproxy_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let timestamp = entry
        .started_date_time
        .parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());

    let updated_at = entry
        .oproxy_updated_at
        .as_deref()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok());

    let response = har_response_to_context(entry, &id);
    let metrics = har_entry_metrics(entry);

    Exchange {
        id,
        timestamp,
        updated_at,
        request: RequestContext {
            method: entry.request.method.clone(),
            uri: entry.request.url.clone(),
            headers,
            body: Bytes::from(body.into_bytes()),
            host,
            ..Default::default()
        },
        response,
        metrics,
        source: SessionSource::Imported,
        ws_frames: vec![],
        events: vec![],
        note: entry.oproxy_note.clone(),
        tags: entry.oproxy_tags.clone(),
        inspector_data: None,
        paused_at: None,
        connection_id: entry.connection.clone(),
        stream_id: entry.oproxy_stream_id,
        downstream_protocol: entry.oproxy_downstream_protocol.clone(),
        protocol_context: entry.oproxy_protocol_context.clone(),
    }
}

fn har_response_to_context(entry: &HarEntry, session_id: &str) -> Option<ResponseContext> {
    if entry.response.status == 0 {
        return None;
    }
    let headers = entry
        .response
        .headers
        .iter()
        .map(|value| (value.name.clone(), value.value.clone()))
        .collect();
    let mime_type = entry
        .response
        .content
        .mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    Some(ResponseContext {
        status: entry.response.status,
        headers,
        body: har_body_bytes(
            entry.response.content.text.clone().unwrap_or_default(),
            mime_type,
        ),
        request_uri: entry.request.url.clone(),
        session_id: Some(session_id.to_string()),
        ttfb_ms: full_ttfb_ms(&entry.timings),
        body_ms: entry.timings.receive as u64,
        ..Default::default()
    })
}

fn har_entry_metrics(entry: &HarEntry) -> Option<InspectionMetrics> {
    if entry.time <= 0.0 && entry.response.status == 0 {
        return None;
    }
    Some(InspectionMetrics {
        latency_ms: entry.time as u64,
        request_size_bytes: entry.request.body_size.max(0) as usize,
        response_size_bytes: entry.response.body_size.max(0) as usize,
        status_code: entry.response.status,
        ttfb_ms: full_ttfb_ms(&entry.timings),
        body_ms: entry.timings.receive as u64,
        dns_ms: entry.timings.dns.map(|value| value as u64),
        tcp_connect_ms: entry.timings.connect.map(|value| value as u64),
        tls_ms: entry.timings.ssl.map(|value| value as u64),
        protocol: non_empty_string(&entry.response.http_version),
    })
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub fn exchanges_to_har(exchanges: &IndexMap<String, Exchange>) -> Har {
    Har {
        log: HarLog {
            version: "1.2".to_string(),
            creator: HarCreator {
                name: "oproxy".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            entries: exchanges.values().map(exchange_to_har_entry).collect(),
        },
    }
}

pub fn exchanges_to_har_redacted(exchanges: &IndexMap<String, Exchange>) -> Har {
    let redacted = exchanges
        .values()
        .cloned()
        .map(redact_exchange_for_export)
        .map(|ex| exchange_to_har_entry(&ex))
        .collect();

    Har {
        log: HarLog {
            version: "1.2".to_string(),
            creator: HarCreator {
                name: "oproxy".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            entries: redacted,
        },
    }
}

pub fn har_to_exchanges(har: &Har) -> Vec<Exchange> {
    har.log.entries.iter().map(har_entry_to_exchange).collect()
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn redact_exchange_for_export(mut ex: Exchange) -> Exchange {
    let sensitive_values =
        crate::redaction::sensitive_values(&ex.request.headers, &ex.request.body_text());
    ex.request.headers = crate::redaction::redact_headers(&ex.request.headers);
    strip_har_headers(&mut ex.request.headers);
    let redacted_req = crate::redaction::redact_body_text(&ex.request.body_text());
    ex.request.set_body_text(redacted_req);
    if let Some(response) = &mut ex.response {
        response.headers = crate::redaction::redact_headers(&response.headers);
        strip_har_headers(&mut response.headers);
        let redacted = crate::redaction::redact_body_text(&response.body_text());
        let redacted = crate::redaction::redact_known_values(&redacted, &sensitive_values);
        response.set_body_text(redacted);
    }
    ex
}

fn strip_har_headers(headers: &mut crate::middleware::HeaderMap) {
    headers.retain(|name, _| !should_skip_har_header(name));
}

fn full_ttfb_ms(timings: &HarTimings) -> u64 {
    let setup_ms =
        timings.dns.unwrap_or(0.0) + timings.connect.unwrap_or(0.0) + timings.ssl.unwrap_or(0.0);
    (setup_ms + timings.wait).max(0.0) as u64
}

fn should_skip_har_header(header: &str) -> bool {
    let header = header.trim().to_ascii_lowercase();
    matches!(
        header.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) || header.starts_with("x-oproxy-")
}

fn parse_query_string(uri: &str) -> Vec<HarNameValue> {
    let query = uri.find('?').map(|i| &uri[i + 1..]).unwrap_or("");
    if query.is_empty() {
        return vec![];
    }
    query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let name = parts.next()?;
            let value = parts.next().unwrap_or("");
            Some(HarNameValue {
                name: url_decode(name),
                value: url_decode(value),
            })
        })
        .collect()
}

fn url_decode(s: &str) -> String {
    s.replace('+', " ").replace("%20", " ")
}

fn extract_host(url: &str) -> String {
    let rest = if let Some(i) = url.find("://") {
        &url[i + 3..]
    } else {
        url
    };
    rest.split('/')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_string()
}

fn extract_request_cookies(req: &RequestContext) -> Vec<HarCookie> {
    req.headers
        .get("cookie")
        .map(|v| parse_cookies(v))
        .unwrap_or_default()
}

fn extract_response_cookies(res: &ResponseContext) -> Vec<HarCookie> {
    res.headers
        .get("set-cookie")
        .map(|v| v.split(';').next().map(parse_cookies).unwrap_or_default())
        .unwrap_or_default()
}

fn parse_cookies(s: &str) -> Vec<HarCookie> {
    s.split(';')
        .filter_map(|pair| {
            let mut parts = pair.trim().splitn(2, '=');
            let name = parts.next()?.trim().to_string();
            let value = parts.next().unwrap_or("").trim().to_string();
            if name.is_empty() {
                None
            } else {
                Some(HarCookie { name, value })
            }
        })
        .collect()
}

fn http_status_text(status: u16) -> String {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
    .to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{HeaderMap, RequestContext, ResponseContext};
    use crate::session::InspectionMetrics;
    use chrono::Utc;

    fn make_exchange(method: &str, uri: &str, status: u16, body: &str, req_body: &str) -> Exchange {
        let mut req_headers = HeaderMap::new();
        req_headers.insert("host".to_string(), "example.com".to_string());
        if !req_body.is_empty() {
            req_headers.insert("content-type".to_string(), "application/json".to_string());
        }
        let response = if status > 0 {
            let mut res_headers = HeaderMap::new();
            res_headers.insert("content-type".to_string(), "application/json".to_string());
            Some(ResponseContext {
                status,
                headers: res_headers,
                body: Bytes::from(body.to_string()),
                request_uri: uri.to_string(),
                session_id: Some("test-id".to_string()),
                ttfb_ms: 80,
                body_ms: 20,
                ..Default::default()
            })
        } else {
            None
        };
        let metrics = if status > 0 {
            Some(InspectionMetrics {
                latency_ms: 100,
                request_size_bytes: req_body.len(),
                response_size_bytes: body.len(),
                status_code: status,
                ttfb_ms: 80,
                body_ms: 20,
                ..Default::default()
            })
        } else {
            None
        };
        Exchange {
            id: "test-id".to_string(),
            timestamp: Utc::now(),
            updated_at: None,
            request: RequestContext {
                method: method.to_string(),
                uri: uri.to_string(),
                headers: req_headers,
                body: Bytes::from(req_body.to_string()),
                host: "example.com".to_string(),
                ..Default::default()
            },
            response,
            metrics,
            source: SessionSource::Proxy,
            ws_frames: vec![],
            events: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
            paused_at: None,
            connection_id: None,
            stream_id: None,
            downstream_protocol: None,
            protocol_context: None,
        }
    }

    // ── exchange_to_har_entry ──────────────────────────────────────────────────

    #[test]
    fn har_entry_has_correct_method_and_url() {
        let ex = make_exchange("GET", "https://example.com/api", 200, "ok", "");
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.request.method, "GET");
        assert_eq!(entry.request.url, "https://example.com/api");
    }

    #[test]
    fn har_entry_response_status_and_body() {
        let ex = make_exchange(
            "POST",
            "https://example.com/submit",
            201,
            r#"{"id":1}"#,
            "{}",
        );
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.response.status, 201);
        assert_eq!(entry.response.content.text.as_deref(), Some(r#"{"id":1}"#));
    }

    #[test]
    fn har_entry_post_data_populated_for_request_body() {
        let ex = make_exchange(
            "POST",
            "https://api.example.com/",
            200,
            "",
            r#"{"data":"x"}"#,
        );
        let entry = exchange_to_har_entry(&ex);
        assert!(entry.request.post_data.is_some());
        assert!(entry.request.post_data.unwrap().text.contains("data"));
    }

    #[test]
    fn redacted_har_masks_sensitive_headers_and_body() {
        let mut ex = make_exchange(
            "POST",
            "https://api.example.com/",
            200,
            r#"{"access_token":"response-token"}"#,
            r#"{"password":"request-password"}"#,
        );
        ex.request.headers.insert(
            "authorization".to_string(),
            "Bearer request-token".to_string(),
        );
        let mut map = IndexMap::new();
        map.insert(ex.id.clone(), ex);

        let har = exchanges_to_har_redacted(&map);
        let json = serde_json::to_string(&har).unwrap();

        assert!(json.contains(crate::redaction::REDACTED));
        assert!(!json.contains("request-token"));
        assert!(!json.contains("request-password"));
        assert!(!json.contains("response-token"));
    }

    #[test]
    fn har_export_omits_internal_proxy_headers_even_when_raw() {
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        ex.request
            .headers
            .insert("x-oproxy-session-id".to_string(), "sid".to_string());
        ex.request.headers.insert(
            "x-oproxy-destination".to_string(),
            "https://example.com".to_string(),
        );
        ex.request
            .headers
            .insert("proxy-connection".to_string(), "Keep-Alive".to_string());
        ex.request
            .headers
            .insert("x-custom".to_string(), "yes".to_string());
        let mut map = IndexMap::new();
        map.insert(ex.id.clone(), ex);

        let har = exchanges_to_har(&map);
        let json = serde_json::to_string(&har).unwrap();

        assert!(!json.contains("x-oproxy-session-id"));
        assert!(!json.contains("x-oproxy-destination"));
        assert!(!json.contains("proxy-connection"));
        assert!(json.contains("x-custom"));
    }

    #[test]
    fn redacted_har_masks_request_secret_reflected_in_response_text() {
        let ex = make_exchange(
            "POST",
            "https://api.example.com/login",
            200,
            r#"hello {"token":"secret-value"}"#,
            r#"{"token":"secret-value","name":"dev"}"#,
        );
        let mut map = IndexMap::new();
        map.insert(ex.id.clone(), ex);

        let har = exchanges_to_har_redacted(&map);
        let json = serde_json::to_string(&har).unwrap();

        assert!(json.contains(crate::redaction::REDACTED));
        assert!(!json.contains("secret-value"));
        assert!(json.contains("dev"));
    }

    #[test]
    fn har_entry_get_has_no_post_data() {
        let ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        let entry = exchange_to_har_entry(&ex);
        assert!(entry.request.post_data.is_none());
    }

    #[test]
    fn har_entry_timings_derived_from_metrics() {
        let ex = make_exchange("GET", "https://example.com/", 200, "body", "");
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.timings.wait, 80.0);
        assert_eq!(entry.timings.receive, 20.0);
        assert_eq!(entry.time, 100.0);
    }

    #[test]
    fn har_entry_with_waterfall_timing_fields() {
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        if let Some(m) = &mut ex.metrics {
            m.dns_ms = Some(10);
            m.tcp_connect_ms = Some(15);
            m.tls_ms = Some(25);
        }
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.timings.dns, Some(10.0));
        assert_eq!(entry.timings.connect, Some(15.0));
        assert_eq!(entry.timings.ssl, Some(25.0));
    }

    #[test]
    fn har_entry_preserves_oproxy_id() {
        let ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.oproxy_id.as_deref(), Some("test-id"));
    }

    #[test]
    fn har_entry_uses_captured_protocol_for_http_version() {
        // The negotiated upstream protocol is surfaced on export.
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        if let Some(m) = &mut ex.metrics {
            m.protocol = Some("HTTP/2".to_string());
        }
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.response.http_version, "HTTP/2");
    }

    #[test]
    fn har_import_roundtrips_http_version_into_protocol_metric() {
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        if let Some(m) = &mut ex.metrics {
            m.protocol = Some("HTTP/3".to_string());
        }
        let entry = exchange_to_har_entry(&ex);
        let restored = har_entry_to_exchange(&entry);
        assert_eq!(
            restored.metrics.and_then(|m| m.protocol).as_deref(),
            Some("HTTP/3")
        );
    }

    #[test]
    fn har_roundtrip_preserves_connection_stream_identity() {
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        ex.connection_id = Some("conn-h2-1".to_string());
        ex.stream_id = Some(7);
        ex.downstream_protocol = Some("HTTP/2".to_string());
        ex.protocol_context = Some(
            crate::core::forward::ProtocolContext::http(
                axum::http::Version::HTTP_2,
                "https",
                crate::core::forward::ApplicationProtocol::Grpc,
                crate::core::forward::BodyMode::StreamMessages,
            )
            .with_identity(ex.connection_id.clone(), ex.stream_id),
        );

        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.connection.as_deref(), Some("conn-h2-1"));
        assert_eq!(entry.oproxy_stream_id, Some(7));
        assert_eq!(entry.oproxy_downstream_protocol.as_deref(), Some("HTTP/2"));

        let restored = har_entry_to_exchange(&entry);
        assert_eq!(restored.connection_id.as_deref(), Some("conn-h2-1"));
        assert_eq!(restored.stream_id, Some(7));
        assert_eq!(restored.downstream_protocol.as_deref(), Some("HTTP/2"));
        assert_eq!(
            restored.protocol_context.as_ref().map(|ctx| ctx.downstream),
            Some(crate::core::forward::WireProtocol::Http2)
        );
    }

    #[test]
    fn har_entry_preserves_notes_and_tags() {
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        ex.note = Some("important".to_string());
        ex.tags = vec!["prod".to_string(), "auth".to_string()];
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.oproxy_note.as_deref(), Some("important"));
        assert_eq!(entry.oproxy_tags, vec!["prod", "auth"]);
    }

    #[test]
    fn har_entry_version_is_1_2() {
        let map = indexmap::IndexMap::new();
        let har = exchanges_to_har(&map);
        assert_eq!(har.log.version, "1.2");
        assert_eq!(har.log.creator.name, "oproxy");
    }

    // ── har_entry_to_exchange ─────────────────────────────────────────────────

    #[test]
    fn har_roundtrip_preserves_method_url_status() {
        let ex = make_exchange(
            "PUT",
            "https://api.example.com/item/1",
            200,
            "updated",
            r#"{"name":"x"}"#,
        );
        let entry = exchange_to_har_entry(&ex);
        let ex2 = har_entry_to_exchange(&entry);
        assert_eq!(ex2.request.method, "PUT");
        assert_eq!(ex2.request.uri, "https://api.example.com/item/1");
        assert_eq!(ex2.response.as_ref().unwrap().status, 200);
    }

    #[test]
    fn har_roundtrip_preserves_note_and_tags() {
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        ex.note = Some("test note".to_string());
        ex.tags = vec!["staging".to_string()];
        let entry = exchange_to_har_entry(&ex);
        let ex2 = har_entry_to_exchange(&entry);
        assert_eq!(ex2.note.as_deref(), Some("test note"));
        assert_eq!(ex2.tags, vec!["staging"]);
    }

    #[test]
    fn har_roundtrip_preserves_request_body() {
        let ex = make_exchange("POST", "https://example.com/", 201, "", r#"{"k":"v"}"#);
        let entry = exchange_to_har_entry(&ex);
        let ex2 = har_entry_to_exchange(&entry);
        assert!(ex2.request.body_text().contains("k"));
    }

    #[test]
    fn har_roundtrip_preserves_waterfall_timing() {
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        if let Some(m) = &mut ex.metrics {
            m.dns_ms = Some(5);
            m.tcp_connect_ms = Some(10);
            m.tls_ms = Some(20);
        }
        let entry = exchange_to_har_entry(&ex);
        let ex2 = har_entry_to_exchange(&entry);
        let m = ex2.metrics.unwrap();
        assert_eq!(m.dns_ms, Some(5));
        assert_eq!(m.tcp_connect_ms, Some(10));
        assert_eq!(m.tls_ms, Some(20));
    }

    #[test]
    fn har_roundtrip_preserves_full_ttfb_with_setup_timing() {
        let mut ex = make_exchange("GET", "https://example.com/", 200, "ok", "");
        if let Some(m) = &mut ex.metrics {
            m.ttfb_ms = 95;
            m.dns_ms = Some(5);
            m.tcp_connect_ms = Some(10);
            m.tls_ms = Some(20);
        }
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.timings.wait, 60.0);

        let ex2 = har_entry_to_exchange(&entry);
        let response = ex2.response.unwrap();
        let metrics = ex2.metrics.unwrap();
        assert_eq!(response.ttfb_ms, 95);
        assert_eq!(metrics.ttfb_ms, 95);
    }

    #[test]
    fn har_entry_without_response_has_status_zero() {
        let ex = make_exchange("GET", "https://example.com/", 0, "", "");
        let entry = exchange_to_har_entry(&ex);
        assert_eq!(entry.response.status, 0);
    }

    // ── parse_query_string ────────────────────────────────────────────────────

    #[test]
    fn parse_query_string_extracts_params() {
        let qs = parse_query_string("https://example.com/search?q=hello&page=2");
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].name, "q");
        assert_eq!(qs[0].value, "hello");
        assert_eq!(qs[1].name, "page");
        assert_eq!(qs[1].value, "2");
    }

    #[test]
    fn parse_query_string_empty_for_no_query() {
        assert!(parse_query_string("https://example.com/path").is_empty());
    }

    // ── har serde roundtrip ───────────────────────────────────────────────────

    #[test]
    fn har_serialises_and_deserialises() {
        let ex = make_exchange("GET", "https://example.com/", 200, "hello", "");
        let mut map = indexmap::IndexMap::new();
        map.insert(ex.id.clone(), ex);
        let har = exchanges_to_har(&map);
        let json = serde_json::to_string(&har).unwrap();
        let har2: Har = serde_json::from_str(&json).unwrap();
        assert_eq!(har2.log.entries.len(), 1);
        assert_eq!(har2.log.entries[0].request.method, "GET");
    }

    #[test]
    fn har_full_import_roundtrip() {
        let mut map = indexmap::IndexMap::new();
        for i in 0..3 {
            let mut ex = make_exchange("GET", &format!("https://example.com/{}", i), 200, "ok", "");
            ex.id = format!("id-{}", i);
            map.insert(ex.id.clone(), ex);
        }
        let har = exchanges_to_har(&map);
        let imported = har_to_exchanges(&har);
        assert_eq!(imported.len(), 3);
    }
}
