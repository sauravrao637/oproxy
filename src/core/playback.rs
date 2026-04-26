use crate::session::{Exchange, SharedSessionManager};
use reqwest::Client;
use std::sync::Arc;
use tracing::{info, warn};

pub struct PlaybackEngine {
    session_manager: SharedSessionManager,
    http_client: Client,
}

impl PlaybackEngine {
    pub fn new(session_manager: SharedSessionManager) -> Self {
        Self {
            session_manager,
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Re-issue all provided exchanges against their original targets.
    /// Responses are recorded back into the session manager as new sessions
    /// so they appear in the UI alongside the originals.
    pub async fn replay(&self, exchanges: Vec<Exchange>) {
        for exchange in exchanges {
            let method = exchange.request.method.clone();
            let uri = exchange.request.uri.clone();

            let Ok(reqwest_method) = reqwest::Method::from_bytes(method.as_bytes()) else {
                warn!(method=%method, uri=%uri, "Playback: unrecognised method, skipping");
                continue;
            };
            let mut builder = self.http_client.request(reqwest_method, &uri);
            for (name, value) in &exchange.request.headers {
                // Skip hop-by-hop headers that shouldn't be re-sent.
                if matches!(name.to_lowercase().as_str(),
                    "host" | "connection" | "transfer-encoding" | "keep-alive"
                    | "proxy-authenticate" | "proxy-authorization" | "te" | "trailer" | "upgrade"
                ) {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                    reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    builder = builder.header(n, v);
                }
            }
            if !exchange.request.body.is_empty() {
                builder = builder.body(exchange.request.body.clone());
            }

            info!(method=%method, uri=%uri, "Playback: replaying");
            match builder.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let new_id = uuid::Uuid::new_v4().to_string();
                    let mut req_ctx = exchange.request.clone();
                    req_ctx.method = format!("[REPLAY] {}", req_ctx.method);
                    self.session_manager.record_request(new_id.clone(), req_ctx);
                    let body = resp.text().await.unwrap_or_default();
                    self.session_manager.record_response(
                        new_id,
                        crate::middleware::ResponseContext {
                            status,
                            headers: std::collections::HashMap::new(),
                            body,
                            request_uri: uri.clone(),
                            session_id: None,
                            ttfb_ms: 0,
                            body_ms: 0,
                            body_bytes: None,
                        },
                    );
                    info!(status=%status, uri=%uri, "Playback: replayed");
                }
                Err(e) => warn!(error=%e, uri=%uri, "Playback: request failed"),
            }
        }
    }
}
