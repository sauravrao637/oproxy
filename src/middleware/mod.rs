use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;

use crate::core::engine::is_binary_content_type;

/// An ordered, duplicate-preserving, case-insensitive collection of HTTP headers.
///
/// HTTP allows several headers with the same field name (e.g. multiple
/// `Set-Cookie`), but a `HashMap<String, String>` silently collapses them to a
/// single value. `HeaderMap` keeps every entry in insertion order so that the
/// engine can forward all of them to the client/upstream intact.
///
/// Field names are normalised to lowercase on insert and all lookups are
/// case-insensitive, matching HTTP's case-insensitive header semantics. The
/// API mirrors the subset of `HashMap` used across the codebase
/// (`get`/`insert`/`remove`/`contains_key`/`iter`/…) so it is a near drop-in,
/// with [`append`](HeaderMap::append)/[`get_all`](HeaderMap::get_all) added for
/// the multi-value cases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderMap {
    entries: Vec<(String, String)>,
}

impl HeaderMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// First value for `key` (case-insensitive), if any.
    pub fn get(&self, key: &str) -> Option<&String> {
        let k = key.to_ascii_lowercase();
        self.entries.iter().find(|(n, _)| *n == k).map(|(_, v)| v)
    }

    /// All values for `key` (case-insensitive), in insertion order.
    pub fn get_all<'a>(&'a self, key: &str) -> impl Iterator<Item = &'a String> + 'a {
        let k = key.to_ascii_lowercase();
        self.entries
            .iter()
            .filter(move |(n, _)| *n == k)
            .map(|(_, v)| v)
    }

    /// Replace every existing value for `key` with a single `value`
    /// (HashMap-like semantics). Returns the previous first value, if any.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        let k = key.into().to_ascii_lowercase();
        let prev = self.remove(&k);
        self.entries.push((k, value.into()));
        prev
    }

    /// Append a value for `key` without disturbing existing entries — this is
    /// what preserves multi-valued headers such as `Set-Cookie`.
    pub fn append(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries
            .push((key.into().to_ascii_lowercase(), value.into()));
    }

    /// Remove all entries for `key`, returning the first removed value.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        let k = key.to_ascii_lowercase();
        let mut removed = None;
        self.entries.retain(|(n, v)| {
            if *n == k {
                if removed.is_none() {
                    removed = Some(v.clone());
                }
                false
            } else {
                true
            }
        });
        removed
    }

    pub fn contains_key(&self, key: &str) -> bool {
        let k = key.to_ascii_lowercase();
        self.entries.iter().any(|(n, _)| *n == k)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.entries.iter().map(|(n, v)| (n, v))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Retain only entries for which `f(name, value)` returns true.
    pub fn retain(&mut self, mut f: impl FnMut(&str, &str) -> bool) {
        self.entries.retain(|(n, v)| f(n, v));
    }
}

impl<'a> IntoIterator for &'a HeaderMap {
    type Item = (&'a String, &'a String);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (String, String)>,
        fn(&'a (String, String)) -> (&'a String, &'a String),
    >;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|(n, v)| (n, v))
    }
}

impl IntoIterator for HeaderMap {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<(String, String)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl FromIterator<(String, String)> for HeaderMap {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
        let mut map = HeaderMap::new();
        for (k, v) in iter {
            map.append(k, v);
        }
        map
    }
}

impl Extend<(String, String)> for HeaderMap {
    fn extend<T: IntoIterator<Item = (String, String)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.append(k, v);
        }
    }
}

impl From<HashMap<String, String>> for HeaderMap {
    fn from(map: HashMap<String, String>) -> Self {
        map.into_iter().collect()
    }
}

impl From<HeaderMap> for HashMap<String, String> {
    /// Collapses duplicate field names (last value wins). Used at the
    /// serialization/export boundary where a JSON object cannot hold duplicates.
    fn from(headers: HeaderMap) -> Self {
        headers.entries.into_iter().collect()
    }
}

impl Serialize for HeaderMap {
    /// Serialized as a JSON object for backward compatibility with stored
    /// sessions/exports. A JSON object cannot hold duplicate keys, so duplicate
    /// field names collapse (last value wins) — multi-value preservation lives in
    /// the live proxy path, not the persisted representation.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let map: HashMap<&str, &str> = self
            .entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        serializer.collect_map(map)
    }
}

impl<'de> Deserialize<'de> for HeaderMap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(HashMap::<String, String>::deserialize(deserializer)?.into())
    }
}

fn content_type_of(headers: &HeaderMap) -> String {
    headers.get("content-type").cloned().unwrap_or_default()
}

/// A response a middleware wants the engine to return immediately instead of
/// forwarding upstream (mock, map-local, Lua `abort()`, breakpoint timeout, …).
///
/// This is the typed replacement for the old `x-oproxy-mock-response` header
/// protocol: the body is carried as raw [`Bytes`] so binary payloads survive
/// without a base64 round-trip, and nothing leaks into the forwarded headers.
#[derive(Debug, Clone)]
pub struct InterceptedResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
    /// Session tags to attach when this response is recorded (e.g. "mock").
    pub tags: Vec<String>,
}

/// A captured request.
///
/// `body` is the single source of truth, stored as raw [`Bytes`]. There is no
/// separate string copy to keep in sync — text-oriented middleware read a lossy
/// view via [`RequestContext::body_text`] and write through
/// [`RequestContext::set_body_text`], both of which operate on the same bytes.
///
/// On the wire the `body` field serialises as a string (lossy UTF-8) for
/// compatibility with the UI, HAR export, and saved sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(into = "RequestContextWire", from = "RequestContextWire")]
pub struct RequestContext {
    pub method: String,
    pub uri: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub host: String,
    // ── Internal middleware ↔ engine side-channel ───────────────────────────────
    // The fields below replace the former `x-oproxy-*` pseudo-header protocol.
    // They are in-memory only (never serialised) so they can never leak to the
    // upstream server or into recordings/exports.
    /// Upstream target override (Routing / DNS override / MITM). When set the
    /// engine forwards here instead of the request's original host.
    pub destination: Option<String>,
    /// Session id assigned by InspectionMiddleware, used to correlate the
    /// response back to the exact request even under concurrent same-URI traffic.
    pub session_id: Option<String>,
    /// Set by CaptureFilterMiddleware to suppress session recording for this host.
    pub skip_recording: bool,
    /// Short-circuit response set by Mock / map-local / Lua / breakpoint timeout.
    /// When present the engine returns it instead of forwarding upstream.
    pub mock_response: Option<InterceptedResponse>,
    /// Parsed inspector data (JWT / GraphQL / gRPC) populated by the inspector
    /// middlewares and consumed by InspectionMiddleware.
    pub inspector: crate::session::InspectorData,
}

impl RequestContext {
    /// Lossy UTF-8 view of the body for text-oriented inspection/modification.
    pub fn body_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Replace the body from a text value (the single source of truth).
    pub fn set_body_text(&mut self, text: impl Into<String>) {
        self.body = Bytes::from(text.into());
    }
}

/// A captured response. See [`RequestContext`] — `body` is the single source of
/// truth as raw [`Bytes`].
///
/// On the wire the `body` field serialises as a string: base64 for binary
/// content-types (so the UI can render images and binary survives export/replay)
/// and lossy UTF-8 otherwise.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(into = "ResponseContextWire", from = "ResponseContextWire")]
pub struct ResponseContext {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub request_uri: String,
    // Injected by InspectionMiddleware during on_request; used in on_response for exact
    // session lookup so concurrent requests to the same URI don't overwrite each other.
    pub session_id: Option<String>,
    // Time from request send to response headers received (DNS+TCP+TLS+TTFB).
    pub ttfb_ms: u64,
    // Time to read response body after headers received.
    pub body_ms: u64,
    /// Session tags to attach when this exchange is recorded. Typed replacement
    /// for the former `x-oproxy-tags` response header. Not serialised.
    pub tags: Vec<String>,
    /// Host authority of the originating request (e.g. `api.example.com:8080`).
    /// Populated by the engine; not serialised. Used by rule-matching middleware
    /// so a Location's host/port conditions can be evaluated on responses.
    #[serde(skip)]
    pub request_host: String,
    /// HTTP method of the originating request. Populated by the engine; not serialised.
    #[serde(skip)]
    pub request_method: String,
}

impl ResponseContext {
    /// Lossy UTF-8 view of the body for text-oriented inspection/modification.
    pub fn body_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Replace the body from a text value (the single source of truth).
    pub fn set_body_text(&mut self, text: impl Into<String>) {
        self.body = Bytes::from(text.into());
    }
}

// ── Wire representations ─────────────────────────────────────────────────────
// Only the persisted/observable fields appear here; the in-memory side-channel
// fields are reconstructed via `Default` on deserialize. The `body` field is a
// string on the wire (see the doc comments on the context types).

#[derive(Serialize, Deserialize)]
struct RequestContextWire {
    method: String,
    uri: String,
    headers: HashMap<String, String>,
    body: String,
    host: String,
}

impl From<RequestContext> for RequestContextWire {
    fn from(ctx: RequestContext) -> Self {
        RequestContextWire {
            method: ctx.method,
            uri: ctx.uri,
            body: String::from_utf8_lossy(&ctx.body).into_owned(),
            host: ctx.host,
            headers: ctx.headers.into(),
        }
    }
}

impl From<RequestContextWire> for RequestContext {
    fn from(wire: RequestContextWire) -> Self {
        RequestContext {
            method: wire.method,
            uri: wire.uri,
            headers: wire.headers.into(),
            body: Bytes::from(wire.body.into_bytes()),
            host: wire.host,
            ..Default::default()
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ResponseContextWire {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
    request_uri: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    ttfb_ms: u64,
    #[serde(default)]
    body_ms: u64,
}

impl From<ResponseContext> for ResponseContextWire {
    fn from(ctx: ResponseContext) -> Self {
        let body = if is_binary_content_type(&content_type_of(&ctx.headers)) {
            base64::engine::general_purpose::STANDARD.encode(&ctx.body)
        } else {
            String::from_utf8_lossy(&ctx.body).into_owned()
        };
        ResponseContextWire {
            status: ctx.status,
            headers: ctx.headers.into(),
            body,
            request_uri: ctx.request_uri,
            session_id: ctx.session_id,
            ttfb_ms: ctx.ttfb_ms,
            body_ms: ctx.body_ms,
        }
    }
}

impl From<ResponseContextWire> for ResponseContext {
    fn from(wire: ResponseContextWire) -> Self {
        let headers: HeaderMap = wire.headers.into();
        let body = if is_binary_content_type(&content_type_of(&headers)) {
            base64::engine::general_purpose::STANDARD
                .decode(wire.body.as_bytes())
                .map(Bytes::from)
                .unwrap_or_else(|_| Bytes::from(wire.body.into_bytes()))
        } else {
            Bytes::from(wire.body.into_bytes())
        };
        ResponseContext {
            status: wire.status,
            headers,
            body,
            request_uri: wire.request_uri,
            session_id: wire.session_id,
            ttfb_ms: wire.ttfb_ms,
            body_ms: wire.body_ms,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareAction {
    Continue,      // Proceed to next middleware
    StopAndReturn, // Stop chain and return current response (e.g., Map Local)
    #[allow(dead_code)]
    Pause, // Halt execution (e.g., Breakpoint)
}

#[async_trait]
pub trait Middleware: Send + Sync {
    fn name(&self) -> &str;

    /// Process the request before it is sent to the target server.
    async fn on_request(&self, _ctx: &mut RequestContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }

    /// Process the response before it is sent back to the client.
    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }
}

// ── Unified Context Helpers ─────────────────────────────────────────────────

pub fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).cloned()
}

pub fn set_header(headers: &mut HeaderMap, name: &str, value: String) {
    headers.insert(name, value);
}

pub fn append_header(headers: &mut HeaderMap, name: &str, value: &str) {
    let joined = match headers.get(name) {
        Some(existing) if !existing.is_empty() => format!("{existing}, {value}"),
        _ => value.to_string(),
    };
    headers.insert(name, joined);
}

pub fn remove_header(headers: &mut HeaderMap, name: &str) {
    headers.remove(name);
}

pub fn path_of(uri: &str) -> &str {
    let s = uri.split('?').next().unwrap_or(uri);
    // Strip absolute-form scheme://host prefix if present.
    if let Some(idx) = s.find("://") {
        let after_scheme = &s[idx + 3..];
        return after_scheme
            .find('/')
            .map(|i| &after_scheme[i..])
            .unwrap_or("/");
    }
    s
}

pub fn split_path_query(uri: &str) -> (String, String) {
    match uri.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (uri.to_string(), String::new()),
    }
}

/// Parse `key=value` pairs from a raw query string.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (p.to_string(), String::new()),
        })
        .collect()
}

pub fn build_query(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                k.clone()
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

pub fn set_query_param(uri: &str, name: &str, value: &str) -> String {
    let (path, query) = split_path_query(uri);
    let mut pairs = parse_query(&query);
    if let Some(pos) = pairs.iter().position(|(k, _)| k == name) {
        pairs[pos].1 = value.to_string();
    } else {
        pairs.push((name.to_string(), value.to_string()));
    }
    let new_q = build_query(&pairs);
    if new_q.is_empty() {
        path
    } else {
        format!("{path}?{new_q}")
    }
}

pub fn remove_query_param(uri: &str, name: &str) -> String {
    let (path, query) = split_path_query(uri);
    let pairs: Vec<_> = parse_query(&query)
        .into_iter()
        .filter(|(k, _)| k != name)
        .collect();
    let new_q = build_query(&pairs);
    if new_q.is_empty() {
        path
    } else {
        format!("{path}?{new_q}")
    }
}

pub mod chain;
pub mod matcher;
pub mod plugins;
