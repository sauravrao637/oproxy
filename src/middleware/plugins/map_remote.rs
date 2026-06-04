//! Map Remote middleware — Location-based upstream URL remapping.
//!
//! Each [`MapRemoteRule`] couples a [`Location`] matcher with a `destination`
//! base URL. When a request matches, [`RequestContext::destination`] is set so
//! the engine forwards it there instead of the original host.
//!
//! Rules are evaluated in insertion order; the first match wins (unlike the
//! Rewrite engine where all rules run). This is the intended behaviour: you
//! normally want exactly one upstream per matching request.

use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{Middleware, MiddlewareAction, RequestContext};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapRemoteRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub location: Location,
    /// Upstream base URL, e.g. `https://staging.example.com:9000`.
    /// The request path and query string are preserved; only the origin is replaced.
    pub destination: String,
}

fn default_true() -> bool {
    true
}

impl MapRemoteRule {
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }
}

pub type SharedMapRemoteRules = Arc<RwLock<Vec<MapRemoteRule>>>;

pub struct MapRemoteMiddleware {
    pub rules: SharedMapRemoteRules,
}

impl MapRemoteMiddleware {
    pub fn new(rules: Vec<MapRemoteRule>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
        }
    }
}

#[async_trait]
impl Middleware for MapRemoteMiddleware {
    fn name(&self) -> &str {
        "MapRemoteMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        let target = MatchTarget::from_request(ctx);
        for rule in rules.iter().filter(|r| r.enabled) {
            if rule.location.matches(&target) {
                ctx.destination = Some(rule.destination.clone());
                return MiddlewareAction::Continue; // first-match wins
            }
        }
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::matcher::Location;
    use crate::middleware::{Middleware, MiddlewareAction};
    use bytes::Bytes;

    fn req(host: &str, path: &str) -> RequestContext {
        RequestContext {
            method: "GET".into(),
            host: host.into(),
            uri: path.into(),
            headers: crate::middleware::HeaderMap::new(),
            body: Bytes::new(),
            ..Default::default()
        }
    }

    fn rule(host: &str, dst: &str) -> MapRemoteRule {
        MapRemoteRule {
            id: "t".into(),
            name: "t".into(),
            enabled: true,
            location: Location {
                host: Some(host.into()),
                ..Default::default()
            },
            destination: dst.into(),
        }
    }

    #[tokio::test]
    async fn sets_destination_for_matching_host() {
        let mw = MapRemoteMiddleware::new(vec![rule("api.local", "http://10.0.0.1:3000")]);
        let mut ctx = req("api.local", "/v1/users");
        assert_eq!(mw.on_request(&mut ctx).await, MiddlewareAction::Continue);
        assert_eq!(ctx.destination.as_deref(), Some("http://10.0.0.1:3000"));
    }

    #[tokio::test]
    async fn no_destination_for_unmatched_host() {
        let mw = MapRemoteMiddleware::new(vec![rule("api.local", "http://10.0.0.1:3000")]);
        let mut ctx = req("other.host", "/");
        mw.on_request(&mut ctx).await;
        assert!(ctx.destination.is_none());
    }

    #[tokio::test]
    async fn first_match_wins() {
        let mw = MapRemoteMiddleware::new(vec![
            rule("api.local", "http://first:3000"),
            rule("api.local", "http://second:3000"),
        ]);
        let mut ctx = req("api.local", "/");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.destination.as_deref(), Some("http://first:3000"));
    }

    #[tokio::test]
    async fn disabled_rule_skipped() {
        let mut r = rule("api.local", "http://should-not-match");
        r.enabled = false;
        let mw = MapRemoteMiddleware::new(vec![r]);
        let mut ctx = req("api.local", "/");
        mw.on_request(&mut ctx).await;
        assert!(ctx.destination.is_none());
    }

    #[tokio::test]
    async fn path_filter_narrows_match() {
        let mw = MapRemoteMiddleware::new(vec![MapRemoteRule {
            id: "t".into(),
            name: "t".into(),
            enabled: true,
            location: Location {
                host: Some("api.local".into()),
                path: Some("/v2/*".into()),
                ..Default::default()
            },
            destination: "http://v2-backend:3000".into(),
        }]);
        let mut hit = req("api.local", "/v2/users");
        mw.on_request(&mut hit).await;
        assert_eq!(hit.destination.as_deref(), Some("http://v2-backend:3000"));

        let mut miss = req("api.local", "/v1/users");
        mw.on_request(&mut miss).await;
        assert!(miss.destination.is_none());
    }
}
