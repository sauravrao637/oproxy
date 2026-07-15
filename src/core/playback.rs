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
            new_id.clone(),
            crate::middleware::ResponseContext {
                status,
                body: response.bytes().await.unwrap_or_default(),
                request_uri: replay.uri.clone(),
                ..Default::default()
            },
        );
        // Apply the replay tag explicitly because this path bypasses
        // `InspectionMiddleware`, which normally merges response tags.
        self.session_manager
            .annotate(&new_id, None, Some(vec!["replay".to_string()]))
            .await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionManager;
    use axum::Router;
    use axum::routing::get;

    fn empty_exchange(id: &str, uri: String) -> Exchange {
        Exchange {
            id: id.to_string(),
            timestamp: chrono::Utc::now(),
            updated_at: None,
            request: crate::middleware::RequestContext {
                method: "GET".to_string(),
                uri,
                headers: crate::middleware::HeaderMap::new(),
                body: bytes::Bytes::new(),
                host: "127.0.0.1".to_string(),
                ..Default::default()
            },
            response: None,
            metrics: None,
            source: SessionSource::Proxy,
            ws_frames: vec![],
            events: vec![],
            note: None,
            tags: vec![],
            inspector_data: None,
            paused_at: None,
            connection_id: None,
            stream_id: None,
            downstream_protocol: None,
            protocol_context: None,
        }
    }

    #[tokio::test]
    async fn replay_tags_the_new_session_as_replayed() {
        let upstream = Router::new().route("/echo", get(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let sessions: SharedSessionManager = std::sync::Arc::new(SessionManager::new(10_000));
        let engine = PlaybackEngine::new(
            sessions.clone(),
            crate::security::AdminEgressPolicy::default(),
        );

        let original = empty_exchange("orig", format!("http://127.0.0.1:{}/echo", addr.port()));
        engine.replay(vec![original]).await;

        sessions.flush().await;
        let recorded = sessions.get_all_sessions();
        let replayed = recorded
            .iter()
            .find(|e| matches!(e.source, SessionSource::Playback))
            .expect("the replayed request must be recorded as a new Playback-sourced session");
        assert!(
            replayed.tags.iter().any(|t| t == "replay"),
            "replayed sessions must carry the \"replayed\" tag, got {:?}",
            replayed.tags
        );
    }

    #[tokio::test]
    async fn replay_of_unreachable_target_records_no_session() {
        let sessions: SharedSessionManager = std::sync::Arc::new(SessionManager::new(10_000));
        let engine = PlaybackEngine::new(
            sessions.clone(),
            crate::security::AdminEgressPolicy::default(),
        );

        // Port 1 is very unlikely to have anything listening.
        let original = empty_exchange("orig", "http://127.0.0.1:1/unreachable".to_string());
        engine.replay(vec![original]).await;

        sessions.flush().await;
        assert!(
            sessions
                .get_all_sessions()
                .iter()
                .all(|e| !matches!(e.source, SessionSource::Playback)),
            "a failed replay must not record a spurious empty session"
        );
    }
}
