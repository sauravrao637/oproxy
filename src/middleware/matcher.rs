//! Shared, Charles-style request/response location matcher.
//!
//! Every rule type (rewrite, map-local, map-remote, block/allow list) matches
//! traffic through a single [`Location`]. This replaces the former per-plugin
//! ad-hoc matching where "host" meant substring, "path" meant regex, and
//! "header" meant contains — four different meanings of the word *match*.
//!
//! A `Location` is a set of optional conditions over the parts of a request:
//! protocol, host, port, path, query, and method. Every field is optional;
//! an unset (`None`/empty) field matches anything. Set fields are **ANDed**
//! together, so `host = "api.example.com"` + `path = "/v2/*"` matches only
//! requests that satisfy both — the multi-criteria matching the old single
//! `MatchCriteria` enum could not express.
//!
//! String fields (`host`, `path`, `query`) are interpreted according to
//! [`MatchMode`]: glob (`*` = any run, `?` = single char, anchored/full-string)
//! by default, or regex (unanchored `is_match`) when opted in.

// TODO(rules-phase2): remove once the unified rule engine consumes this module.
// Until the rewrite/map-local/map-remote/block-list rules are wired into the
// middleware chain, these items have no non-test caller in the binary target.
#![allow(dead_code)]

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;

/// How the freeform string fields (`host`, `path`, `query`) are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// Wildcard matching: `*` matches any run of characters, `?` matches a
    /// single character. Anchored — the pattern must match the whole value.
    #[default]
    Glob,
    /// Regular-expression matching via the `regex` crate. Unanchored, i.e. a
    /// substring match unless the pattern uses `^`/`$`.
    Regex,
}

thread_local! {
    /// Per-thread cache of compiled regexes keyed by pattern string.
    /// Avoids recompiling the same pattern on every request in Regex match mode.
    static REGEX_CACHE: RefCell<HashMap<String, Option<Regex>>> =
        RefCell::new(HashMap::new());
}

/// A Charles-style location condition. Unset fields match anything; set fields
/// are ANDed together.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Location {
    /// `"http"` / `"https"`. Matched case-insensitively, exact. `None` = any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Hostname without port. Matched via [`MatchMode`]. Empty/`None` = any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Exact port. `None` = any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Path without query string. Matched via [`MatchMode`]. Empty/`None` = any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Raw query string (without leading `?`). Matched via [`MatchMode`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Allowed methods, case-insensitive. Empty = any method.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    /// Interpretation of `host` / `path` / `query`.
    #[serde(default)]
    pub mode: MatchMode,
}

/// The neutral, parsed view of a request (or a response's originating request)
/// that a [`Location`] is matched against. Constructed via [`MatchTarget::from_request`].
#[derive(Debug, Clone, Default)]
pub struct MatchTarget {
    pub protocol: Option<String>,
    pub method: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
    pub query: String,
}

impl Location {
    /// Returns `true` if every set condition matches `target`.
    pub fn matches(&self, target: &MatchTarget) -> bool {
        if let Some(proto) = self.protocol.as_deref().filter(|p| !p.is_empty()) {
            match &target.protocol {
                Some(actual) if actual.eq_ignore_ascii_case(proto) => {}
                _ => return false,
            }
        }
        if let Some(port) = self.port
            && Some(port) != target.port
        {
            return false;
        }
        if !self.methods.is_empty()
            && !self
                .methods
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&target.method))
        {
            return false;
        }
        if let Some(host) = self.host.as_deref().filter(|h| !h.is_empty())
            && !self.match_str(host, &target.host)
        {
            return false;
        }
        if let Some(path) = self.path.as_deref().filter(|p| !p.is_empty())
            && !self.match_str(path, &target.path)
        {
            return false;
        }
        if let Some(query) = self.query.as_deref().filter(|q| !q.is_empty())
            && !self.match_str(query, &target.query)
        {
            return false;
        }
        true
    }

    /// Returns the compiled path regex when mode is Regex, using the
    /// thread-local cache. Used by mock middleware for capture-group
    /// template substitution after a successful location match.
    /// Regex is Clone (Arc-backed), so this is cheap.
    pub fn compiled_path_regex(&self) -> Option<Regex> {
        if self.mode != MatchMode::Regex {
            return None;
        }
        let path = self.path.as_deref().filter(|p| !p.is_empty())?;
        REGEX_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .entry(path.to_string())
                .or_insert_with(|| Regex::new(path).ok())
                .clone()
        })
    }

    fn match_str(&self, pattern: &str, value: &str) -> bool {
        match self.mode {
            MatchMode::Glob => glob_match(pattern, value),
            // Use thread-local cache to avoid recompiling the same pattern
            // on every request. Invalid patterns are stored as None and
            // never match (preserves the original fail-safe behaviour).
            MatchMode::Regex => REGEX_CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                let re = cache
                    .entry(pattern.to_string())
                    .or_insert_with(|| Regex::new(pattern).ok());
                re.as_ref().map(|r| r.is_match(value)).unwrap_or(false)
            }),
        }
    }
}

impl MatchTarget {
    /// Build a target from a proxied request. Parses host/port from
    /// [`RequestContext::host`] and path/query from
    /// [`RequestContext::uri`] (which may be origin-form `/p?q` or
    /// absolute-form `http://h/p?q`). Protocol is derived from an absolute URI
    /// or an explicit `destination` override when available.
    pub fn from_request(ctx: &crate::middleware::RequestContext) -> Self {
        let (protocol, uri_host, path, query) = parse_uri(&ctx.uri);
        // Prefer host parsed from an absolute URI; otherwise the Host header.
        let host_authority = uri_host.unwrap_or_else(|| ctx.host.clone());
        let (host, port) = split_host_port(&host_authority);
        let protocol = protocol.or_else(|| ctx.destination.as_deref().and_then(|d| parse_uri(d).0));
        MatchTarget {
            protocol,
            method: ctx.method.clone(),
            host,
            port,
            path,
            query,
        }
    }

    /// Build a target for matching against a response. Uses the originating
    /// request metadata injected by the engine into [`ResponseContext`]:
    /// `request_host` (authority) and `request_method`. Path/query are parsed
    /// from `request_uri`. Protocol is not reliably available on responses and
    /// is left as `None` (a Location with `protocol` set will not match).
    pub fn from_response(ctx: &crate::middleware::ResponseContext) -> Self {
        let (_, _, path, query) = parse_uri(&ctx.request_uri);
        let (host, port) = split_host_port(&ctx.request_host);
        MatchTarget {
            protocol: None,
            method: ctx.request_method.clone(),
            host,
            port,
            path,
            query,
        }
    }
}

/// Split an authority into `(host, Some(port))`, tolerating IPv6 brackets and
/// authorities with no port. `"localhost:8080"` → `("localhost", Some(8080))`,
/// `"example.com"` → `("example.com", None)`, `"[::1]:443"` → `("[::1]", Some(443))`.
fn split_host_port(authority: &str) -> (String, Option<u16>) {
    if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 literal: [addr] or [addr]:port
        if let Some((addr, after)) = rest.split_once(']') {
            let port = after.strip_prefix(':').and_then(|p| p.parse::<u16>().ok());
            return (format!("[{addr}]"), port);
        }
        return (authority.to_string(), None);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => {
            (host.to_string(), port.parse::<u16>().ok())
        }
        _ => (authority.to_string(), None),
    }
}

/// Decompose a request URI into `(protocol, host_authority, path, query)`.
/// Handles both origin-form (`/path?q`) and absolute-form (`http://h/path?q`).
fn parse_uri(uri: &str) -> (Option<String>, Option<String>, String, String) {
    // Absolute form: scheme://authority/path?query
    if let Some(scheme_end) = uri.find("://") {
        let scheme = uri[..scheme_end].to_string();
        let rest = &uri[scheme_end + 3..];
        let (authority, path_query) = match rest.find('/') {
            Some(i) => (rest[..i].to_string(), &rest[i..]),
            None => (rest.to_string(), "/"),
        };
        let (path, query) = split_path_query(path_query);
        return (Some(scheme), Some(authority), path, query);
    }
    let (path, query) = split_path_query(uri);
    (None, None, path, query)
}

fn split_path_query(path_query: &str) -> (String, String) {
    match path_query.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (path_query.to_string(), String::new()),
    }
}

/// Anchored wildcard match. `*` matches any run (including empty); `?` matches
/// exactly one character. Comparison is over Unicode scalar values.
fn glob_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    // Iterative backtracking matcher (linear-ish, no recursion blowup on `*`).
    let (mut pi, mut vi) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while vi < v.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = vi;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            vi = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::RequestContext;
    use bytes::Bytes;

    // ── glob_match ──────────────────────────────────────────────────────────

    #[test]
    fn glob_exact_and_wildcards() {
        assert!(glob_match("/api/users", "/api/users"));
        assert!(!glob_match("/api/users", "/api/orders"));
        assert!(glob_match("/api/*", "/api/users"));
        assert!(glob_match("/api/*", "/api/"));
        assert!(glob_match("*.example.com", "api.example.com"));
        assert!(!glob_match("*.example.com", "example.com"));
        assert!(glob_match("/v?/users", "/v2/users"));
        assert!(!glob_match("/v?/users", "/v22/users"));
        assert!(glob_match("*", "anything at all"));
        assert!(glob_match("/a/*/c", "/a/b/c"));
        assert!(glob_match("/a/*/c", "/a/b/b/c"));
        assert!(!glob_match("/a/*/c", "/a/b/c/d"));
    }

    #[test]
    fn glob_anchored_full_string() {
        // No implicit substring matching for globs.
        assert!(!glob_match("api", "api.example.com"));
        assert!(glob_match("api*", "api.example.com"));
    }

    // ── host/port parsing ───────────────────────────────────────────────────

    #[test]
    fn split_host_port_variants() {
        assert_eq!(
            split_host_port("localhost:8080"),
            ("localhost".into(), Some(8080))
        );
        assert_eq!(split_host_port("example.com"), ("example.com".into(), None));
        assert_eq!(split_host_port("[::1]:443"), ("[::1]".into(), Some(443)));
        assert_eq!(split_host_port("[::1]"), ("[::1]".into(), None));
        // A trailing non-numeric ':' segment is not a port.
        assert_eq!(split_host_port("host:abc"), ("host:abc".into(), None));
    }

    #[test]
    fn parse_uri_origin_and_absolute() {
        assert_eq!(
            parse_uri("/api/users?a=1&b=2"),
            (None, None, "/api/users".into(), "a=1&b=2".into())
        );
        assert_eq!(
            parse_uri("http://api.example.com:8080/v2/users?x=1"),
            (
                Some("http".into()),
                Some("api.example.com:8080".into()),
                "/v2/users".into(),
                "x=1".into()
            )
        );
        assert_eq!(
            parse_uri("https://example.com"),
            (
                Some("https".into()),
                Some("example.com".into()),
                "/".into(),
                String::new()
            )
        );
    }

    // ── Location::matches ─────────────────────────────────────────────────────

    fn target(method: &str, host: &str, port: Option<u16>, path: &str, query: &str) -> MatchTarget {
        MatchTarget {
            protocol: Some("https".into()),
            method: method.into(),
            host: host.into(),
            port,
            path: path.into(),
            query: query.into(),
        }
    }

    #[test]
    fn empty_location_matches_everything() {
        let loc = Location::default();
        assert!(loc.matches(&target("GET", "any.host", Some(443), "/x", "")));
    }

    #[test]
    fn fields_are_anded() {
        let loc = Location {
            host: Some("api.example.com".into()),
            path: Some("/v2/*".into()),
            ..Default::default()
        };
        assert!(loc.matches(&target("GET", "api.example.com", None, "/v2/users", "")));
        // host matches but path does not
        assert!(!loc.matches(&target("GET", "api.example.com", None, "/v1/users", "")));
        // path matches but host does not
        assert!(!loc.matches(&target("GET", "other.com", None, "/v2/users", "")));
    }

    #[test]
    fn method_membership_case_insensitive() {
        let loc = Location {
            methods: vec!["POST".into(), "put".into()],
            ..Default::default()
        };
        assert!(loc.matches(&target("post", "h", None, "/", "")));
        assert!(loc.matches(&target("PUT", "h", None, "/", "")));
        assert!(!loc.matches(&target("GET", "h", None, "/", "")));
    }

    #[test]
    fn port_must_match_exactly_when_set() {
        let loc = Location {
            port: Some(8080),
            ..Default::default()
        };
        assert!(loc.matches(&target("GET", "h", Some(8080), "/", "")));
        assert!(!loc.matches(&target("GET", "h", Some(443), "/", "")));
        assert!(!loc.matches(&target("GET", "h", None, "/", "")));
    }

    #[test]
    fn protocol_match_case_insensitive() {
        let loc = Location {
            protocol: Some("HTTPS".into()),
            ..Default::default()
        };
        assert!(loc.matches(&target("GET", "h", None, "/", "")));
        let mut t = target("GET", "h", None, "/", "");
        t.protocol = Some("http".into());
        assert!(!loc.matches(&t));
    }

    #[test]
    fn regex_mode_for_path() {
        let loc = Location {
            path: Some(r"^/api/v\d+/".into()),
            mode: MatchMode::Regex,
            ..Default::default()
        };
        assert!(loc.matches(&target("GET", "h", None, "/api/v2/users", "")));
        assert!(!loc.matches(&target("GET", "h", None, "/web/v2/users", "")));
    }

    #[test]
    fn invalid_regex_never_matches() {
        let loc = Location {
            path: Some("[unterminated".into()),
            mode: MatchMode::Regex,
            ..Default::default()
        };
        assert!(!loc.matches(&target("GET", "h", None, "/anything", "")));
    }

    #[test]
    fn query_matching() {
        let loc = Location {
            query: Some("*debug=true*".into()),
            ..Default::default()
        };
        assert!(loc.matches(&target("GET", "h", None, "/", "x=1&debug=true&y=2")));
        assert!(!loc.matches(&target("GET", "h", None, "/", "x=1&y=2")));
    }

    // ── MatchTarget::from_request ─────────────────────────────────────────────

    fn req(method: &str, host: &str, uri: &str) -> RequestContext {
        RequestContext {
            method: method.into(),
            uri: uri.into(),
            host: host.into(),
            headers: crate::middleware::HeaderMap::new(),
            body: Bytes::new(),
            ..Default::default()
        }
    }

    #[test]
    fn from_request_origin_form_uses_host_header() {
        let t = MatchTarget::from_request(&req("GET", "api.example.com:8080", "/v2/users?x=1"));
        assert_eq!(t.method, "GET");
        assert_eq!(t.host, "api.example.com");
        assert_eq!(t.port, Some(8080));
        assert_eq!(t.path, "/v2/users");
        assert_eq!(t.query, "x=1");
        assert_eq!(t.protocol, None);
    }

    #[test]
    fn from_request_absolute_form_prefers_uri_authority() {
        let t = MatchTarget::from_request(&req(
            "POST",
            "ignored.host",
            "https://real.example.com/path?q=1",
        ));
        assert_eq!(t.protocol.as_deref(), Some("https"));
        assert_eq!(t.host, "real.example.com");
        assert_eq!(t.path, "/path");
        assert_eq!(t.query, "q=1");
    }

    #[test]
    fn from_request_derives_protocol_from_destination() {
        let mut r = req("GET", "h.local", "/x");
        r.destination = Some("https://upstream.internal:9000".into());
        let t = MatchTarget::from_request(&r);
        assert_eq!(t.protocol.as_deref(), Some("https"));
        // host/port still come from the request authority, not the destination.
        assert_eq!(t.host, "h.local");
    }

    #[test]
    fn end_to_end_location_against_request() {
        let loc = Location {
            host: Some("*.example.com".into()),
            path: Some("/api/*".into()),
            methods: vec!["GET".into()],
            ..Default::default()
        };
        let t = MatchTarget::from_request(&req("GET", "api.example.com", "/api/users"));
        assert!(loc.matches(&t));
        let t2 = MatchTarget::from_request(&req("GET", "example.com", "/api/users"));
        assert!(
            !loc.matches(&t2),
            "bare apex should not match *.example.com"
        );
    }
}
