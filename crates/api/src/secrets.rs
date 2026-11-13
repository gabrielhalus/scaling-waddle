use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sw_infrastructure::crypto::CryptoError;
use sw_infrastructure::secrets::{self, SecretError};
use sw_infrastructure::webhook_tokens;

use crate::AppState;
use crate::audit;
use crate::auth::AuthUser;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/secrets", get(list))
        .route("/api/v1/secrets/rotate", post(rotate))
        .route(
            "/api/v1/secrets/{project}/{name}",
            put(store).delete(remove),
        )
}

impl From<SecretError> for ApiError {
    fn from(err: SecretError) -> Self {
        match err {
            SecretError::InvalidName => Self::BadRequest(err.to_string()),
            SecretError::InvalidValue => Self::BadRequest(err.to_string()),
            SecretError::Crypto(CryptoError::NoKeys) => {
                Self::Unavailable("chiffrement des secrets non configuré (SW_SECRET_KEYS)")
            }
            SecretError::Crypto(_) | SecretError::Db(_) => {
                tracing::error!(%err, "erreur secrets");
                Self::Internal
            }
        }
    }
}

#[derive(Deserialize)]
struct Filter {
    project: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(filter): Query<Filter>,
) -> Result<Json<Value>, ApiError> {
    let found = secrets::list(&state.db, filter.project.as_deref()).await?;
    Ok(Json(Value::Array(
        found
            .iter()
            .map(|meta| {
                json!({
                    "project": meta.project,
                    "name": meta.name,
                    "key_id": meta.key_id,
                    "created_at": meta.created_at,
                    "updated_at": meta.updated_at,
                })
            })
            .collect(),
    )))
}

#[derive(Deserialize)]
struct NewValue {
    value: String,
}

async fn store(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((project, name)): Path<(String, String)>,
    Json(body): Json<NewValue>,
) -> Result<StatusCode, ApiError> {
    auth.require_writer()?;
    secrets::set(&state.db, &state.keyring, &project, &name, &body.value).await?;
    audit::record(
        &state,
        &auth.id,
        "secret.set",
        "secret",
        &name,
        json!({ "project": project }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((project, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    auth.require_writer()?;
    if secrets::delete(&state.db, &project, &name).await? {
        audit::record(
            &state,
            &auth.id,
            "secret.delete",
            "secret",
            &name,
            json!({ "project": project }),
        )
        .await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

async fn rotate(State(state): State<AppState>, auth: AuthUser) -> Result<Json<Value>, ApiError> {
    auth.require_admin()?;
    let secrets = secrets::rotate(&state.db, &state.keyring).await?;
    let signing = webhook_tokens::rotate(&state.db, &state.keyring).await?;
    audit::record(
        &state,
        &auth.id,
        "secret.rotate",
        "secret",
        "*",
        json!({ "rotated": secrets + signing }),
    )
    .await;
    Ok(Json(json!({ "rotated": secrets + signing })))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::AppState;
    use crate::testing::{bearer, harness, send};

    async fn put(
        h: &crate::testing::Harness,
        project: &str,
        name: &str,
        value: &str,
    ) -> StatusCode {
        send(
            &h.app,
            "PUT",
            &format!("/api/v1/secrets/{project}/{name}"),
            &bearer(&h.admin),
            Some(("application/json", json!({ "value": value }).to_string())),
        )
        .await
        .0
    }

    #[tokio::test]
    async fn stored_secrets_are_listed_without_their_value() {
        let h = harness().await;
        assert_eq!(
            put(&h, "p", "gh_token", "ghp_abc").await,
            StatusCode::NO_CONTENT
        );
        let (status, listed) = h.get("/api/v1/secrets?project=p").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed[0]["name"], "gh_token");
        assert_eq!(listed[0]["key_id"], "k1");
        assert!(!listed.to_string().contains("ghp_abc"));
        let (_, other) = h.get("/api/v1/secrets?project=q").await;
        assert!(other.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_names_and_values_are_400() {
        let h = harness().await;
        assert_eq!(
            put(&h, "p", "bad%20name", "v").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(put(&h, "p", "ok", "").await, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_removes_and_reports_unknown() {
        let h = harness().await;
        put(&h, "p", "n", "v").await;
        let uri = "/api/v1/secrets/p/n";
        let auth = bearer(&h.admin);
        let first = send(&h.app, "DELETE", uri, &auth, None).await;
        let second = send(&h.app, "DELETE", uri, &auth, None).await;
        assert_eq!(first.0, StatusCode::NO_CONTENT);
        assert_eq!(second.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn viewers_can_list_but_not_write() {
        let h = harness().await;
        let viewer = h.viewer_token().await;
        let (status, _) = send(&h.app, "GET", "/api/v1/secrets", &bearer(&viewer), None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(
            &h.app,
            "PUT",
            "/api/v1/secrets/p/n",
            &bearer(&viewer),
            Some(("application/json", json!({ "value": "v" }).to_string())),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(
            &h.app,
            "POST",
            "/api/v1/secrets/rotate",
            &bearer(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn without_a_configured_key_writes_answer_503() {
        let h = harness().await;
        let bare = crate::router(AppState::new(
            h.state.db.clone(),
            "0123456789abcdef0123456789abcdef",
        ));
        let (status, _) = send(
            &bare,
            "PUT",
            "/api/v1/secrets/p/n",
            &bearer(&h.admin),
            Some(("application/json", json!({ "value": "v" }).to_string())),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn rotate_moves_secrets_to_the_active_key() {
        let h = harness().await;
        put(&h, "p", "n", "v").await;
        let keys = format!(
            "k1={},k2={}",
            hex::encode([1u8; 32]),
            hex::encode([2u8; 32])
        );
        let rotated_state = h
            .state
            .clone()
            .with_keyring(sw_infrastructure::crypto::Keyring::parse(&keys, Some("k2")).unwrap());
        let app = crate::router(rotated_state);
        let (status, body) = send(
            &app,
            "POST",
            "/api/v1/secrets/rotate",
            &bearer(&h.admin),
            None,
        )
        .await;
        assert_eq!((status, &body["rotated"]), (StatusCode::OK, &json!(1)));
        let (_, listed) = h.get("/api/v1/secrets").await;
        assert_eq!(listed[0]["key_id"], "k2");
    }
}
