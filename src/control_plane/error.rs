//! Canonical error type for control-plane HTTP handlers.
//!
//! Instead of building ad-hoc `(StatusCode, message)` tuples, a handler can
//! return `Result<T, ApiError>` and use `?` for its failure paths:
//!
//! ```ignore
//! async fn get_rule(id: Path<String>) -> Result<Json<Rule>, ApiError> {
//!     let rule = store.get(&id).ok_or_else(|| ApiError::NotFound("no such rule".into()))?;
//!     Ok(Json(rule))
//! }
//! ```
//!
//! [`ApiError`] implements [`IntoResponse`] by serialising a consistent JSON
//! body `{ "error": "..." }` with the mapped status code, so every endpoint
//! reports failures the same way.
//!
//! Adoption is incremental — handlers are migrated onto this type over time, so
//! not every variant is wired into a caller yet (hence the `dead_code` allow).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// A control-plane failure mapped to an HTTP status code and a message.
#[derive(Debug)]
#[allow(dead_code)] // canonical taxonomy; handlers adopt variants incrementally
pub(super) enum ApiError {
    /// 400 — the request was malformed or failed validation.
    BadRequest(String),
    /// 401 — authentication is required or failed.
    Unauthorized(String),
    /// 403 — the caller is authenticated but not permitted.
    Forbidden(String),
    /// 404 — the addressed resource does not exist.
    NotFound(String),
    /// 422 — the request was well-formed but semantically invalid.
    Unprocessable(String),
    /// 500 — an unexpected server-side failure.
    Internal(String),
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &str) {
        match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Unprocessable(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = self.parts();
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_variant_maps_to_its_status() {
        let cases = [
            (ApiError::BadRequest("x".into()), StatusCode::BAD_REQUEST),
            (ApiError::Unauthorized("x".into()), StatusCode::UNAUTHORIZED),
            (ApiError::Forbidden("x".into()), StatusCode::FORBIDDEN),
            (ApiError::NotFound("x".into()), StatusCode::NOT_FOUND),
            (
                ApiError::Unprocessable("x".into()),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                ApiError::Internal("x".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.parts().0, expected);
        }
    }

    #[test]
    fn renders_json_error_body() {
        let resp = ApiError::NotFound("missing".into()).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
