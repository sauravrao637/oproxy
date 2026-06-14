//! Forwarding and protocol planning contracts.
//!
//! Defines protocol context, body-access requirements, and forwarding plans.

use serde::{Deserialize, Serialize};

/// Protocol family on one side of a proxied exchange. This is deliberately
/// transport-shaped, not feature-shaped: rules and plugins should reason about
/// "HTTP/2" or "SOCKS5 tunnel" through this enum instead of ad-hoc strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WireProtocol {
    #[default]
    Http1,
    Http2,
    Http3,
    Socks5,
    WebSocket,
}

impl WireProtocol {
    pub fn label(self) -> &'static str {
        match self {
            WireProtocol::Http1 => "HTTP/1.1",
            WireProtocol::Http2 => "HTTP/2",
            WireProtocol::Http3 => "HTTP/3",
            WireProtocol::Socks5 => "SOCKS5",
            WireProtocol::WebSocket => "WebSocket",
        }
    }

    pub fn match_value(self) -> &'static str {
        match self {
            WireProtocol::Http1 => "http1",
            WireProtocol::Http2 => "http2",
            WireProtocol::Http3 => "http3",
            WireProtocol::Socks5 => "socks5",
            WireProtocol::WebSocket => "websocket",
        }
    }

    pub fn from_http_version(v: axum::http::Version) -> Self {
        match v {
            axum::http::Version::HTTP_2 => WireProtocol::Http2,
            axum::http::Version::HTTP_3 => WireProtocol::Http3,
            _ => WireProtocol::Http1,
        }
    }
}

/// Application protocol inferred from request metadata. This is separate from
/// the wire protocol because gRPC and SSE are HTTP applications riding h2/h3/h1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationProtocol {
    #[default]
    Http,
    Grpc,
    Sse,
    Graphql,
    Json,
    Binary,
}

impl ApplicationProtocol {
    pub fn match_value(self) -> &'static str {
        match self {
            ApplicationProtocol::Http => "http",
            ApplicationProtocol::Grpc => "grpc",
            ApplicationProtocol::Sse => "sse",
            ApplicationProtocol::Graphql => "graphql",
            ApplicationProtocol::Json => "json",
            ApplicationProtocol::Binary => "binary",
        }
    }
}

/// Body shape the engine must preserve for correctness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BodyMode {
    Empty,
    #[default]
    Full,
    StreamBytes,
    StreamMessages,
    Frames,
    Tunnel,
}

impl BodyMode {
    pub fn match_value(self) -> &'static str {
        match self {
            BodyMode::Empty => "empty",
            BodyMode::Full => "full",
            BodyMode::StreamBytes => "stream_bytes",
            BodyMode::StreamMessages => "stream_messages",
            BodyMode::Frames => "frames",
            BodyMode::Tunnel => "tunnel",
        }
    }
}

/// Typed protocol identity for one proxied exchange. It is safe to persist, but
/// it should be moved through the runtime as typed context, never as headers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProtocolContext {
    pub downstream: WireProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<WireProtocol>,
    pub application: ApplicationProtocol,
    pub body_mode: BodyMode,
    pub scheme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<u64>,
}

impl ProtocolContext {
    pub fn http(
        version: axum::http::Version,
        scheme: impl Into<String>,
        application: ApplicationProtocol,
        body_mode: BodyMode,
    ) -> Self {
        Self {
            downstream: WireProtocol::from_http_version(version),
            upstream: None,
            application,
            body_mode,
            scheme: scheme.into(),
            connection_id: None,
            stream_id: None,
        }
    }

    pub fn with_identity(mut self, connection_id: Option<String>, stream_id: Option<u64>) -> Self {
        self.connection_id = connection_id;
        self.stream_id = stream_id;
        self
    }

    /// Protocol identity for a WebSocket exchange (frame body mode).
    pub fn websocket(scheme: impl Into<String>) -> Self {
        Self {
            downstream: WireProtocol::WebSocket,
            upstream: None,
            application: ApplicationProtocol::Http,
            body_mode: BodyMode::Frames,
            scheme: scheme.into(),
            connection_id: None,
            stream_id: None,
        }
    }

    /// Protocol identity for a raw SOCKS5 tunnel (opaque bytes, no HTTP).
    pub fn socks5_tunnel() -> Self {
        Self {
            downstream: WireProtocol::Socks5,
            upstream: None,
            application: ApplicationProtocol::Binary,
            body_mode: BodyMode::Tunnel,
            scheme: "socks5".to_string(),
            connection_id: None,
            stream_id: None,
        }
    }
}

/// Encodes one gRPC length-prefixed frame:
/// `[1B compressed flag][4B big-endian length][N bytes message]` — shared by
/// the gRPC inspector, mock scripts, and Compose forwarding.
pub fn encode_grpc_frame(compressed: bool, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(u8::from(compressed));
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Granularity at which an inspecting plugin wants to observe a streamed body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    /// Per network chunk, as bytes arrive.
    Bytes,
    /// Per length-prefixed application message (e.g. the 5-byte gRPC frame).
    /// `needs_full_body` then applies *per message*, never to the whole stream.
    Messages,
}

/// Declares how a plugin needs to access the body. The engine evaluates this
/// from request metadata before forwarding any body bytes.
///
/// Streaming plugins may inspect but not mutate body data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyHint {
    /// Buffer the whole body; mutation is allowed (mock, rewrite, Lua, breakpoint).
    FullBody,
    /// Observe the streamed body without mutating it (inspectors).
    StreamingInspect { granularity: Granularity },
}

impl Default for BodyHint {
    /// Plugins are assumed to need the whole body unless they opt into streaming.
    fn default() -> Self {
        BodyHint::FullBody
    }
}

/// Which forwarding path handles an exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardClass {
    /// Buffer the body, subject to `max_body_bytes`, so middleware can mutate it.
    Buffered,
    /// Relay the body with back-pressure while allowing inspection.
    Streaming,
}

/// Execution path selected from protocol context and active middleware needs.
/// It is richer than [`ForwardClass`] so the planner can represent frame and
/// tunnel traffic even while the current engine still maps HTTP requests to the
/// buffered/streaming forwarders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionClass {
    Buffered,
    StreamingInspect,
    FrameInspect,
    TunnelMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityPlan {
    pub execution: ExecutionClass,
    pub forward_class: ForwardClass,
    pub diagnostic: Option<String>,
}

/// Selects the execution path for an exchange. The important invariant is that
/// the decision is made from the request head and plugin declarations only.
pub fn plan_execution<I>(protocol: &ProtocolContext, hints: I) -> CapabilityPlan
where
    I: IntoIterator<Item = BodyHint>,
{
    let mut saw_streaming = false;
    for hint in hints {
        match hint {
            BodyHint::FullBody => {
                let diagnostic = match protocol.body_mode {
                    BodyMode::Tunnel => Some(
                        "full-body middleware cannot run on a raw tunnel; only metadata-safe actions are supported"
                            .to_string(),
                    ),
                    BodyMode::Frames => Some(
                        "full-body middleware forces buffered HTTP handling; frame streams need a frame-aware action"
                            .to_string(),
                    ),
                    _ => None,
                };
                return CapabilityPlan {
                    execution: ExecutionClass::Buffered,
                    forward_class: ForwardClass::Buffered,
                    diagnostic,
                };
            }
            BodyHint::StreamingInspect { .. } => saw_streaming = true,
        }
    }

    match protocol.body_mode {
        BodyMode::Tunnel => CapabilityPlan {
            execution: ExecutionClass::TunnelMetadata,
            forward_class: ForwardClass::Buffered,
            diagnostic: None,
        },
        BodyMode::Frames => CapabilityPlan {
            execution: ExecutionClass::FrameInspect,
            forward_class: ForwardClass::Streaming,
            diagnostic: None,
        },
        BodyMode::StreamBytes | BodyMode::StreamMessages if saw_streaming => CapabilityPlan {
            execution: ExecutionClass::StreamingInspect,
            forward_class: ForwardClass::Streaming,
            diagnostic: None,
        },
        _ if saw_streaming => CapabilityPlan {
            execution: ExecutionClass::StreamingInspect,
            forward_class: ForwardClass::Streaming,
            diagnostic: None,
        },
        _ => CapabilityPlan {
            execution: ExecutionClass::Buffered,
            forward_class: ForwardClass::Buffered,
            diagnostic: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_context_maps_http_versions_to_wire_protocol() {
        let ctx = ProtocolContext::http(
            axum::http::Version::HTTP_2,
            "https",
            ApplicationProtocol::Grpc,
            BodyMode::StreamMessages,
        )
        .with_identity(Some("conn-1".to_string()), Some(7));

        assert_eq!(ctx.downstream, WireProtocol::Http2);
        assert_eq!(ctx.application, ApplicationProtocol::Grpc);
        assert_eq!(ctx.body_mode, BodyMode::StreamMessages);
        assert_eq!(ctx.connection_id.as_deref(), Some("conn-1"));
        assert_eq!(ctx.stream_id, Some(7));
    }

    #[test]
    fn planner_keeps_stream_messages_streaming_when_plugins_can_inspect() {
        let ctx = ProtocolContext {
            downstream: WireProtocol::Http2,
            application: ApplicationProtocol::Grpc,
            body_mode: BodyMode::StreamMessages,
            scheme: "https".to_string(),
            ..Default::default()
        };

        let plan = plan_execution(
            &ctx,
            [BodyHint::StreamingInspect {
                granularity: Granularity::Messages,
            }],
        );

        assert_eq!(plan.execution, ExecutionClass::StreamingInspect);
        assert_eq!(plan.forward_class, ForwardClass::Streaming);
        assert!(plan.diagnostic.is_none());
    }

    #[test]
    fn planner_marks_raw_tunnel_as_metadata_only_without_full_body_plugins() {
        let ctx = ProtocolContext {
            downstream: WireProtocol::Socks5,
            body_mode: BodyMode::Tunnel,
            scheme: "socks5".to_string(),
            ..Default::default()
        };

        let plan = plan_execution(
            &ctx,
            [BodyHint::StreamingInspect {
                granularity: Granularity::Bytes,
            }],
        );

        assert_eq!(plan.execution, ExecutionClass::TunnelMetadata);
        assert_eq!(plan.forward_class, ForwardClass::Buffered);
        assert!(plan.diagnostic.is_none());
    }

    #[test]
    fn planner_explains_full_body_plugin_on_tunnel() {
        let ctx = ProtocolContext {
            downstream: WireProtocol::Socks5,
            body_mode: BodyMode::Tunnel,
            scheme: "socks5".to_string(),
            ..Default::default()
        };

        let plan = plan_execution(&ctx, [BodyHint::FullBody]);

        assert_eq!(plan.execution, ExecutionClass::Buffered);
        assert_eq!(plan.forward_class, ForwardClass::Buffered);
        assert!(
            plan.diagnostic
                .as_deref()
                .unwrap_or_default()
                .contains("raw tunnel")
        );
    }

    #[test]
    fn default_body_hint_is_full_body() {
        assert_eq!(BodyHint::default(), BodyHint::FullBody);
    }
}
