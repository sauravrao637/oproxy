use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

use crate::session::Exchange;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDiff {
    pub request_diff: ContextDiff,
    pub response_diff: Option<ResponseDiff>,
    pub timing_delta: TimingDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextDiff {
    pub method_changed: bool,
    pub uri_diff: Option<LineDiff>,
    pub headers_added: Vec<String>,
    pub headers_removed: Vec<String>,
    pub headers_changed: Vec<HeaderChange>,
    pub body_diff: Option<LineDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseDiff {
    pub status_changed: Option<(u16, u16)>,
    pub headers_added: Vec<String>,
    pub headers_removed: Vec<String>,
    pub headers_changed: Vec<HeaderChange>,
    pub body_diff: Option<LineDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderChange {
    pub name: String,
    pub old_value: String,
    pub new_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingDelta {
    pub latency_delta_ms: i64,
    pub ttfb_delta_ms: i64,
    pub size_delta_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineDiff {
    pub hunks: Vec<String>,
}

fn unified_diff(old: &str, new: &str) -> Option<LineDiff> {
    if old == new {
        return None;
    }
    let diff = TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(3) {
        let mut hunk = String::new();
        for op in group {
            for change in diff.iter_changes(&op) {
                let prefix = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                hunk.push_str(prefix);
                hunk.push_str(change.value());
                if !change.value().ends_with('\n') {
                    hunk.push('\n');
                }
            }
        }
        if !hunk.is_empty() {
            hunks.push(hunk);
        }
    }
    if hunks.is_empty() {
        None
    } else {
        Some(LineDiff { hunks })
    }
}

fn diff_headers(
    a: &std::collections::HashMap<String, String>,
    b: &std::collections::HashMap<String, String>,
) -> (Vec<String>, Vec<String>, Vec<HeaderChange>) {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    for (k, bv) in b {
        match a.get(k) {
            None => added.push(format!("{}: {}", k, bv)),
            Some(av) if av != bv => changed.push(HeaderChange {
                name: k.clone(),
                old_value: av.clone(),
                new_value: bv.clone(),
            }),
            _ => {}
        }
    }
    for k in a.keys() {
        if !b.contains_key(k) {
            removed.push(format!("{}: {}", k, a[k]));
        }
    }
    (added, removed, changed)
}

pub fn diff_exchanges(a: &Exchange, b: &Exchange) -> SessionDiff {
    let req_a = &a.request;
    let req_b = &b.request;

    let method_changed = req_a.method != req_b.method;
    let uri_diff = unified_diff(&req_a.uri, &req_b.uri);
    let (headers_added, headers_removed, headers_changed) =
        diff_headers(&req_a.headers, &req_b.headers);
    let body_diff = unified_diff(&req_a.body, &req_b.body);

    let request_diff = ContextDiff {
        method_changed,
        uri_diff,
        headers_added,
        headers_removed,
        headers_changed,
        body_diff,
    };

    let response_diff = match (&a.response, &b.response) {
        (Some(ra), Some(rb)) => {
            let status_changed = if ra.status != rb.status {
                Some((ra.status, rb.status))
            } else {
                None
            };
            let (headers_added, headers_removed, headers_changed) =
                diff_headers(&ra.headers, &rb.headers);
            let body_diff = unified_diff(&ra.body, &rb.body);
            Some(ResponseDiff {
                status_changed,
                headers_added,
                headers_removed,
                headers_changed,
                body_diff,
            })
        }
        _ => None,
    };

    let ma = a.metrics.as_ref();
    let mb = b.metrics.as_ref();
    let timing_delta = TimingDelta {
        latency_delta_ms: mb.map(|m| m.latency_ms as i64).unwrap_or(0)
            - ma.map(|m| m.latency_ms as i64).unwrap_or(0),
        ttfb_delta_ms: mb.map(|m| m.ttfb_ms as i64).unwrap_or(0)
            - ma.map(|m| m.ttfb_ms as i64).unwrap_or(0),
        size_delta_bytes: mb.map(|m| m.response_size_bytes as i64).unwrap_or(0)
            - ma.map(|m| m.response_size_bytes as i64).unwrap_or(0),
    };

    SessionDiff {
        request_diff,
        response_diff,
        timing_delta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{RequestContext, ResponseContext};
    use crate::session::{Exchange, InspectionMetrics};
    use chrono::Utc;
    use std::collections::HashMap;

    fn make_exchange(id: &str, method: &str, uri: &str, body: &str, status: u16, resp_body: &str) -> Exchange {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        let mut resp_headers = HashMap::new();
        resp_headers.insert("content-type".to_string(), "application/json".to_string());
        Exchange {
            id: id.to_string(),
            timestamp: Utc::now(),
            updated_at: None,
            request: RequestContext {
                method: method.to_string(),
                uri: uri.to_string(),
                headers,
                body: body.to_string(),
                host: "example.com".to_string(),
                body_bytes: None,
            },
            response: Some(ResponseContext {
                request_uri: uri.to_string(),
                status,
                headers: resp_headers,
                body: resp_body.to_string(),
                session_id: None,
                ttfb_ms: 10,
                body_ms: 5,
                body_bytes: None,
            }),
            metrics: Some(InspectionMetrics {
                latency_ms: 100,
                ttfb_ms: 50,
                request_size_bytes: body.len(),
                response_size_bytes: resp_body.len(),
                status_code: status,
                ..Default::default()
            }),
            ws_frames: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
        }
    }

    #[test]
    fn identical_sessions_produce_empty_diff() {
        let a = make_exchange("a", "GET", "/api/v1", "", 200, "ok");
        let b = make_exchange("b", "GET", "/api/v1", "", 200, "ok");
        let diff = diff_exchanges(&a, &b);
        assert!(!diff.request_diff.method_changed);
        assert!(diff.request_diff.uri_diff.is_none());
        assert!(diff.request_diff.body_diff.is_none());
        assert!(diff.request_diff.headers_added.is_empty());
        assert!(diff.request_diff.headers_removed.is_empty());
        assert!(diff.request_diff.headers_changed.is_empty());
        let rd = diff.response_diff.unwrap();
        assert!(rd.status_changed.is_none());
        assert!(rd.body_diff.is_none());
        assert_eq!(diff.timing_delta.latency_delta_ms, 0);
    }

    #[test]
    fn method_change_detected() {
        let a = make_exchange("a", "GET", "/api", "", 200, "");
        let b = make_exchange("b", "POST", "/api", "", 200, "");
        let diff = diff_exchanges(&a, &b);
        assert!(diff.request_diff.method_changed);
    }

    #[test]
    fn status_change_detected() {
        let a = make_exchange("a", "GET", "/api", "", 200, "");
        let b = make_exchange("b", "GET", "/api", "", 404, "not found");
        let diff = diff_exchanges(&a, &b);
        let rd = diff.response_diff.unwrap();
        assert_eq!(rd.status_changed, Some((200, 404)));
    }

    #[test]
    fn body_diff_has_hunks_when_different() {
        let a = make_exchange("a", "GET", "/api", "", 200, "hello world");
        let b = make_exchange("b", "GET", "/api", "", 200, "hello rust");
        let diff = diff_exchanges(&a, &b);
        let rd = diff.response_diff.unwrap();
        assert!(rd.body_diff.is_some());
        let hunks = &rd.body_diff.unwrap().hunks;
        assert!(!hunks.is_empty());
        assert!(hunks[0].contains("-hello world") || hunks[0].contains("+hello rust"));
    }

    #[test]
    fn request_body_diff_detected() {
        let a = make_exchange("a", "POST", "/api", r#"{"x":1}"#, 200, "");
        let b = make_exchange("b", "POST", "/api", r#"{"x":2}"#, 200, "");
        let diff = diff_exchanges(&a, &b);
        assert!(diff.request_diff.body_diff.is_some());
    }

    #[test]
    fn header_added_detected() {
        let a = make_exchange("a", "GET", "/api", "", 200, "");
        let mut b = make_exchange("b", "GET", "/api", "", 200, "");
        b.request.headers.insert("x-custom".to_string(), "value".to_string());
        let diff = diff_exchanges(&a, &b);
        assert!(diff.request_diff.headers_added.iter().any(|h| h.contains("x-custom")));
    }

    #[test]
    fn header_removed_detected() {
        let mut a = make_exchange("a", "GET", "/api", "", 200, "");
        a.request.headers.insert("x-custom".to_string(), "value".to_string());
        let b = make_exchange("b", "GET", "/api", "", 200, "");
        let diff = diff_exchanges(&a, &b);
        assert!(diff.request_diff.headers_removed.iter().any(|h| h.contains("x-custom")));
    }

    #[test]
    fn header_changed_detected() {
        let a = make_exchange("a", "GET", "/api", "", 200, "");
        let mut b = make_exchange("b", "GET", "/api", "", 200, "");
        b.request.headers.insert("content-type".to_string(), "text/plain".to_string());
        let diff = diff_exchanges(&a, &b);
        assert!(diff.request_diff.headers_changed.iter().any(|c| c.name == "content-type"));
    }

    #[test]
    fn timing_delta_calculated_correctly() {
        let mut a = make_exchange("a", "GET", "/api", "", 200, "hello");
        let mut b = make_exchange("b", "GET", "/api", "", 200, "hello");
        a.metrics.as_mut().unwrap().latency_ms = 100;
        b.metrics.as_mut().unwrap().latency_ms = 250;
        a.metrics.as_mut().unwrap().response_size_bytes = 10;
        b.metrics.as_mut().unwrap().response_size_bytes = 30;
        let diff = diff_exchanges(&a, &b);
        assert_eq!(diff.timing_delta.latency_delta_ms, 150);
        assert_eq!(diff.timing_delta.size_delta_bytes, 20);
    }

    #[test]
    fn no_response_produces_none_response_diff() {
        let mut a = make_exchange("a", "GET", "/api", "", 200, "");
        let mut b = make_exchange("b", "GET", "/api", "", 200, "");
        a.response = None;
        b.response = None;
        let diff = diff_exchanges(&a, &b);
        assert!(diff.response_diff.is_none());
    }
}
