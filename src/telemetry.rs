//! OpenTelemetry export of per-exchange protocol metrics.
//!
//! Decision §4: **one span per exchange**, carrying `connection_id` / `stream_id`
//! and protocol attributes — no long-lived connection-parent span. The
//! connection→streams structure is recovered by grouping on `connection_id`
//! (the Connections view already does this) or via span links in a trace viewer.
//!
//! [`span_attributes`] is pure and always compiled, so the attribute mapping is
//! unit-testable regardless of feature flags. The emission ([`export_exchange`])
//! is gated behind the `otel` Cargo feature and is a no-op otherwise.
//!
//! Wiring the OTLP exporter: when `otel` is enabled and `Config.otel_enabled`
//! is set, attach a `tracing-opentelemetry` layer pointed at
//! `Config.otel_endpoint` in `runtime::logging` at startup. This module emits a
//! `tracing` span/event per exchange (target `oproxy::otel`) that the layer
//! converts to OTLP spans; without such a layer the emission is inert.
//!
//! Without the `otel` feature the emission call site is compiled out, so these
//! functions are only reached from tests; suppress dead_code for that build.
#![cfg_attr(not(feature = "otel"), allow(dead_code))]

use crate::session::Exchange;

/// OpenTelemetry-style attributes for one completed exchange. Attribute names
/// follow OTel HTTP semantic conventions where applicable, with `oproxy.*` for
/// proxy-specific fields. Pure and always available for testing.
pub fn span_attributes(ex: &Exchange) -> Vec<(&'static str, String)> {
    let mut a: Vec<(&'static str, String)> = Vec::with_capacity(12);
    a.push(("http.request.method", ex.request.method.clone()));
    if !ex.request.host.is_empty() {
        a.push(("server.address", ex.request.host.clone()));
    }
    a.push(("url.full", ex.request.uri.clone()));

    if let Some(m) = &ex.metrics {
        a.push(("http.response.status_code", m.status_code.to_string()));
        a.push(("oproxy.latency_ms", m.latency_ms.to_string()));
        a.push(("oproxy.ttfb_ms", m.ttfb_ms.to_string()));
        a.push(("oproxy.request_bytes", m.request_size_bytes.to_string()));
        a.push(("oproxy.response_bytes", m.response_size_bytes.to_string()));
        if let Some(p) = &m.protocol {
            // Upstream (proxy→origin) negotiated protocol.
            a.push(("oproxy.upstream.protocol", p.clone()));
        }
    }
    if let Some(p) = &ex.downstream_protocol {
        // Downstream (client→proxy) negotiated protocol — the OTel "network.protocol".
        a.push(("network.protocol.name", p.clone()));
    }
    if let Some(c) = &ex.connection_id {
        a.push(("oproxy.connection_id", c.clone()));
    }
    if let Some(s) = ex.stream_id {
        a.push(("oproxy.stream_id", s.to_string()));
    }
    if let Some(g) = ex.inspector_data.as_ref().and_then(|i| i.grpc.as_ref())
        && let Some(st) = &g.grpc_status
    {
        a.push(("rpc.grpc.status_code", st.clone()));
    }
    a
}

/// Emits a per-exchange span carrying the OTel attributes. Compiled to a no-op
/// without the `otel` feature, so the default build pays nothing.
#[cfg(feature = "otel")]
pub fn export_exchange(ex: &Exchange) {
    let attrs = span_attributes(ex);
    // `tracing` field names must be static, so the variable attribute set is
    // serialised into one field; a tracing-opentelemetry layer (attached at
    // startup when otel is enabled) maps this event onto an OTLP span.
    let serialised = attrs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
    tracing::info!(
        target: "oproxy::otel",
        exchange_id = %ex.id,
        otel_attributes = %serialised,
        "exchange",
    );
}

/// No-op when the `otel` feature is disabled.
#[cfg(not(feature = "otel"))]
#[inline]
pub fn export_exchange(_ex: &Exchange) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{HeaderMap, RequestContext};
    use crate::session::{Exchange, InspectionMetrics};

    fn sample() -> Exchange {
        Exchange {
            id: "x1".to_string(),
            timestamp: chrono::Utc::now(),
            updated_at: None,
            request: RequestContext {
                method: "POST".to_string(),
                uri: "https://api.test/v1.Svc/Call".to_string(),
                headers: HeaderMap::new(),
                body: bytes::Bytes::new(),
                host: "api.test".to_string(),
                ..Default::default()
            },
            response: None,
            metrics: Some(InspectionMetrics {
                status_code: 200,
                latency_ms: 42,
                ttfb_ms: 30,
                request_size_bytes: 10,
                response_size_bytes: 200,
                protocol: Some("HTTP/2".to_string()),
                ..Default::default()
            }),
            source: Default::default(),
            ws_frames: vec![],
            events: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
            paused_at: None,
            connection_id: Some("conn-9".to_string()),
            stream_id: Some(3),
            downstream_protocol: Some("HTTP/3".to_string()),
            protocol_context: None,
        }
    }

    fn get<'a>(attrs: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        attrs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn maps_core_protocol_attributes() {
        let attrs = span_attributes(&sample());
        assert_eq!(get(&attrs, "http.request.method"), Some("POST"));
        assert_eq!(get(&attrs, "http.response.status_code"), Some("200"));
        assert_eq!(get(&attrs, "oproxy.latency_ms"), Some("42"));
        assert_eq!(get(&attrs, "oproxy.upstream.protocol"), Some("HTTP/2"));
        assert_eq!(get(&attrs, "network.protocol.name"), Some("HTTP/3"));
        assert_eq!(get(&attrs, "oproxy.connection_id"), Some("conn-9"));
        assert_eq!(get(&attrs, "oproxy.stream_id"), Some("3"));
    }

    #[test]
    fn omits_absent_optional_fields() {
        let mut ex = sample();
        ex.metrics = None;
        ex.connection_id = None;
        ex.stream_id = None;
        let attrs = span_attributes(&ex);
        assert!(get(&attrs, "http.response.status_code").is_none());
        assert!(get(&attrs, "oproxy.connection_id").is_none());
        // Method and URL are always present.
        assert_eq!(get(&attrs, "http.request.method"), Some("POST"));
    }
}
