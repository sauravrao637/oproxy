use async_trait::async_trait;
use bytes::Bytes;
use mlua::{Lua, VmState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};

const LUA_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LuaScript {
    pub id: String,
    pub name: String,
    pub code: String,
    pub enabled: bool,
}

pub type SharedLuaScripts = Arc<RwLock<Vec<LuaScript>>>;

pub struct LuaEngineMiddleware {
    pub scripts: SharedLuaScripts,
}

impl LuaEngineMiddleware {
    pub fn new(scripts: SharedLuaScripts) -> Self {
        Self { scripts }
    }
}

/// Create a sandboxed Lua state — remove dangerous globals.
fn make_sandbox() -> mlua::Result<Lua> {
    let lua = Lua::new();
    {
        let globals = lua.globals();
        for name in &[
            "io",
            "os",
            "package",
            "require",
            "load",
            "loadfile",
            "dofile",
            "debug",
            "coroutine",
        ] {
            globals.raw_remove(*name)?;
        }
        // Limit string.rep to prevent memory exhaustion (e.g. string.rep("x", 2^30)).
        let string_table: mlua::Table = globals.get("string")?;
        string_table.set(
            "rep",
            lua.create_function(|_, (s, n, sep): (String, usize, Option<String>)| {
                let sep = sep.unwrap_or_default();
                let out = s.len() * n + sep.len().saturating_mul(n.saturating_sub(1));
                if out > 1_048_576 {
                    return Err(mlua::Error::RuntimeError(
                        "string.rep: output exceeds 1 MiB limit".into(),
                    ));
                }
                let mut result = String::with_capacity(out);
                for i in 0..n {
                    if i > 0 {
                        result.push_str(&sep);
                    }
                    result.push_str(&s);
                }
                Ok(result)
            })?,
        )?;
    }
    Ok(lua)
}

/// Execute Lua code with a timeout enforced via the debug hook.
fn exec_with_timeout(lua: &Lua, code: &str) -> mlua::Result<()> {
    let deadline = Instant::now() + LUA_TIMEOUT;
    let _ = lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(1000),
        move |_lua, _debug| {
            if Instant::now() >= deadline {
                Err(mlua::Error::RuntimeError("script timeout".into()))
            } else {
                Ok(VmState::Continue)
            }
        },
    );
    let res = lua.load(code).exec();
    lua.remove_hook();
    res
}

/// Inject request data into Lua globals.
fn inject_request(
    lua: &Lua,
    method: &str,
    uri: &str,
    body: &str,
    headers: &HashMap<String, String>,
) -> mlua::Result<()> {
    let request = lua.create_table()?;
    request.set("method", method)?;
    request.set("uri", uri)?;
    request.set("body", body)?;

    let header_table = lua.create_table()?;
    for (k, v) in headers {
        header_table.set(k.clone(), v.clone())?;
    }
    request.set("headers", header_table)?;
    lua.globals().set("request", request)?;
    Ok(())
}

/// Extract modified request data from Lua globals.
fn extract_request(
    lua: &Lua,
    body: &mut String,
    headers: &mut HashMap<String, String>,
) -> mlua::Result<()> {
    let request: mlua::Table = lua.globals().get("request")?;

    if let Ok(new_body) = request.get::<String>("body") {
        *body = new_body;
    }
    let header_table: mlua::Table = request.get("headers")?;
    for (k, v) in header_table.pairs::<String, String>().flatten() {
        headers.insert(k, v);
    }
    Ok(())
}

/// Inject response data into Lua globals.
fn inject_response(
    lua: &Lua,
    status: u16,
    body: &str,
    headers: &HashMap<String, String>,
) -> mlua::Result<()> {
    let response = lua.create_table()?;
    response.set("status", status)?;
    response.set("body", body)?;

    let header_table = lua.create_table()?;
    for (k, v) in headers {
        header_table.set(k.clone(), v.clone())?;
    }
    response.set("headers", header_table)?;
    lua.globals().set("response", response)?;
    Ok(())
}

/// Extract modified response data from Lua globals.
fn extract_response(
    lua: &Lua,
    status: &mut u16,
    body: &mut String,
    headers: &mut HashMap<String, String>,
) -> mlua::Result<()> {
    let response: mlua::Table = lua.globals().get("response")?;
    if let Ok(new_status) = response.get::<u16>("status") {
        *status = new_status;
    }
    if let Ok(new_body) = response.get::<String>("body") {
        *body = new_body;
    }
    let header_table: mlua::Table = response.get("headers")?;
    for (k, v) in header_table.pairs::<String, String>().flatten() {
        headers.insert(k, v);
    }
    Ok(())
}

struct RequestScriptOutcome {
    body: String,
    headers: HashMap<String, String>,
    abort: Option<(u16, String)>,
}

/// Runs the enabled request scripts synchronously. Intended to be called from
/// `spawn_blocking` so the Lua VM never blocks a Tokio worker thread.
fn run_request_scripts(
    scripts: Vec<LuaScript>,
    method: String,
    uri: String,
    mut body: String,
    mut headers: HashMap<String, String>,
) -> RequestScriptOutcome {
    for script in scripts {
        let lua = match make_sandbox() {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Lua sandbox init failed: {e}");
                continue;
            }
        };
        if let Err(e) = setup_log(&lua) {
            tracing::warn!("Lua log setup failed: {e}");
            continue;
        }
        if let Err(e) = setup_abort(&lua) {
            tracing::warn!("Lua abort setup failed: {e}");
            continue;
        }
        if let Err(e) = inject_request(&lua, &method, &uri, &body, &headers) {
            tracing::warn!("Lua inject failed: {e}");
            continue;
        }
        if let Err(e) = exec_with_timeout(&lua, &script.code) {
            tracing::warn!(script = %script.name, "Lua exec error: {e}");
            continue;
        }
        if let Some((status, abort_body)) = check_abort(&lua) {
            return RequestScriptOutcome {
                body,
                headers,
                abort: Some((status, abort_body)),
            };
        }
        if let Err(e) = extract_request(&lua, &mut body, &mut headers) {
            tracing::warn!("Lua extract failed: {e}");
        }
    }
    RequestScriptOutcome {
        body,
        headers,
        abort: None,
    }
}

struct ResponseScriptOutcome {
    status: u16,
    body: String,
    headers: HashMap<String, String>,
}

/// Runs the enabled response scripts synchronously inside `spawn_blocking`.
fn run_response_scripts(
    scripts: Vec<LuaScript>,
    mut status: u16,
    mut body: String,
    mut headers: HashMap<String, String>,
) -> ResponseScriptOutcome {
    for script in scripts {
        let lua = match make_sandbox() {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Lua sandbox init failed: {e}");
                continue;
            }
        };
        if let Err(e) = setup_log(&lua) {
            tracing::warn!("{e}");
            continue;
        }
        if let Err(e) = inject_response(&lua, status, &body, &headers) {
            tracing::warn!("Lua inject resp failed: {e}");
            continue;
        }
        if let Err(e) = exec_with_timeout(&lua, &script.code) {
            tracing::warn!(script = %script.name, "Lua exec error: {e}");
            continue;
        }
        if let Err(e) = extract_response(&lua, &mut status, &mut body, &mut headers) {
            tracing::warn!("Lua extract resp failed: {e}");
        }
    }
    ResponseScriptOutcome {
        status,
        body,
        headers,
    }
}

fn setup_log(lua: &Lua) -> mlua::Result<()> {
    let log_fn = lua.create_function(|_, msg: String| {
        tracing::info!(lua = true, "{}", msg);
        Ok(())
    })?;
    lua.globals().set("log", log_fn)?;
    Ok(())
}

fn setup_abort(lua: &Lua) -> mlua::Result<()> {
    let abort_fn = lua.create_function(|lua, (status, body): (u16, String)| {
        // Signal abort by setting a special global
        lua.globals().set("__abort_status__", status)?;
        lua.globals().set("__abort_body__", body)?;
        Ok(())
    })?;
    lua.globals().set("abort", abort_fn)?;
    Ok(())
}

fn check_abort(lua: &Lua) -> Option<(u16, String)> {
    let status: Option<u16> = lua.globals().get("__abort_status__").ok();
    let body: Option<String> = lua.globals().get("__abort_body__").ok();
    match (status, body) {
        (Some(s), Some(b)) => Some((s, b)),
        _ => None,
    }
}

#[async_trait]
impl Middleware for LuaEngineMiddleware {
    fn name(&self) -> &str {
        "LuaEngineMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let scripts: Vec<LuaScript> = self
            .scripts
            .read()
            .await
            .iter()
            .filter(|s| s.enabled)
            .cloned()
            .collect();
        if scripts.is_empty() {
            return MiddlewareAction::Continue;
        }

        let method = ctx.method.clone();
        let uri = ctx.uri.clone();
        let body = ctx.body_text().into_owned();
        let headers: HashMap<String, String> = ctx.headers.clone().into();

        let outcome =
            match tokio::task::spawn_blocking(move || {
                run_request_scripts(scripts, method, uri, body, headers)
            })
            .await
            {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!(error = %e, "Lua request task failed to join");
                    return MiddlewareAction::Continue;
                }
            };

        if let Some((status, body)) = outcome.abort {
            let sc = axum::http::StatusCode::from_u16(status)
                .unwrap_or(axum::http::StatusCode::FORBIDDEN);
            let mut headers = crate::middleware::HeaderMap::new();
            headers.insert("content-type".to_string(), "text/plain".to_string());
            ctx.mock_response = Some(crate::middleware::InterceptedResponse {
                status: sc.as_u16(),
                headers,
                body: Bytes::from(body),
                tags: Vec::new(),
            });
            return MiddlewareAction::StopAndReturn;
        }

        ctx.set_body_text(outcome.body);
        ctx.headers = outcome.headers.into();
        MiddlewareAction::Continue
    }

    async fn on_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        let scripts: Vec<LuaScript> = self
            .scripts
            .read()
            .await
            .iter()
            .filter(|s| s.enabled)
            .cloned()
            .collect();
        if scripts.is_empty() {
            return MiddlewareAction::Continue;
        }

        let status = ctx.status;
        let body = ctx.body_text().into_owned();
        let headers: HashMap<String, String> = ctx.headers.clone().into();

        let outcome = match tokio::task::spawn_blocking(move || {
            run_response_scripts(scripts, status, body, headers)
        })
        .await
        {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(error = %e, "Lua response task failed to join");
                return MiddlewareAction::Continue;
            }
        };

        ctx.status = outcome.status;
        ctx.set_body_text(outcome.body);
        ctx.headers = outcome.headers.into();
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::HeaderMap;

    fn make_req(method: &str, uri: &str, body: &str) -> RequestContext {
        RequestContext {
            method: method.to_string(),
            uri: uri.to_string(),
            headers: HeaderMap::new(),
            body: Bytes::from(body.to_string()),
            host: "example.com".to_string(),
            ..Default::default()
        }
    }

    fn make_script(code: &str) -> LuaScript {
        LuaScript {
            id: "test".to_string(),
            name: "test".to_string(),
            code: code.to_string(),
            enabled: true,
        }
    }

    #[tokio::test]
    async fn script_can_add_header() {
        let script = make_script(r#"request.headers["x-test"] = "hello""#);
        let scripts = Arc::new(RwLock::new(vec![script]));
        let mw = LuaEngineMiddleware::new(scripts);
        let mut ctx = make_req("GET", "/api", "");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
        assert_eq!(ctx.headers.get("x-test").map(|s| s.as_str()), Some("hello"));
    }

    #[tokio::test]
    async fn script_can_modify_body() {
        let script = make_script(r#"request.body = "modified""#);
        let scripts = Arc::new(RwLock::new(vec![script]));
        let mw = LuaEngineMiddleware::new(scripts);
        let mut ctx = make_req("POST", "/api", "original");
        mw.on_request(&mut ctx).await;
        assert_eq!(ctx.body_text(), "modified");
    }

    #[tokio::test]
    async fn disabled_script_is_skipped() {
        let mut script = make_script(r#"request.headers["x-test"] = "hello""#);
        script.enabled = false;
        let scripts = Arc::new(RwLock::new(vec![script]));
        let mw = LuaEngineMiddleware::new(scripts);
        let mut ctx = make_req("GET", "/api", "");
        mw.on_request(&mut ctx).await;
        assert!(!ctx.headers.contains_key("x-test"));
    }

    #[tokio::test]
    async fn sandbox_prevents_io_access() {
        let script = make_script(r#"io.open("/etc/passwd", "r")"#);
        let scripts = Arc::new(RwLock::new(vec![script]));
        let mw = LuaEngineMiddleware::new(scripts);
        let mut ctx = make_req("GET", "/api", "");
        // Must not panic — sandbox should log error and continue
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
    }

    #[tokio::test]
    async fn abort_returns_stop_and_return() {
        let script = make_script(r#"abort(403, "forbidden")"#);
        let scripts = Arc::new(RwLock::new(vec![script]));
        let mw = LuaEngineMiddleware::new(scripts);
        let mut ctx = make_req("GET", "/api", "");
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        let mock = ctx.mock_response.as_ref().unwrap();
        assert_eq!(mock.status, 403);
        assert_eq!(&mock.body[..], b"forbidden");
    }

    #[tokio::test]
    async fn syntax_error_handled_gracefully() {
        let script = make_script("this is not valid lua!!!!");
        let scripts = Arc::new(RwLock::new(vec![script]));
        let mw = LuaEngineMiddleware::new(scripts);
        let mut ctx = make_req("GET", "/api", "");
        // Should not panic
        let action = mw.on_request(&mut ctx).await;
        assert_eq!(action, MiddlewareAction::Continue);
    }
}
