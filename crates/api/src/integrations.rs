use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sw_infrastructure::integrations::{self, IntegrationError, IntegrationRecord};

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::ApiError;

const FORBIDDEN_HEADERS: &[&str] = &["host", "content-length", "transfer-encoding", "connection"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/integrations", get(list).post(create))
        .route("/api/v1/integrations/{id}", delete(remove))
}

#[derive(Deserialize)]
struct NewIntegration {
    project: String,
    name: String,
    base_url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

impl From<IntegrationError> for ApiError {
    fn from(err: IntegrationError) -> Self {
        match err {
            IntegrationError::AlreadyExists => {
                Self::Conflict("une intégration porte déjà ce nom dans ce projet")
            }
            IntegrationError::Headers(_) | IntegrationError::Db(_) => {
                tracing::error!(%err, "erreur intégrations");
                Self::Internal
            }
        }
    }
}

fn record_json(record: &IntegrationRecord) -> Value {
    json!({
        "id": record.id,
        "project": record.project,
        "name": record.name,
        "base_url": record.base_url,
        "headers": record.headers,
        "created_at": record.created_at,
    })
}

fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.to_owned())
}

fn validate(input: &NewIntegration) -> Result<(), ApiError> {
    if input.project.trim().is_empty() {
        return Err(bad("le projet est requis"));
    }
    let name_ok = !input.name.is_empty()
        && input.name.len() <= 64
        && input
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !name_ok {
        return Err(bad(
            "le nom doit faire 1 à 64 caractères parmi a-z, 0-9, - et _",
        ));
    }
    let url = url::Url::parse(&input.base_url).map_err(|_| bad("base_url invalide"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(bad("base_url doit être une URL http ou https avec un hôte"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("base_url ne doit pas contenir d'identifiants"));
    }
    for (name, value) in &input.headers {
        let name_ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !name_ok || FORBIDDEN_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(bad("nom d'en-tête invalide ou interdit"));
        }
        if value.chars().any(char::is_control) {
            return Err(bad("valeur d'en-tête invalide"));
        }
    }
    Ok(())
}

async fn list(State(state): State<AppState>, _auth: AuthUser) -> Result<Json<Value>, ApiError> {
    let records = integrations::list(&state.db).await?;
    Ok(Json(Value::Array(
        records.iter().map(record_json).collect(),
    )))
}

async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(input): Json<NewIntegration>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    auth.require_writer()?;
    validate(&input)?;
    let record = integrations::create(
        &state.db,
        &input.project,
        &input.name,
        &input.base_url,
        &input.headers,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(record_json(&record))))
}

async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require_writer()?;
    if integrations::delete(&state.db, &id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::testing::{bearer, harness, send};

    fn body(name: &str) -> Value {
        json!({ "project": "p", "name": name, "base_url": "https://api.example.com", "headers": { "x-team": "a" } })
    }

    #[tokio::test]
    async fn create_then_list() {
        let h = harness().await;
        let (status, created) = h.post("/api/v1/integrations", body("hook")).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["name"], "hook");
        assert_eq!(created["headers"]["x-team"], "a");
        let (status, listed) = h.get("/api/v1/integrations").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], created["id"]);
    }

    #[tokio::test]
    async fn routes_require_authentication_and_writers_for_changes() {
        let h = harness().await;
        let (status, _) = send(&h.app, "GET", "/api/v1/integrations", &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let viewer = h.viewer_token().await;
        let (status, _) = send(
            &h.app,
            "GET",
            "/api/v1/integrations",
            &bearer(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(
            &h.app,
            "POST",
            "/api/v1/integrations",
            &bearer(&viewer),
            Some(("application/json", body("hook").to_string())),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(
            &h.app,
            "DELETE",
            "/api/v1/integrations/x",
            &bearer(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn duplicate_name_in_a_project_is_a_409() {
        let h = harness().await;
        h.post("/api/v1/integrations", body("hook")).await;
        let (status, _) = h.post("/api/v1/integrations", body("hook")).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn invalid_fields_are_rejected() {
        let h = harness().await;
        let cases = [
            json!({ "project": "", "name": "a", "base_url": "https://x.example" }),
            json!({ "project": "p", "name": "Bad Name", "base_url": "https://x.example" }),
            json!({ "project": "p", "name": "a", "base_url": "ftp://x.example" }),
            json!({ "project": "p", "name": "a", "base_url": "pas une url" }),
            json!({ "project": "p", "name": "a", "base_url": "https://u:p@x.example" }),
            json!({ "project": "p", "name": "a", "base_url": "https://x.example", "headers": { "bad name": "v" } }),
            json!({ "project": "p", "name": "a", "base_url": "https://x.example", "headers": { "host": "evil" } }),
            json!({ "project": "p", "name": "a", "base_url": "https://x.example", "headers": { "x": "a\nb" } }),
        ];
        for case in cases {
            let (status, _) = h.post("/api/v1/integrations", case.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{case}");
        }
    }

    #[tokio::test]
    async fn headers_are_optional() {
        let h = harness().await;
        let (status, created) = h
            .post(
                "/api/v1/integrations",
                json!({ "project": "p", "name": "a", "base_url": "https://x.example" }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["headers"], json!({}));
    }

    #[tokio::test]
    async fn delete_removes_the_integration() {
        let h = harness().await;
        let (_, created) = h.post("/api/v1/integrations", body("hook")).await;
        let uri = format!("/api/v1/integrations/{}", created["id"].as_str().unwrap());
        let (status, _) = send(&h.app, "DELETE", &uri, &bearer(&h.admin), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(&h.app, "DELETE", &uri, &bearer(&h.admin), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, listed) = h.get("/api/v1/integrations").await;
        assert!(listed.as_array().unwrap().is_empty());
    }
}
