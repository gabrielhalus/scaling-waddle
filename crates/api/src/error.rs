use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Invalid {
        message: String,
        details: Vec<String>,
    },
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict(&'static str),
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, json!({ "error": message })),
            Self::Invalid { message, details } => (
                StatusCode::BAD_REQUEST,
                json!({ "error": message, "details": details }),
            ),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                json!({ "error": "identifiants invalides" }),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                json!({ "error": "droits insuffisants" }),
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, json!({ "error": "introuvable" })),
            Self::Conflict(message) => (StatusCode::CONFLICT, json!({ "error": message })),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "erreur interne" }),
            ),
        };
        (status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(%err, "erreur base de données");
        Self::Internal
    }
}
