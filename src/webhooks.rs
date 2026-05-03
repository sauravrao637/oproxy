use std::sync::Arc;
use std::time::Duration;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

use crate::session::SessionManager;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvent {
    RequestCaptured,
    ResponseCaptured,
    BreakpointHit,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    pub url: String,
    pub events: Vec<WebhookEvent>,
    pub enabled: bool,
    pub secret: Option<String>,
}

pub type SharedWebhooks = Arc<RwLock<Vec<WebhookConfig>>>;

fn hmac_sha256_hex(key: &str, data: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(data.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

pub struct WebhookDispatcher {
    configs: SharedWebhooks,
}

impl WebhookDispatcher {
    pub fn new(configs: SharedWebhooks) -> Self {
        Self { configs }
    }

    pub fn spawn(self, mut change_rx: broadcast::Receiver<()>, sm: Arc<SessionManager>) {
        let configs = self.configs.clone();
        tokio::spawn(async move {
            loop {
                match change_rx.recv().await {
                    Ok(_) => {
                        let sessions = sm.get_all_sessions();
                        if let Some(latest) = sessions.last() {
                            let hooks: Vec<WebhookConfig> = configs.read().await.clone();
                            for hook in hooks.iter().filter(|h| h.enabled) {
                                let event = if latest.response.is_some() {
                                    WebhookEvent::ResponseCaptured
                                } else {
                                    WebhookEvent::RequestCaptured
                                };
                                if !hook.events.contains(&event) {
                                    continue;
                                }
                                let payload = serde_json::json!({
                                    "event": serde_json::to_value(&event).unwrap_or_default(),
                                    "session_id": latest.id,
                                    "timestamp": latest.timestamp.to_rfc3339(),
                                    "request": {
                                        "method": latest.request.method,
                                        "uri": latest.request.uri,
                                        "status": latest.response.as_ref().map(|r| r.status),
                                    }
                                });
                                let payload_str = payload.to_string();
                                let url = hook.url.clone();
                                let sig = hook.secret.as_deref()
                                    .map(|s| hmac_sha256_hex(s, &payload_str));

                                tokio::spawn(async move {
                                    fire_webhook(&url, &payload_str, sig.as_deref()).await;
                                });
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

async fn fire_webhook(url: &str, payload: &str, signature: Option<&str>) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("webhook client build failed: {e}");
            return;
        }
    };

    let mut attempts = 0u32;
    loop {
        let mut req = client
            .post(url)
            .header("content-type", "application/json");
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
            url: "http://example.com/hook".to_string(),
            events: vec![WebhookEvent::ResponseCaptured, WebhookEvent::RequestCaptured],
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
            url: "http://example.com".to_string(),
            events: vec![WebhookEvent::ResponseCaptured],
            enabled: false,
            secret: None,
        };
        // disabled hook should be filtered out
        let hooks = vec![hook];
        let active: Vec<_> = hooks.iter().filter(|h| h.enabled).collect();
        assert!(active.is_empty());
    }

    #[test]
    fn event_not_in_webhook_events_skipped() {
        let hook = WebhookConfig {
            id: "x".to_string(),
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
}
