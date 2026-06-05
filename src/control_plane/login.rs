use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;

use super::auth::{encode_next_param, token_matches};

// ─── Login page HTML ──────────────────────────────────────────────────────────

static LOGIN_TEMPLATE: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Sign in — oproxy</title>
  <style>
    :root { color-scheme: dark; }
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
      background: #0f1117; color: #e2e8f0;
      min-height: 100vh; display: flex; align-items: center; justify-content: center;
    }
    .card {
      background: #1a1f2e; border: 1px solid #2d3348;
      border-radius: 12px; padding: 2.5rem 2rem; width: 100%; max-width: 360px;
      box-shadow: 0 8px 32px rgb(0 0 0 / .4);
    }
    .logo { display: flex; align-items: center; gap: .6rem; margin-bottom: 2rem; }
    .logo-mark {
      width: 34px; height: 34px; border-radius: 8px;
      background: linear-gradient(135deg, #6366f1, #8b5cf6);
      display: flex; align-items: center; justify-content: center;
      font-weight: 700; font-size: 13px; color: #fff; letter-spacing: -.5px;
      flex-shrink: 0;
    }
    .logo h1 { font-size: 1.1rem; font-weight: 600; color: #f1f5f9; }
    .logo .sub { font-size: .8rem; color: #64748b; margin-top: 1px; }
    label { display: block; font-size: .8rem; color: #94a3b8; margin-bottom: .4rem; letter-spacing: .02em; }
    input[type=password] {
      width: 100%; padding: .6rem .75rem;
      background: #0f1117; border: 1px solid #2d3348; border-radius: 6px;
      color: #e2e8f0; font-size: .9rem; outline: none; margin-bottom: 1rem;
      transition: border-color .15s, box-shadow .15s;
    }
    input[type=password]:focus {
      border-color: #6366f1;
      box-shadow: 0 0 0 3px rgb(99 102 241 / .18);
    }
    button[type=submit] {
      width: 100%; padding: .65rem; background: #6366f1;
      border: none; border-radius: 6px; color: #fff; font-size: .9rem;
      font-weight: 500; cursor: pointer; transition: background .15s;
    }
    button[type=submit]:hover { background: #4f51c8; }
    button[type=submit]:active { background: #4345b0; }
    .error {
      background: rgb(127 29 29 / .25); border: 1px solid rgb(185 28 28 / .5);
      border-radius: 6px; padding: .6rem .75rem; color: #fca5a5;
      font-size: .85rem; margin-bottom: 1rem;
    }
    .hint {
      font-size: .75rem; color: #475569; margin-top: .9rem; text-align: center;
      line-height: 1.5;
    }
    .hint code {
      background: #0f1117; border: 1px solid #1e293b;
      border-radius: 4px; padding: 1px 5px; font-size: .8em; color: #94a3b8;
    }
  </style>
</head>
<body>
  <div class="card">
    <div class="logo">
      <div class="logo-mark">op</div>
      <div>
        <h1>oproxy</h1>
        <div class="sub">Sign in to continue</div>
      </div>
    </div>
    {{ERROR}}
    <form method="POST" action="/login">
      <input type="hidden" name="next" value="{{NEXT}}">
      <label for="token">Admin token</label>
      <input type="password" id="token" name="token"
             autocomplete="current-password" placeholder="Paste your token" autofocus>
      <button type="submit">Sign in</button>
    </form>
    <p class="hint">
      Token is configured via <code>OPROXY_ADMIN_TOKEN</code> at startup.
    </p>
  </div>
</body>
</html>"#;

// ─── Handlers ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct LoginForm {
    pub token: String,
    pub next: Option<String>,
}

pub(super) async fn get_login(
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> axum::response::Html<String> {
    let raw_next = params.get("next").map(String::as_str).unwrap_or("/");
    let next = html_escape(sanitize_next(raw_next));
    let error_block = if params.contains_key("error") {
        r#"<div class="error">Invalid token — please try again.</div>"#
    } else {
        ""
    };
    axum::response::Html(
        LOGIN_TEMPLATE
            .replace("{{NEXT}}", &next)
            .replace("{{ERROR}}", error_block),
    )
}

pub(super) async fn post_login(
    State(state): State<Arc<AppState>>,
    axum::extract::Form(form): axum::extract::Form<LoginForm>,
) -> axum::response::Response {
    let expected = state
        .config
        .admin_token
        .as_deref()
        .map(str::trim)
        .unwrap_or("");
    let submitted = form.token.trim();

    // Validate using the same constant-time comparison as the auth middleware.
    if !expected.is_empty() && token_matches(submitted, expected) {
        // Success — set the session cookie and redirect to the originally requested page.
        let destination = sanitize_next(form.next.as_deref().unwrap_or("/"));
        let cookie = format!("oproxy_admin_token={submitted}; HttpOnly; SameSite=Strict; Path=/");
        (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, destination.to_string()),
                (header::SET_COOKIE, cookie),
            ],
        )
            .into_response()
    } else {
        // Wrong token — bounce back to the login page with an error indicator.
        let next_raw = form.next.as_deref().unwrap_or("/");
        let next_encoded = encode_next_param(sanitize_next(next_raw));
        (
            StatusCode::SEE_OTHER,
            [(
                header::LOCATION,
                format!("/login?error=1&next={next_encoded}"),
            )],
        )
            .into_response()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Only allow paths that start with `/` but not `//` (open-redirect guard).
/// Redirecting to `/login` would create a loop, so we fall back to `/` there.
fn sanitize_next(next: &str) -> &str {
    if next.starts_with('/') && !next.starts_with("//") && next != "/login" {
        next
    } else {
        "/"
    }
}

/// Minimal HTML escaping for safely injecting values into HTML attribute values.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}
