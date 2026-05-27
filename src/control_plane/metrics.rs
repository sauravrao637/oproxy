use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::session::SessionSource;

const ENDPOINT_TIMING_LIMIT: usize = 64;

pub(crate) type SharedEndpointMetrics = Arc<std::sync::Mutex<EndpointMetrics>>;

#[derive(Debug, Clone)]
struct EndpointTimingSample {
    endpoint: &'static str,
    duration_ms: u64,
    session_count: usize,
    timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Default)]
pub(crate) struct EndpointMetrics {
    samples: VecDeque<EndpointTimingSample>,
}

pub(crate) fn new_endpoint_metrics() -> SharedEndpointMetrics {
    Arc::new(std::sync::Mutex::new(EndpointMetrics::default()))
}

impl EndpointMetrics {
    fn record(&mut self, endpoint: &'static str, elapsed: Duration, session_count: usize) {
        if self.samples.len() >= ENDPOINT_TIMING_LIMIT {
            self.samples.pop_front();
        }
        self.samples.push_back(EndpointTimingSample {
            endpoint,
            duration_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
            session_count,
            timestamp: chrono::Utc::now(),
        });
    }

    fn payload(&self) -> serde_json::Value {
        let mut grouped: BTreeMap<&'static str, Vec<&EndpointTimingSample>> = BTreeMap::new();
        for sample in &self.samples {
            grouped.entry(sample.endpoint).or_default().push(sample);
        }

        let summaries: BTreeMap<_, _> = grouped
            .into_iter()
            .map(|(endpoint, samples)| {
                let total: u64 = samples.iter().map(|sample| sample.duration_ms).sum();
                let max = samples
                    .iter()
                    .map(|sample| sample.duration_ms)
                    .max()
                    .unwrap_or(0);
                let last = samples.last().copied();
                (
                    endpoint,
                    serde_json::json!({
                        "samples": samples.len(),
                        "last_ms": last.map(|sample| sample.duration_ms).unwrap_or(0),
                        "avg_ms": if samples.is_empty() { 0 } else { total / samples.len() as u64 },
                        "max_ms": max,
                        "last_session_count": last.map(|sample| sample.session_count).unwrap_or(0),
                    }),
                )
            })
            .collect();

        let recent: Vec<_> = self
            .samples
            .iter()
            .rev()
            .take(12)
            .map(|sample| {
                serde_json::json!({
                    "endpoint": sample.endpoint,
                    "duration_ms": sample.duration_ms,
                    "session_count": sample.session_count,
                    "timestamp": sample.timestamp.to_rfc3339(),
                })
            })
            .collect();

        serde_json::json!({
            "sample_limit": ENDPOINT_TIMING_LIMIT,
            "summaries": summaries,
            "recent": recent,
        })
    }
}

pub(super) fn record_endpoint_timing(
    metrics: &SharedEndpointMetrics,
    endpoint: &'static str,
    started: Instant,
    session_count: usize,
) {
    if let Ok(mut guard) = metrics.lock() {
        guard.record(endpoint, started.elapsed(), session_count);
    }
}

pub(super) fn endpoint_timing_payload(metrics: &SharedEndpointMetrics) -> serde_json::Value {
    metrics
        .lock()
        .map(|guard| guard.payload())
        .unwrap_or_else(|_| serde_json::json!({ "error": "endpoint metrics unavailable" }))
}

pub(super) fn build_metrics_payload(sessions: &[crate::session::Exchange]) -> serde_json::Value {
    let raw: Vec<_> = sessions.iter().filter_map(|s| s.metrics.as_ref()).collect();
    let latency_samples: Vec<u64> = raw.iter().map(|m| m.latency_ms).collect();
    let captured_session_count = sessions.len();
    let active_requests = sessions.iter().filter(|s| s.response.is_none()).count();
    let completed_requests = captured_session_count.saturating_sub(active_requests);
    let proxied_requests = sessions
        .iter()
        .filter(|s| s.source == SessionSource::Proxy)
        .count();
    let admin_forward_requests = sessions
        .iter()
        .filter(|s| s.source == SessionSource::AdminForward)
        .count();
    let playback_requests = sessions
        .iter()
        .filter(|s| s.source == SessionSource::Playback)
        .count();
    let imported_sessions = sessions
        .iter()
        .filter(|s| s.source == SessionSource::Imported)
        .count();
    let inspected_requests = raw.len();
    let error_count = raw.iter().filter(|m| m.status_code >= 400).count();
    let total_request_bytes: u64 = raw.iter().map(|m| m.request_size_bytes as u64).sum();
    let total_response_bytes: u64 = raw.iter().map(|m| m.response_size_bytes as u64).sum();
    let avg_latency_ms = if inspected_requests > 0 {
        raw.iter().map(|m| m.latency_ms).sum::<u64>() / inspected_requests as u64
    } else {
        0
    };
    let avg_request_size_bytes = if inspected_requests > 0 {
        total_request_bytes / inspected_requests as u64
    } else {
        0
    };
    let avg_response_size_bytes = if inspected_requests > 0 {
        total_response_bytes / inspected_requests as u64
    } else {
        0
    };
    serde_json::json!({
        "sessions": {
            "captured": captured_session_count,
            "active_without_response": active_requests,
            "completed": completed_requests,
            "by_source": {
                "proxy": proxied_requests,
                "admin_forward": admin_forward_requests,
                "playback": playback_requests,
                "imported": imported_sessions,
            },
        },
        "requests": {
            "active": active_requests,
            "completed_with_metrics": inspected_requests,
            "errors": error_count,
            "proxied": proxied_requests,
            "admin_forward": admin_forward_requests,
            "playback": playback_requests,
        },
        "captured_session_count": captured_session_count,
        "active_requests": active_requests,
        "completed_requests": completed_requests,
        "proxied_requests": proxied_requests,
        "admin_forward_requests": admin_forward_requests,
        "playback_requests": playback_requests,
        "imported_sessions": imported_sessions,
        "inspected_requests": inspected_requests,
        "error_count": error_count,
        "latency_samples": latency_samples,
        "total_request_bytes": total_request_bytes,
        "total_response_bytes": total_response_bytes,
        "avg_latency_ms": avg_latency_ms,
        "avg_request_size_bytes": avg_request_size_bytes,
        "avg_response_size_bytes": avg_response_size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{RequestContext, ResponseContext};
    use crate::session::{Exchange, InspectionMetrics};
    use std::collections::HashMap;

    fn exchange(id: &str, source: SessionSource, status: Option<u16>) -> Exchange {
        let response = status.map(|code| ResponseContext {
            status: code,
            headers: HashMap::new(),
            body: String::new(),
            request_uri: "/test".to_string(),
            session_id: Some(id.to_string()),
            ttfb_ms: 4,
            body_ms: 2,
            body_bytes: None,
        });
        let metrics = status.map(|code| InspectionMetrics {
            latency_ms: 12,
            request_size_bytes: 3,
            response_size_bytes: 5,
            status_code: code,
            ttfb_ms: 4,
            body_ms: 2,
            ..Default::default()
        });
        Exchange {
            id: id.to_string(),
            timestamp: chrono::Utc::now(),
            updated_at: None,
            request: RequestContext {
                method: "GET".to_string(),
                uri: "/test".to_string(),
                headers: HashMap::new(),
                body: String::new(),
                host: "example.com".to_string(),
                body_bytes: None,
            },
            response,
            metrics,
            source,
            ws_frames: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
        }
    }

    #[test]
    fn metrics_payload_splits_session_sources_and_active_requests() {
        let sessions = vec![
            exchange("proxy-ok", SessionSource::Proxy, Some(200)),
            exchange("proxy-pending", SessionSource::Proxy, None),
            exchange("admin", SessionSource::AdminForward, Some(502)),
            exchange("imported", SessionSource::Imported, Some(201)),
        ];

        let metrics = build_metrics_payload(&sessions);

        assert_eq!(metrics["captured_session_count"], 4);
        assert_eq!(metrics["active_requests"], 1);
        assert_eq!(metrics["completed_requests"], 3);
        assert_eq!(metrics["proxied_requests"], 2);
        assert_eq!(metrics["admin_forward_requests"], 1);
        assert_eq!(metrics["imported_sessions"], 1);
        assert_eq!(metrics["inspected_requests"], 3);
        assert_eq!(metrics["error_count"], 1);
        assert_eq!(metrics["sessions"]["captured"], 4);
        assert_eq!(metrics["sessions"]["active_without_response"], 1);
        assert_eq!(metrics["sessions"]["completed"], 3);
        assert_eq!(metrics["sessions"]["by_source"]["proxy"], 2);
        assert_eq!(metrics["sessions"]["by_source"]["admin_forward"], 1);
        assert_eq!(metrics["sessions"]["by_source"]["imported"], 1);
        assert_eq!(metrics["requests"]["active"], 1);
        assert_eq!(metrics["requests"]["completed_with_metrics"], 3);
        assert_eq!(metrics["requests"]["proxied"], 2);
        assert_eq!(metrics["requests"]["admin_forward"], 1);
        assert!(metrics.get("total_requests").is_none());
        assert!(metrics.get("active_sessions").is_none());
    }
}
