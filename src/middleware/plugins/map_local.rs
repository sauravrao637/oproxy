//! Path-aware Map Local middleware.
//!
//! Unlike the old host-keyed HashMap, each [`MapLocalRule`] uses a full
//! [`Location`] (host + path + method…) so you can serve different fixtures
//! for different paths on the same host.
//!
//! `file_path` can point to either a **file** (always served verbatim) or a
//! **directory** (the request path is appended and the resulting file is
//! served, after path-traversal checks).
//!
//! When a base path is configured, relative paths are resolved from it;
//! absolute paths work as before for backward compatibility.

use crate::middleware::matcher::{Location, MatchTarget};
use crate::middleware::{InterceptedResponse, Middleware, MiddlewareAction, RequestContext};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapLocalRule {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub location: Location,
    /// Absolute path to a file or directory on disk.
    pub file_path: String,
    /// Transient: inline fixture content supplied on create/update. When set, the
    /// API writes it to the managed `storage/map-local/` directory under
    /// `file_path` (treated as a single file name) and clears this field. It is
    /// never persisted to storage nor echoed back in API responses.
    #[serde(default, skip_serializing)]
    pub inline_body: Option<String>,
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

/// Resolve a Map Local `file_path` to a concrete path on disk.
///
/// - Absolute paths are used verbatim.
/// - Relative paths are resolved by trying, in order: the user-configured
///   `base_path` (a mounted host directory) and then the managed `fixtures_dir`
///   (`storage/map-local/`, where UI uploads land). The first candidate that
///   exists wins, so both a mounted directory and managed uploads work without
///   the user knowing which one a given name lives in.
/// - If neither candidate exists yet, the managed `fixtures_dir` is preferred
///   for the returned (non-existent) default so error messages point at the
///   place new files should go.
///
/// This is a free function so the API validation layer and the middleware share
/// identical resolution semantics.
pub fn resolve_map_local_path(
    file_path: &str,
    base_path: Option<&Path>,
    fixtures_dir: Option<&Path>,
) -> PathBuf {
    let path = Path::new(file_path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    // Prefer an existing candidate: mounted base dir first, then managed fixtures.
    if let Some(base) = base_path {
        let candidate = base.join(file_path);
        if candidate.exists() {
            return candidate;
        }
    }
    if let Some(fx) = fixtures_dir {
        let candidate = fx.join(file_path);
        if candidate.exists() {
            return candidate;
        }
    }
    // Nothing exists yet — pick a sensible default for error messages.
    if let Some(fx) = fixtures_dir {
        fx.join(file_path)
    } else if let Some(base) = base_path {
        base.join(file_path)
    } else {
        path.to_path_buf()
    }
}

pub struct MapLocalMiddleware {
    pub rules: SharedMapLocalRules,
    pub base_path: Option<PathBuf>,
    /// Managed fixtures directory (`storage/map-local/`) where UI uploads land.
    pub fixtures_dir: Option<PathBuf>,
}

impl MapLocalMiddleware {
    #[cfg(test)]
    pub fn new(rules: Vec<MapLocalRule>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
            base_path: None,
            fixtures_dir: None,
        }
    }

    #[cfg(test)]
    pub fn with_base_path(rules: Vec<MapLocalRule>, base_path: Option<PathBuf>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
            base_path,
            fixtures_dir: None,
        }
    }

    pub fn with_dirs(
        rules: Vec<MapLocalRule>,
        base_path: Option<PathBuf>,
        fixtures_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
            base_path,
            fixtures_dir,
        }
    }

    fn resolve_path(&self, file_path: &str) -> PathBuf {
        resolve_map_local_path(
            file_path,
            self.base_path.as_deref(),
            self.fixtures_dir.as_deref(),
        )
    }

    fn path_for_request(&self, root: &Path, request_path: &str) -> Result<Option<PathBuf>, String> {
        if !root.exists() {
            return Err(format!("root path '{}' does not exist", root.display()));
        }
        if root.is_dir() {
            let candidate = root.join(request_path.trim_start_matches('/'));
            let Ok(resolved) = candidate.canonicalize() else {
                return Ok(None);
            };
            let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            return Ok(
                (resolved.starts_with(canonical_root) && resolved.is_file()).then_some(resolved)
            );
        }
        match root.canonicalize() {
            Ok(path) if path.is_file() => Ok(Some(path)),
            Ok(_) => Err(format!("'{}' is not a regular file", root.display())),
            Err(error) => Err(format!("'{}' is not accessible: {error}", root.display())),
        }
    }
}

fn map_local_error(message: String) -> InterceptedResponse {
    InterceptedResponse {
        status: 502,
        headers: error_headers(),
        body: Bytes::from(format!("map_local: {message}")),
        tags: vec!["map-local-error".to_string()],
        served_mock: None,
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
            let file_path = self.resolve_path(&rule.file_path);
            let path_to_serve = match self.path_for_request(&file_path, &target.path) {
                Ok(Some(path)) => path,
                Ok(None) => continue,
                Err(message) => {
                    tracing::warn!(path=%file_path.display(), %message, "map_local resolution failed");
                    ctx.mock_response = Some(map_local_error(message));
                    return MiddlewareAction::StopAndReturn;
                }
            };
            match tokio::fs::read(&path_to_serve).await {
                Ok(contents) => {
                    let ct = mime_for_path(&path_to_serve);
                    let mut headers = crate::middleware::HeaderMap::new();
                    headers.insert("Content-Type".to_string(), ct.to_string());
                    headers.insert("Content-Length".to_string(), contents.len().to_string());
                    ctx.mock_response = Some(InterceptedResponse {
                        status: 200,
                        headers,
                        body: Bytes::from(contents),
                        tags: vec!["map-local".to_string()],
                        served_mock: None,
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

fn error_headers() -> crate::middleware::HeaderMap {
    let mut h = crate::middleware::HeaderMap::new();
    h.insert("content-type".to_string(), "text/plain".to_string());
    h
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
    use crate::middleware::{HeaderMap, Middleware, MiddlewareAction};
    use bytes::Bytes;

    fn req(host: &str, path: &str) -> RequestContext {
        RequestContext {
            method: "GET".into(),
            host: host.into(),
            uri: path.into(),
            headers: HeaderMap::new(),
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
            inline_body: None,
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

    #[tokio::test]
    async fn base_path_resolves_relative_paths() {
        let base_dir = std::env::temp_dir().join("map_local_base_test");
        tokio::fs::create_dir_all(&base_dir).await.unwrap();
        tokio::fs::write(base_dir.join("api.json"), b"[1,2,3]")
            .await
            .unwrap();

        let mw = MapLocalMiddleware::with_base_path(
            vec![rule("local.test", None, "api.json")],
            Some(base_dir.clone()),
        );
        let mut ctx = req("local.test", "/any");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        let mock = ctx.mock_response.unwrap();
        assert_eq!(&mock.body[..], b"[1,2,3]");
        let _ = tokio::fs::remove_dir_all(&base_dir).await;
    }

    #[tokio::test]
    async fn fixtures_dir_resolves_managed_uploads() {
        let fx = std::env::temp_dir().join("map_local_fixtures_test");
        tokio::fs::create_dir_all(&fx).await.unwrap();
        tokio::fs::write(fx.join("users.json"), b"{\"u\":1}")
            .await
            .unwrap();

        // No base path; relative name should resolve from the fixtures dir.
        let mw = MapLocalMiddleware::with_dirs(
            vec![rule("local.test", None, "users.json")],
            None,
            Some(fx.clone()),
        );
        let mut ctx = req("local.test", "/any");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        let mock = ctx.mock_response.unwrap();
        assert_eq!(&mock.body[..], b"{\"u\":1}");
        let _ = tokio::fs::remove_dir_all(&fx).await;
    }

    #[tokio::test]
    async fn base_path_takes_precedence_over_fixtures_when_present() {
        let base = std::env::temp_dir().join("map_local_prec_base");
        let fx = std::env::temp_dir().join("map_local_prec_fx");
        tokio::fs::create_dir_all(&base).await.unwrap();
        tokio::fs::create_dir_all(&fx).await.unwrap();
        tokio::fs::write(base.join("same.json"), b"from-base")
            .await
            .unwrap();
        tokio::fs::write(fx.join("same.json"), b"from-fixtures")
            .await
            .unwrap();

        let mw = MapLocalMiddleware::with_dirs(
            vec![rule("local.test", None, "same.json")],
            Some(base.clone()),
            Some(fx.clone()),
        );
        let mut ctx = req("local.test", "/any");
        assert_eq!(
            mw.on_request(&mut ctx).await,
            MiddlewareAction::StopAndReturn
        );
        assert_eq!(&ctx.mock_response.unwrap().body[..], b"from-base");

        // When the name only exists in fixtures, it falls back to fixtures.
        let mw2 = MapLocalMiddleware::with_dirs(
            vec![rule("local.test", None, "same.json")],
            Some(std::env::temp_dir().join("map_local_prec_missing")),
            Some(fx.clone()),
        );
        let mut ctx2 = req("local.test", "/any");
        assert_eq!(
            mw2.on_request(&mut ctx2).await,
            MiddlewareAction::StopAndReturn
        );
        assert_eq!(&ctx2.mock_response.unwrap().body[..], b"from-fixtures");

        let _ = tokio::fs::remove_dir_all(&base).await;
        let _ = tokio::fs::remove_dir_all(&fx).await;
    }

    #[tokio::test]
    async fn base_path_still_allows_absolute_paths() {
        let base_dir = std::env::temp_dir().join("map_local_base_abs");
        tokio::fs::create_dir_all(&base_dir).await.unwrap();
        let abs_file = base_dir.join("absolute.json");
        tokio::fs::write(&abs_file, b"absolute").await.unwrap();

        let mw = MapLocalMiddleware::with_base_path(
            vec![rule("local.test", None, abs_file.to_str().unwrap())],
            Some(base_dir.clone()),
        );
        let mut ctx = req("local.test", "/any");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        let mock = ctx.mock_response.unwrap();
        assert_eq!(&mock.body[..], b"absolute");
        let _ = tokio::fs::remove_dir_all(&base_dir).await;
    }
}
