use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};

use crate::session::{SessionChange, SessionChangeKind, SessionManager};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvent {
    RequestCaptured,
    ResponseCaptured,
    // Legacy config values retained only so older webhooks.json files deserialize.
    // These session changes are not dispatched as webhooks.
    BreakpointHit,
    Error,
}

impl WebhookEvent {
    pub fn is_dispatchable(&self) -> bool {
        matches!(self, Self::RequestCaptured | Self::ResponseCaptured)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub url: String,
    pub events: Vec<WebhookEvent>,
    pub enabled: bool,
    pub secret: Option<String>,
}

pub fn sanitize_webhook_events(events: &mut Vec<WebhookEvent>) {
    let mut seen = std::collections::HashSet::new();
    events.retain(WebhookEvent::is_dispatchable);
    events.retain(|event| seen.insert(event.clone()));
}

pub type SharedWebhooks = Arc<RwLock<Vec<WebhookConfig>>>;

fn hmac_sha256_hex(key: &str, data: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(data.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

pub struct WebhookDispatcher {
    configs: SharedWebhooks,
    client: reqwest::Client,
}

impl WebhookDispatcher {
    pub fn new(configs: SharedWebhooks) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("webhook reqwest client");
        Self { configs, client }
    }

    pub fn spawn(self, mut change_rx: broadcast::Receiver<SessionChange>, sm: Arc<SessionManager>) {
        let configs = self.configs.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            loop {
                match change_rx.recv().await {
                    Ok(change) => {
                        let event = match change.kind {
                            SessionChangeKind::RequestCaptured => WebhookEvent::RequestCaptured,
                            SessionChangeKind::ResponseCaptured => WebhookEvent::ResponseCaptured,
                            _ => continue,
                        };
                        let Some(session_id) = change.session_id else {
                            continue;
                        };
                        let Some(session) = sm.get_session(&session_id) else {
                            continue;
                        };

                        let hooks: Vec<WebhookConfig> = configs.read().await.clone();
                        for hook in hooks.iter().filter(|h| h.enabled) {
                            if !hook.events.contains(&event) {
                                continue;
                            }
                            let payload = serde_json::json!({
                                "event": serde_json::to_value(&event).unwrap_or_default(),
                                "session_id": session.id,
                                "timestamp": session.timestamp.to_rfc3339(),
                                "request": {
                                    "method": session.request.method,
                                    "uri": session.request.uri,
                                    "status": session.response.as_ref().map(|r| r.status),
                                }
                            });
                            let payload_str = payload.to_string();
                            let url = hook.url.clone();
                            let sig = hook
                                .secret
                                .as_deref()
                                .map(|s| hmac_sha256_hex(s, &payload_str));
                            let c = client.clone();
                            tokio::spawn(async move {
                                fire_webhook(&c, &url, &payload_str, sig.as_deref()).await;
                            });
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

async fn fire_webhook(client: &reqwest::Client, url: &str, payload: &str, signature: Option<&str>) {
    let mut attempts = 0u32;
    loop {
        let mut req = client.post(url).header("content-type", "application/json");
        if let Some(sig) = signature {
            req = req.header("x-oproxy-signature", sig);
        }
        match req.body(payload.to_string()).send().await {
            Ok(resp) => {
                info!("webhook {} fired, status {}", url, resp.status());
                return;
            }
            Err(e) => {
                attempts += 1;
                if attempts >= 3 {
                    warn!("webhook {} failed after 3 attempts: {e}", url);
                    return;
                }
                let delay = Duration::from_millis(200 * (1 << attempts) as u64);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_produces_deterministic_output() {
        let sig1 = hmac_sha256_hex("secret", "payload");
        let sig2 = hmac_sha256_hex("secret", "payload");
        assert_eq!(sig1, sig2);
        assert_eq!(sig1.len(), 64);
    }

    #[test]
    fn hmac_sha256_different_keys_produce_different_sigs() {
        let sig1 = hmac_sha256_hex("key1", "payload");
        let sig2 = hmac_sha256_hex("key2", "payload");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn hmac_sha256_different_data_produce_different_sigs() {
        let sig1 = hmac_sha256_hex("key", "data1");
        let sig2 = hmac_sha256_hex("key", "data2");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn webhook_config_serializes_roundtrip() {
        let hook = WebhookConfig {
            id: "abc".to_string(),
            name: Some("test hook".to_string()),
            url: "http://example.com/hook".to_string(),
            events: vec![
                WebhookEvent::ResponseCaptured,
                WebhookEvent::RequestCaptured,
            ],
            enabled: true,
            secret: Some("s3cr3t".to_string()),
        };
        let json = serde_json::to_string(&hook).unwrap();
        let decoded: WebhookConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "abc");
        assert_eq!(decoded.events.len(), 2);
        assert_eq!(decoded.secret.as_deref(), Some("s3cr3t"));
    }

    #[test]
    fn disabled_webhook_skipped_in_events() {
        let hook = WebhookConfig {
            id: "x".to_string(),
            name: None,
            url: "http://example.com".to_string(),
            events: vec![WebhookEvent::ResponseCaptured],
            enabled: false,
            secret: None,
        };
        // disabled hook should be filtered out
        let hooks = [hook];
        let active: Vec<_> = hooks.iter().filter(|h| h.enabled).collect();
        assert!(active.is_empty());
    }

    #[test]
    fn event_not_in_webhook_events_skipped() {
        let hook = WebhookConfig {
            id: "x".to_string(),
            name: None,
            url: "http://example.com".to_string(),
            events: vec![WebhookEvent::RequestCaptured],
            enabled: true,
            secret: None,
        };
        assert!(!hook.events.contains(&WebhookEvent::ResponseCaptured));
        assert!(hook.events.contains(&WebhookEvent::RequestCaptured));
    }

    #[test]
    fn webhook_event_serializes_as_snake_case() {
        let ev = WebhookEvent::ResponseCaptured;
        let s = serde_json::to_string(&ev).unwrap();
        assert_eq!(s, "\"response_captured\"");
    }

    #[test]
    fn sanitize_webhook_events_removes_never_dispatched_events() {
        let mut events = vec![
            WebhookEvent::BreakpointHit,
            WebhookEvent::RequestCaptured,
            WebhookEvent::Error,
            WebhookEvent::RequestCaptured,
            WebhookEvent::ResponseCaptured,
        ];

        sanitize_webhook_events(&mut events);

        assert_eq!(
            events,
            vec![
                WebhookEvent::RequestCaptured,
                WebhookEvent::ResponseCaptured
            ]
        );
    }
}
