use crate::session::{Exchange, SessionSource, SharedSessionManager};
use reqwest::Client;
use tracing::{info, warn};

struct PreparedReplay {
    exchange: Exchange,
    method: reqwest::Method,
    uri: String,
}

pub struct PlaybackEngine {
    session_manager: SharedSessionManager,
    http_client: Client,
    egress_policy: crate::security::AdminEgressPolicy,
}

impl PlaybackEngine {
    pub fn new(
        session_manager: SharedSessionManager,
        egress_policy: crate::security::AdminEgressPolicy,
    ) -> Self {
        Self {
            session_manager,
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            egress_policy,
        }
    }

    /// Re-issue all provided exchanges against their original targets.
    /// Responses are recorded back into the session manager as new sessions
    /// so they appear in the UI alongside the originals.
    pub async fn replay(&self, exchanges: Vec<Exchange>) {
        for exchange in exchanges {
            self.replay_one(exchange).await;
        }
    }

    async fn replay_one(&self, exchange: Exchange) {
        let Some(replay) = self.prepare_replay(exchange).await else {
            return;
        };
        let method = replay.exchange.request.method.clone();
        info!(method=%method, uri=%replay.uri, "Playback: replaying");
        let response = match self.build_request(&replay).send().await {
            Ok(response) => response,
            Err(error) => {
                warn!(%error, uri=%replay.uri, "Playback: request failed");
                return;
            }
        };
        let status = response.status().as_u16();
        let new_id = uuid::Uuid::new_v4().to_string();
        let mut request = replay.exchange.request;
        request.method = format!("[REPLAY] {}", request.method);
        self.session_manager.record_request_with_source(
            new_id.clone(),
            request,
            SessionSource::Playback,
        );
        self.session_manager.record_response(
            new_id,
            crate::middleware::ResponseContext {
                status,
                body: response.bytes().await.unwrap_or_default(),
                request_uri: replay.uri.clone(),
                ..Default::default()
            },
        );
        info!(status, uri=%replay.uri, "Playback: replayed");
    }

    async fn prepare_replay(&self, exchange: Exchange) -> Option<PreparedReplay> {
        let method_name = &exchange.request.method;
        let uri = exchange.request.uri.clone();
        let method = reqwest::Method::from_bytes(method_name.as_bytes())
            .ok()
            .or_else(|| {
                warn!(method=%method_name, uri=%uri, "Playback: unrecognised method, skipping");
                None
            })?;
        let parsed_url = reqwest::Url::parse(&uri).ok().or_else(|| {
            warn!(uri=%uri, "Playback: invalid URL, skipping");
            None
        })?;
        if let Err(error) =
            crate::security::enforce_admin_egress_policy(&parsed_url, self.egress_policy).await
        {
            warn!(uri=%uri, reason=%error, "Playback: blocked by admin egress policy");
            return None;
        }
        Some(PreparedReplay {
            exchange,
            method,
            uri,
        })
    }

    fn build_request(&self, replay: &PreparedReplay) -> reqwest::RequestBuilder {
        let mut request = self.http_client.request(replay.method.clone(), &replay.uri);
        for (name, value) in &replay.exchange.request.headers {
            if is_hop_by_hop_header(name) {
                continue;
            }
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
            ) {
                request = request.header(name, value);
            }
        }
        if !replay.exchange.request.body.is_empty() {
            request = request.body(replay.exchange.request.body.clone());
        }
        request
    }
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "connection"
            | "transfer-encoding"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "upgrade"
    )
}
