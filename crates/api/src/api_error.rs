use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

/// Typed API error envelope for consistent error responses.
///
/// JSON shape: `{"error":{"code":"NOT_FOUND","message":"...","details":null}}`
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    Conflict(String),
    ValidationFailed(String),
    RateLimited { retry_after: u64 },
    Internal(String),
}

impl ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::ValidationFailed(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code_string(&self) -> &'static str {
        match self {
            ApiError::NotFound(_) => "NOT_FOUND",
            ApiError::BadRequest(_) => "BAD_REQUEST",
            ApiError::Unauthorized(_) => "UNAUTHORIZED",
            ApiError::Forbidden(_) => "FORBIDDEN",
            ApiError::Conflict(_) => "CONFLICT",
            ApiError::ValidationFailed(_) => "VALIDATION_FAILED",
            ApiError::RateLimited { .. } => "RATE_LIMITED",
            ApiError::Internal(_) => "INTERNAL",
        }
    }

    fn message(&self) -> &str {
        match self {
            ApiError::NotFound(m) => m,
            ApiError::BadRequest(m) => m,
            ApiError::Unauthorized(m) => m,
            ApiError::Forbidden(m) => m,
            ApiError::Conflict(m) => m,
            ApiError::ValidationFailed(m) => m,
            ApiError::RateLimited { .. } => "Too many requests",
            ApiError::Internal(m) => m,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status_code();
        let mut body = serde_json::json!({
            "error": {
                "code": self.code_string(),
                "message": self.message(),
                "details": null,
            }
        });

        // Include retry_after in details for rate-limited responses
        if let ApiError::RateLimited { retry_after } = &self {
            body["error"]["details"] = serde_json::json!({ "retry_after": retry_after });
        }

        (status, Json(body)).into_response()
    }
}
