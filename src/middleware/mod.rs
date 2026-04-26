use async_trait::async_trait;
use std::collections::HashMap;
use bytes::Bytes;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestContext {
    pub method: String,
    pub uri: String,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub host: String,
    /// Raw bytes of the body as received from the client. Populated by the engine
    /// before the middleware chain runs. Middlewares that modify `body` (text) should
    /// clear this to `None` so the engine knows to forward the modified string rather
    /// than the original bytes. Not serialised — only live in memory.
    #[serde(skip)]
    pub body_bytes: Option<Bytes>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResponseContext {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub request_uri: String,
    // Injected by InspectionMiddleware during on_request; used in on_response for exact
    // session lookup so concurrent requests to the same URI don't overwrite each other.
    #[serde(default)]
    pub session_id: Option<String>,
    // Time from request send to response headers received (DNS+TCP+TLS+TTFB).
    #[serde(default)]
    pub ttfb_ms: u64,
    // Time to read response body after headers received.
    #[serde(default)]
    pub body_ms: u64,
    /// Canonical bytes of the response body (decoded from gzip/br if needed).
    /// Engine uses these when no middleware modified `body`. Not serialised.
    #[serde(skip)]
    pub body_bytes: Option<Bytes>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareAction {
    Continue,      // Proceed to next middleware
    StopAndReturn, // Stop chain and return current response (e.g., Map Local)
    Pause,         // Halt execution (e.g., Breakpoint)
}

#[async_trait]
pub trait Middleware: Send + Sync {
    fn name(&self) -> &str;

    /// Process the request before it is sent to the target server.
    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction;

    /// Process the response before it is sent back to the client.
    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction;
}

pub mod chain;
pub mod plugins;
