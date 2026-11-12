use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
}

async fn live() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "status": "ok", "database": "ok" })),
        ),
        Err(err) => {
            tracing::warn!(%err, "readiness: base injoignable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "unavailable", "database": "down" })),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::{AppState, router};

    async fn app() -> (axum::Router, AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = sw_infrastructure::db::open(&dir.path().join("sw.db"))
            .await
            .unwrap();
        let state = AppState::new(db, "0123456789abcdef0123456789abcdef");
        (router(state.clone()), state, dir)
    }

    async fn get(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let res = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn live_answers_ok() {
        let (app, _state, _dir) = app().await;
        let (status, body) = get(app, "/health/live").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn ready_answers_ok_when_database_is_reachable() {
        let (app, _state, _dir) = app().await;
        let (status, body) = get(app, "/health/ready").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["database"], "ok");
    }

    #[tokio::test]
    async fn ready_answers_503_when_database_is_down() {
        let (app, state, _dir) = app().await;
        state.db.close().await;
        let (status, body) = get(app, "/health/ready").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["database"], "down");
    }
}
