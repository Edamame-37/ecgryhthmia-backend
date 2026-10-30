use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);
pub type ApiResult<T> = Result<T, ApiError>;
impl ApiError {
    pub fn bad(m: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, m.into())
    }
    pub fn unauthorized() -> Self {
        Self(StatusCode::UNAUTHORIZED, "Authentication required".into())
    }
    pub fn forbidden() -> Self {
        Self(StatusCode::FORBIDDEN, "Access denied".into())
    }
    pub fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "Resource not found".into())
    }
    pub fn conflict(m: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, m.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"success":false,"message":self.1}))).into_response()
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::RowNotFound => Self::missing(),
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                Self::conflict("Resource already exists")
            }
            _ => {
                tracing::error!(error=%e,"Database operation failed");
                Self(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Database unavailable".into(),
                )
            }
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        tracing::error!(error=%e,"Storage operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Storage operation failed".into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek};

    fn convert_with_logs(error: sqlx::Error) -> (ApiError, String) {
        let mut output = tempfile::tempfile().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.try_clone().unwrap())
            .finish();
        let response = tracing::subscriber::with_default(subscriber, || ApiError::from(error));
        output.rewind().unwrap();
        let mut logs = String::new();
        output.read_to_string(&mut logs).unwrap();
        (response, logs)
    }

    #[test]
    fn missing_rows_return_not_found_without_database_failure_logs() {
        let (response, logs) = convert_with_logs(sqlx::Error::RowNotFound);
        assert_eq!(response.0, StatusCode::NOT_FOUND);
        assert_eq!(response.1, "Resource not found");
        assert!(logs.is_empty(), "{logs}");
    }

    #[test]
    fn unavailable_database_returns_service_unavailable_and_logs_error() {
        let (response, logs) = convert_with_logs(sqlx::Error::PoolClosed);
        assert_eq!(response.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.1, "Database unavailable");
        assert!(logs.contains("ERROR"), "{logs}");
        assert!(logs.contains("Database operation failed"), "{logs}");
    }
}
