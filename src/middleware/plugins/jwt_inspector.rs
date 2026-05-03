use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;

use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use crate::session::JwtInfo;

pub struct JwtInspectorMiddleware;

impl JwtInspectorMiddleware {
    pub fn extract_jwt(headers: &std::collections::HashMap<String, String>) -> Option<String> {
        // Authorization: Bearer <token>
        if let Some(auth) = headers.get("authorization") {
            let lower = auth.to_lowercase();
            if lower.starts_with("bearer ") {
                let token = auth[7..].trim().to_string();
                if !token.is_empty() {
                    return Some(token);
                }
            }
        }
        // Cookie header — look for token=, jwt=, access_token=
        if let Some(cookie) = headers.get("cookie") {
            for part in cookie.split(';') {
                let part = part.trim();
                for name in &["token", "jwt", "access_token"] {
                    let prefix = format!("{}=", name);
                    if part.starts_with(&prefix) {
                        let val = part[prefix.len()..].trim().to_string();
                        if !val.is_empty() {
                            return Some(val);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn decode_jwt(token: &str) -> Option<JwtInfo> {
        let parts: Vec<&str> = token.splitn(3, '.').collect();
        if parts.len() != 3 {
            return None;
        }
        let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).ok()?;
        let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;

        let header: serde_json::Value = serde_json::from_slice(&header_bytes).ok()?;
        let claims: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;

        let alg_none_warning = header
            .get("alg")
            .and_then(|a| a.as_str())
            .map(|a| a.eq_ignore_ascii_case("none"))
            .unwrap_or(false);

        let expired = claims
            .get("exp")
            .and_then(|e| e.as_i64())
            .map(|exp| exp < Utc::now().timestamp())
            .unwrap_or(false);

        Some(JwtInfo {
            header,
            claims,
            expired,
            alg_none_warning,
        })
    }
}

#[async_trait]
impl Middleware for JwtInspectorMiddleware {
    fn name(&self) -> &str { "JwtInspectorMiddleware" }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        if let Some(token) = Self::extract_jwt(&ctx.headers) {
            if let Some(info) = Self::decode_jwt(&token) {
                if let Ok(json) = serde_json::to_string(&info) {
                    ctx.headers.insert("x-oproxy-jwt".to_string(), json);
                }
            }
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use std::collections::HashMap;

    fn make_jwt(header: &str, payload: &str) -> String {
        let h = URL_SAFE_NO_PAD.encode(header.as_bytes());
        let p = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("{}.{}.fakesig", h, p)
    }

    #[test]
    fn extract_jwt_from_bearer_header() {
        let mut headers = HashMap::new();
        headers.insert(
            "authorization".to_string(),
            "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.sig".to_string(),
        );
        let token = JwtInspectorMiddleware::extract_jwt(&headers);
        assert!(token.is_some());
        assert!(token.unwrap().starts_with("eyJ"));
    }

    #[test]
    fn extract_jwt_from_cookie_token() {
        let mut headers = HashMap::new();
        headers.insert(
            "cookie".to_string(),
            "session=abc; token=mytoken123; other=val".to_string(),
        );
        let token = JwtInspectorMiddleware::extract_jwt(&headers);
        assert_eq!(token.as_deref(), Some("mytoken123"));
    }

    #[test]
    fn extract_jwt_from_cookie_jwt_name() {
        let mut headers = HashMap::new();
        headers.insert("cookie".to_string(), "jwt=tok.en.val".to_string());
        let token = JwtInspectorMiddleware::extract_jwt(&headers);
        assert_eq!(token.as_deref(), Some("tok.en.val"));
    }

    #[test]
    fn extract_jwt_from_cookie_access_token() {
        let mut headers = HashMap::new();
        headers.insert("cookie".to_string(), "access_token=at.123.sig".to_string());
        let token = JwtInspectorMiddleware::extract_jwt(&headers);
        assert_eq!(token.as_deref(), Some("at.123.sig"));
    }

    #[test]
    fn no_jwt_returns_none() {
        let headers = HashMap::new();
        assert!(JwtInspectorMiddleware::extract_jwt(&headers).is_none());
    }

    #[test]
    fn decode_valid_jwt_parses_header_and_claims() {
        let token = make_jwt(r#"{"alg":"HS256","typ":"JWT"}"#, r#"{"sub":"user","iss":"test"}"#);
        let info = JwtInspectorMiddleware::decode_jwt(&token).unwrap();
        assert_eq!(info.header["alg"], "HS256");
        assert_eq!(info.claims["sub"], "user");
        assert!(!info.alg_none_warning);
        assert!(!info.expired);
    }

    #[test]
    fn decode_jwt_detects_alg_none() {
        let token = make_jwt(r#"{"alg":"none"}"#, r#"{"sub":"x"}"#);
        let info = JwtInspectorMiddleware::decode_jwt(&token).unwrap();
        assert!(info.alg_none_warning);
    }

    #[test]
    fn decode_jwt_detects_expired() {
        // exp in the past
        let token = make_jwt(r#"{"alg":"HS256"}"#, r#"{"exp":1000000}"#);
        let info = JwtInspectorMiddleware::decode_jwt(&token).unwrap();
        assert!(info.expired);
    }

    #[test]
    fn decode_jwt_not_expired_for_future() {
        // exp far in the future
        let token = make_jwt(r#"{"alg":"HS256"}"#, r#"{"exp":9999999999}"#);
        let info = JwtInspectorMiddleware::decode_jwt(&token).unwrap();
        assert!(!info.expired);
    }

    #[test]
    fn invalid_jwt_returns_none() {
        assert!(JwtInspectorMiddleware::decode_jwt("notajwt").is_none());
        assert!(JwtInspectorMiddleware::decode_jwt("a.b").is_none());
        assert!(JwtInspectorMiddleware::decode_jwt("!!!.!!!.!!!").is_none());
    }
}
