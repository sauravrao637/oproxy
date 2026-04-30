use async_trait::async_trait;
use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HmScope {
    All,
    Host,
    Path,
}

impl Default for HmScope {
    fn default() -> Self { HmScope::All }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HmAction {
    Set,
    Append,
    Remove,
}

impl Default for HmAction {
    fn default() -> Self { HmAction::Set }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderMapRule {
    pub id: String,
    #[serde(default)]
    pub scope: HmScope,
    #[serde(default)]
    pub r#match: String,
    #[serde(default)]
    pub action: HmAction,
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool { true }

impl HeaderMapRule {
    fn matches_request(&self, req: &RequestContext) -> bool {
        if !self.enabled { return false; }
        match self.scope {
            HmScope::All => true,
            HmScope::Host => req.host.contains(&self.r#match),
            HmScope::Path => {
                if self.r#match.is_empty() { return true; }
                Regex::new(&self.r#match).map(|re| re.is_match(&req.uri)).unwrap_or(false)
            }
        }
    }

    fn apply_to_headers(&self, headers: &mut std::collections::HashMap<String, String>) {
        let key = self.name.to_lowercase();
        match self.action {
            HmAction::Set => { headers.insert(key, self.value.clone()); }
            HmAction::Append => {
                let existing = headers.get(&key).cloned().unwrap_or_default();
                let sep = if existing.is_empty() { "" } else { ", " };
                headers.insert(key, format!("{existing}{sep}{}", self.value));
            }
            HmAction::Remove => { headers.remove(&key); }
        }
    }
}

pub struct HeaderMapMiddleware {
    pub rules: Arc<RwLock<Vec<HeaderMapRule>>>,
}

impl HeaderMapMiddleware {
    pub fn new(rules: Vec<HeaderMapRule>) -> Self {
        Self { rules: Arc::new(RwLock::new(rules)) }
    }
}

#[async_trait]
impl Middleware for HeaderMapMiddleware {
    fn name(&self) -> &'static str { "header_map" }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        for rule in rules.iter() {
            if rule.matches_request(ctx) {
                rule.apply_to_headers(&mut ctx.headers);
            }
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }
}
