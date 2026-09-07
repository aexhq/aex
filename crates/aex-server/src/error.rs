use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use brain_protocol::ApiError;

pub struct Error(pub StatusCode, pub ApiError);
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn invalid(message: &str) -> Self {
        Self(StatusCode::BAD_REQUEST, ApiError::invalid_request(message))
    }
    pub fn denied() -> Self {
        Self(
            StatusCode::UNAUTHORIZED,
            ApiError::unauthorized("valid active credentials required"),
        )
    }
    pub fn missing() -> Self {
        Self(
            StatusCode::NOT_FOUND,
            ApiError::not_found("resource not found"),
        )
    }
    pub fn conflict(message: &str) -> Self {
        Self(StatusCode::CONFLICT, ApiError::conflict(message))
    }
    pub fn capacity() -> Self {
        Self(
            StatusCode::SERVICE_UNAVAILABLE,
            ApiError::overloaded("hosted capacity unavailable"),
        )
    }
    pub fn ambiguous() -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::ambiguous(
                "outcome unresolved; operator reconciliation required; do not use a new operation key",
            ),
        )
    }
    pub fn internal() -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::internal("operation failed"),
        )
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(kind = "product_store", error = %error, "product operation failed");
        Self::internal()
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::invalid("invalid JSON request")
    }
}
impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Error")
            .field(&self.0)
            .field(&self.1.code)
            .finish()
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.0, self.1.message)
    }
}
impl std::error::Error for Error {}
