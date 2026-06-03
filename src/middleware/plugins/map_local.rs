//! Path-aware Map Local middleware.
//!
//! Unlike the old host-keyed HashMap, each [`MapLocalRule`] uses a full
//! [`Location`] (host + path + method…) so you can serve different fixtures
//! for different paths on the same host.
//!
//! `file_path` can point to either a **file** (always served verbatim) or a
//! **directory** (the request path is appended and the resulting file is
//! served, after path-traversal checks).

use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{InterceptedResponse, Middleware, MiddlewareAction, RequestContext};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapLocalRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub location: Location,
    /// Absolute path to a file or directory on disk.
    pub file_path: String,
}

fn default_true() -> bool {
    true
}

impl MapLocalRule {
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }
}

pub type SharedMapLocalRules = Arc<RwLock<Vec<MapLocalRule>>>;

pub struct MapLocalMiddleware {
    pub rules: SharedMapLocalRules,
}

impl MapLocalMiddleware {
    pub fn new(rules: Vec<MapLocalRule>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
        }
    }
}

#[async_trait]
impl Middleware for MapLocalMiddleware {
    fn name(&self) -> &str {
        "MapLocalMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let rules = self.rules.read().await;
        let target = MatchTarget::from_request(ctx);
        for rule in rules.iter().filter(|r| r.enabled) {
            if !rule.location.matches(&target) {
                continue;
            }
            let file_path = Path::new(&rule.file_path);
            let path_to_serve = if file_path.is_dir() {
                // Strip leading '?' or '#' from uri, take the path component only.
                let req_path = target.path.trim_start_matches('/');
                let candidate = file_path.join(req_path);
                // Path-traversal guard: the resolved path must remain inside file_path.
                match candidate.canonicalize() {
                    Ok(resolved) => {
                        let root = file_path
                            .canonicalize()
                            .unwrap_or_else(|_| file_path.to_path_buf());
                        if !resolved.starts_with(&root) || !resolved.is_file() {
                            tracing::warn!(
                                candidate = %candidate.display(),
                                "map_local: path traversal or missing file, skipping"
                            );
                            continue;
                        }
                        resolved
                    }
                    Err(e) => {
                        tracing::debug!(path=%candidate.display(), error=%e, "map_local: file not found in dir");
                        continue;
                    }
                }
            } else {
                match file_path.canonicalize() {
                    Ok(p) if p.is_file() => p,
                    _ => {
                        tracing::warn!(path=%file_path.display(), "map_local: file not found or not a file");
                        continue;
                    }
                }
            };
            match tokio::fs::read(&path_to_serve).await {
                Ok(contents) => {
                    let ct = mime_for_path(&path_to_serve);
                    let mut headers = HashMap::new();
                    headers.insert("Content-Type".to_string(), ct.to_string());
                    headers.insert("Content-Length".to_string(), contents.len().to_string());
                    ctx.mock_response = Some(InterceptedResponse {
                        status: 200,
                        headers,
                        body: Bytes::from(contents),
                        tags: vec!["map-local".to_string()],
                    });
                    return MiddlewareAction::StopAndReturn;
                }
                Err(e) => {
                    tracing::warn!(path=%path_to_serve.display(), error=%e, "map_local: read failed");
                }
            }
        }
        MiddlewareAction::Continue
    }
}

fn mime_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => "application/json",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "application/javascript",
        Some("ts") => "application/typescript",
        Some("css") => "text/css",
        Some("xml") => "application/xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        Some("pdf") => "application/pdf",
        Some("gz") => "application/gzip",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::matcher::Location;
    use crate::middleware::{Middleware, MiddlewareAction};
    use bytes::Bytes;
    use std::collections::HashMap;

    fn req(host: &str, path: &str) -> RequestContext {
        RequestContext {
            method: "GET".into(),
            host: host.into(),
            uri: path.into(),
            headers: HashMap::new(),
            body: Bytes::new(),
            ..Default::default()
        }
    }

    fn rule(host: &str, path_pattern: Option<&str>, file_path: &str) -> MapLocalRule {
        MapLocalRule {
            id: "t".into(),
            name: "t".into(),
            enabled: true,
            location: Location {
                host: Some(host.into()),
                path: path_pattern.map(|p| p.into()),
                ..Default::default()
            },
            file_path: file_path.into(),
        }
    }

    #[tokio::test]
    async fn serves_file_for_matching_host() {
        let tmp = std::env::temp_dir().join("map_local_test_file.txt");
        tokio::fs::write(&tmp, b"hello map local").await.unwrap();
        let mw = MapLocalMiddleware::new(vec![rule("local.mock", None, tmp.to_str().unwrap())]);
        let mut ctx = req("local.mock", "/any");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        let mock = ctx.mock_response.unwrap();
        assert_eq!(&mock.body[..], b"hello map local");
        assert!(mock.tags.contains(&"map-local".to_string()));
        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[tokio::test]
    async fn skips_when_host_does_not_match() {
        let tmp = std::env::temp_dir().join("map_local_test_skip.txt");
        tokio::fs::write(&tmp, b"content").await.unwrap();
        let mw = MapLocalMiddleware::new(vec![rule("specific.host", None, tmp.to_str().unwrap())]);
        let mut ctx = req("other.host", "/any");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[tokio::test]
    async fn serves_from_directory_by_request_path() {
        let dir = std::env::temp_dir().join("map_local_test_dir");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("hello.json"), b"{}")
            .await
            .unwrap();
        let mw = MapLocalMiddleware::new(vec![rule("dir.mock", None, dir.to_str().unwrap())]);
        let mut ctx = req("dir.mock", "/hello.json");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        let mock = ctx.mock_response.unwrap();
        assert_eq!(&mock.body[..], b"{}");
        assert_eq!(
            mock.headers.get("Content-Type").map(String::as_str),
            Some("application/json")
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn directory_missing_path_falls_through() {
        let dir = std::env::temp_dir().join("map_local_test_dir_miss");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let mw = MapLocalMiddleware::new(vec![rule("dir.mock", None, dir.to_str().unwrap())]);
        let mut ctx = req("dir.mock", "/nonexistent.txt");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn path_pattern_narrows_match() {
        let tmp = std::env::temp_dir().join("map_local_test_path.json");
        tokio::fs::write(&tmp, b"[]").await.unwrap();
        let mw = MapLocalMiddleware::new(vec![rule("h", Some("/api/*"), tmp.to_str().unwrap())]);
        // path matches
        let mut ctx1 = req("h", "/api/users");
        assert_eq!(
            mw.on_request(&mut ctx1).await,
            MiddlewareAction::StopAndReturn
        );
        // path does not match
        let mut ctx2 = req("h", "/static/x.js");
        assert_eq!(mw.on_request(&mut ctx2).await, MiddlewareAction::Continue);
        let _ = tokio::fs::remove_file(&tmp).await;
    }
}
