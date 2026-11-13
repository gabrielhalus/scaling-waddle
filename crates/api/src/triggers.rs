use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sw_infrastructure::runs::{self, NewEvent, RunError};
use sw_infrastructure::{webhook_tokens, workflows};

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::signature;

const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

pub fn routes() -> Router<AppState> {
    let limit = DefaultBodyLimit::max(MAX_PAYLOAD_BYTES);
    Router::new()
        .route(
            "/api/v1/triggers/webhook/{token}",
            post(webhook).layer(limit),
        )
        .route("/api/v1/workflows/{id}/runs", post(manual_run).layer(limit))
        .route(
            "/api/v1/workflows/{id}/webhook-tokens",
            get(list_tokens).post(issue_token),
        )
        .route(
            "/api/v1/workflows/{id}/webhook-tokens/{token_id}",
            delete(revoke_token),
        )
}

impl From<webhook_tokens::WebhookError> for ApiError {
    fn from(err: webhook_tokens::WebhookError) -> Self {
        tracing::error!(%err, "erreur jetons de webhook");
        Self::Internal
    }
}

impl From<RunError> for ApiError {
    fn from(err: RunError) -> Self {
        match err {
            RunError::NotFound => Self::NotFound,
            RunError::NotActive => Self::Conflict("le workflow n'est pas actif"),
            RunError::AlreadyFinished => Self::Conflict("le run est déjà terminé"),
            RunError::NotRetryable => {
                Self::Conflict("seul un run échoué ou annulé peut être relancé")
            }
            RunError::InvalidStatus(_) | RunError::Definition(_) | RunError::Db(_) => {
                tracing::error!(%err, "erreur runs");
                Self::Internal
            }
        }
    }
}

fn printable(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|b| b.is_ascii_graphic())
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn parse_payload(body: &Bytes) -> Result<Value, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(body)
        .map_err(|err| ApiError::BadRequest(format!("corps JSON invalide: {err}")))
}

async fn ingest(
    state: &AppState,
    workflow_id: &str,
    source: &str,
    kind: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let payload = parse_payload(body)?;
    let idempotency_key = match header(headers, "idempotency-key") {
        None => None,
        Some(key) if printable(key, 255) => Some(key.to_owned()),
        Some(_) => return Err(ApiError::BadRequest("Idempotency-Key invalide".into())),
    };
    let correlation_id = header(headers, "x-correlation-id")
        .filter(|id| printable(id, 128))
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let ingested = runs::ingest(
        &state.db,
        &NewEvent {
            workflow_id: workflow_id.to_owned(),
            source: source.to_owned(),
            kind: kind.to_owned(),
            payload,
            correlation_id: correlation_id.clone(),
            idempotency_key,
        },
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "run_id": ingested.run_id,
            "event_id": ingested.event_id,
            "duplicate": ingested.duplicate,
            "correlation_id": correlation_id,
        })),
    ))
}

#[derive(Deserialize)]
struct IssueRequest {
    #[serde(default)]
    signed: bool,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

async fn webhook(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let resolved = webhook_tokens::resolve(&state.db, &state.keyring, &token)
        .await?
        .ok_or(ApiError::NotFound)?;
    if let Some(secret) = &resolved.signing_secret {
        let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        signature::verify(
            secret,
            header("x-sw-timestamp"),
            header("x-sw-signature"),
            &body,
            now_secs(),
        )
        .map_err(|reason| {
            tracing::warn!(?reason, "signature de webhook refusée");
            ApiError::Unauthorized
        })?;
    }
    ingest(
        &state,
        &resolved.workflow_id,
        "webhook",
        "webhook.received",
        &headers,
        &body,
    )
    .await
}

async fn manual_run(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    auth.require_writer()?;
    ingest(&state, &id, "manual", "manual.run", &headers, &body).await
}

async fn require_workflow(state: &AppState, id: &str) -> Result<(), ApiError> {
    workflows::get(&state.db, id)
        .await?
        .map(|_| ())
        .ok_or(ApiError::NotFound)
}

async fn issue_token(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    auth.require_writer()?;
    require_workflow(&state, &id).await?;
    let signed = if body.is_empty() {
        false
    } else {
        serde_json::from_slice::<IssueRequest>(&body)
            .map_err(|_| ApiError::BadRequest("corps JSON invalide".into()))?
            .signed
    };
    let issued = webhook_tokens::issue(&state.db, &state.keyring, &id, signed).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": issued.id,
            "token": issued.token,
            "signing_secret": issued.signing_secret,
        })),
    ))
}

async fn list_tokens(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_workflow(&state, &id).await?;
    let tokens = webhook_tokens::list(&state.db, &id).await?;
    Ok(Json(Value::Array(
        tokens
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "created_at": t.created_at,
                    "revoked_at": t.revoked_at,
                    "signed": t.signed,
                })
            })
            .collect(),
    )))
}

async fn revoke_token(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, token_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    auth.require_writer()?;
    if webhook_tokens::revoke(&state.db, &id, &token_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::testing::{Harness, bearer, harness, send};

    async fn webhook_token(h: &Harness, wf: &str) -> String {
        let (status, body) = h
            .post(&format!("/api/v1/workflows/{wf}/webhook-tokens"), json!({}))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        body["token"].as_str().unwrap().to_owned()
    }

    async fn hook(
        h: &Harness,
        token: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> (StatusCode, Value) {
        send(
            &h.app,
            "POST",
            &format!("/api/v1/triggers/webhook/{token}"),
            headers,
            Some(("application/json", body.to_owned())),
        )
        .await
    }

    #[tokio::test]
    async fn issuing_a_token_returns_it_once_and_listing_hides_it() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let (status, issued) = h
            .post(&format!("/api/v1/workflows/{wf}/webhook-tokens"), json!({}))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        assert!(issued["token"].as_str().unwrap().starts_with("swh_"));
        let (_, listed) = h
            .get(&format!("/api/v1/workflows/{wf}/webhook-tokens"))
            .await;
        assert_eq!(listed[0]["id"], issued["id"]);
        assert!(listed[0].get("token").is_none());
    }

    #[tokio::test]
    async fn viewer_cannot_issue_or_revoke_tokens() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let viewer = h.viewer_token().await;
        let (status, _) = send(
            &h.app,
            "POST",
            &format!("/api/v1/workflows/{wf}/webhook-tokens"),
            &bearer(&viewer),
            Some(("application/json", "{}".into())),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(
            &h.app,
            "DELETE",
            &format!("/api/v1/workflows/{wf}/webhook-tokens/x"),
            &bearer(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn webhook_queues_a_run_and_answers_202() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let token = webhook_token(&h, &wf).await;
        let (status, body) = hook(&h, &token, &[], r#"{"service":"api"}"#).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["duplicate"], false);

        let run = sw_infrastructure::runs::get(&h.state.db, body["run_id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (run.status.as_str(), run.workflow_id.as_str()),
            ("queued", wf.as_str())
        );
        let (source, payload): (String, String) =
            sqlx::query_as("SELECT source, payload FROM events WHERE id = ?")
                .bind(&run.event_id)
                .fetch_one(&h.state.db)
                .await
                .unwrap();
        assert_eq!(source, "webhook");
        assert_eq!(
            serde_json::from_str::<Value>(&payload).unwrap(),
            json!({ "service": "api" })
        );
    }

    #[tokio::test]
    async fn webhook_needs_no_session_but_hides_unknown_tokens() {
        let h = harness().await;
        let (status, _) = hook(&h, "swh_inconnu", &[], "{}").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn revoked_token_answers_404() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let (_, issued) = h
            .post(&format!("/api/v1/workflows/{wf}/webhook-tokens"), json!({}))
            .await;
        let token = issued["token"].as_str().unwrap();
        let id = issued["id"].as_str().unwrap();
        let (status, _) = send(
            &h.app,
            "DELETE",
            &format!("/api/v1/workflows/{wf}/webhook-tokens/{id}"),
            &bearer(&h.admin),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = hook(&h, token, &[], "{}").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &h.app,
            "DELETE",
            &format!("/api/v1/workflows/{wf}/webhook-tokens/{id}"),
            &bearer(&h.admin),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn inactive_workflow_answers_409() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let token = webhook_token(&h, &wf).await;
        h.post(&format!("/api/v1/workflows/{wf}/deactivate"), json!({}))
            .await;
        let (status, _) = hook(&h, &token, &[], "{}").await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn idempotency_key_header_deduplicates() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let token = webhook_token(&h, &wf).await;
        let key = [("idempotency-key", "delivery-42".to_owned())];
        let (_, first) = hook(&h, &token, &key, "{}").await;
        let (status, second) = hook(&h, &token, &key, "{}").await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(second["duplicate"], true);
        assert_eq!(second["run_id"], first["run_id"]);
    }

    #[tokio::test]
    async fn correlation_id_is_kept_when_valid_else_generated() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let token = webhook_token(&h, &wf).await;
        let (_, kept) = hook(&h, &token, &[("x-correlation-id", "abc-123".into())], "{}").await;
        assert_eq!(kept["correlation_id"], "abc-123");
        let (_, generated) = hook(
            &h,
            &token,
            &[("x-correlation-id", "mauvais id".into())],
            "{}",
        )
        .await;
        assert_ne!(generated["correlation_id"], "mauvais id");
        assert!(generated["correlation_id"].as_str().unwrap().len() >= 32);
    }

    #[tokio::test]
    async fn malformed_json_is_a_400_and_empty_body_is_null() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let token = webhook_token(&h, &wf).await;
        let (status, _) = hook(&h, &token, &[], "{pas du json").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, body) = hook(&h, &token, &[], "").await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let run = sw_infrastructure::runs::get(&h.state.db, body["run_id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        let payload: String = sqlx::query_scalar("SELECT payload FROM events WHERE id = ?")
            .bind(&run.event_id)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
        assert_eq!(payload, "null");
    }

    #[tokio::test]
    async fn oversized_payload_is_a_413() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let token = webhook_token(&h, &wf).await;
        let big = format!("\"{}\"", "x".repeat(1_100_000));
        let (status, _) = hook(&h, &token, &[], &big).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn manual_run_requires_a_writer() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let uri = format!("/api/v1/workflows/{wf}/runs");
        let (status, _) = send(
            &h.app,
            "POST",
            &uri,
            &[],
            Some(("application/json", "{}".into())),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let viewer = h.viewer_token().await;
        let (status, _) = send(
            &h.app,
            "POST",
            &uri,
            &bearer(&viewer),
            Some(("application/json", "{}".into())),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn manual_run_queues_with_manual_source() {
        let h = harness().await;
        let wf = h.active_workflow("w").await;
        let (status, body) = h
            .post(
                &format!("/api/v1/workflows/{wf}/runs"),
                json!({ "env": "prod" }),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let run = sw_infrastructure::runs::get(&h.state.db, body["run_id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        let source: String = sqlx::query_scalar("SELECT source FROM events WHERE id = ?")
            .bind(&run.event_id)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
        assert_eq!(source, "manual");
    }

    #[tokio::test]
    async fn manual_run_on_unknown_or_inactive_workflow() {
        let h = harness().await;
        let (status, _) = h.post("/api/v1/workflows/nope/runs", json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let wf = h.active_workflow("w").await;
        h.post(&format!("/api/v1/workflows/{wf}/deactivate"), json!({}))
            .await;
        let (status, _) = h
            .post(&format!("/api/v1/workflows/{wf}/runs"), json!({}))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    async fn signed_token(h: &Harness, wf: &str) -> (String, String) {
        let (status, body) = h
            .post(
                &format!("/api/v1/workflows/{wf}/webhook-tokens"),
                json!({ "signed": true }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        (
            body["token"].as_str().unwrap().to_owned(),
            body["signing_secret"].as_str().unwrap().to_owned(),
        )
    }

    fn signature_headers(secret: &str, timestamp: i64, body: &str) -> Vec<(&'static str, String)> {
        vec![
            ("x-sw-timestamp", timestamp.to_string()),
            (
                "x-sw-signature",
                crate::signature::sign(secret, timestamp, body.as_bytes()),
            ),
        ]
    }

    #[tokio::test]
    async fn unsigned_tokens_expose_no_secret() {
        let h = harness().await;
        let wf = h.active_workflow("a").await;
        let (_, body) = h
            .post(&format!("/api/v1/workflows/{wf}/webhook-tokens"), json!({}))
            .await;
        assert!(body["signing_secret"].is_null());
    }

    #[tokio::test]
    async fn signed_webhook_accepts_a_valid_signature_only() {
        let h = harness().await;
        let wf = h.active_workflow("a").await;
        let (token, secret) = signed_token(&h, &wf).await;
        let body = r#"{"x":1}"#;
        let (status, _) = hook(&h, &token, &[], body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = hook(
            &h,
            &token,
            &signature_headers("whsec_faux", super::now_secs(), body),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = hook(
            &h,
            &token,
            &signature_headers(&secret, super::now_secs(), body),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn signed_webhook_rejects_a_replayed_old_request() {
        let h = harness().await;
        let wf = h.active_workflow("a").await;
        let (token, secret) = signed_token(&h, &wf).await;
        let body = "{}";
        let old = super::now_secs() - 3600;
        let (status, _) = hook(&h, &token, &signature_headers(&secret, old, body), body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn token_listing_flags_signed_tokens() {
        let h = harness().await;
        let wf = h.active_workflow("a").await;
        signed_token(&h, &wf).await;
        let (_, listed) = h
            .get(&format!("/api/v1/workflows/{wf}/webhook-tokens"))
            .await;
        assert_eq!(listed[0]["signed"], true);
        assert!(listed[0].get("signing_secret").is_none());
    }

    #[tokio::test]
    async fn issuing_with_an_invalid_body_is_a_400() {
        let h = harness().await;
        let wf = h.active_workflow("a").await;
        let (status, _) = h
            .post(
                &format!("/api/v1/workflows/{wf}/webhook-tokens"),
                json!({ "signed": "oui" }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
