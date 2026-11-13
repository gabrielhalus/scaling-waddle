use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::error::ApiError;
use crate::triggers::{header, ingest, parse_payload, resolve_token};

const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

pub fn routes() -> Router<AppState> {
    let limit = DefaultBodyLimit::max(MAX_PAYLOAD_BYTES);
    Router::new()
        .route("/api/v1/triggers/gitlab/{token}", post(gitlab).layer(limit))
        .route(
            "/api/v1/triggers/uptime-kuma/{token}",
            post(uptime_kuma).layer(limit),
        )
}

fn slug(text: &str) -> String {
    let cleaned: String = text
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_').to_owned();
    if trimmed.is_empty() {
        "event".to_owned()
    } else {
        trimmed.chars().take(40).collect()
    }
}

// `object_kind` du corps, sinon l'en-tête `X-Gitlab-Event` ("Push Hook" devient "push").
pub fn gitlab_kind(headers: &HeaderMap, payload: &Value) -> String {
    let raw = payload
        .get("object_kind")
        .and_then(Value::as_str)
        .or_else(|| header(headers, "x-gitlab-event").map(|h| h.trim_end_matches(" Hook")))
        .unwrap_or("event");
    format!("gitlab.{}", slug(raw))
}

fn same_secret(a: &str, b: &str) -> bool {
    Sha256::digest(a.as_bytes()) == Sha256::digest(b.as_bytes())
}

async fn gitlab(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let resolved = resolve_token(&state, &token).await?;
    if let Some(secret) = &resolved.signing_secret {
        let provided = header(&headers, "x-gitlab-token").unwrap_or_default();
        if !same_secret(secret, provided) {
            tracing::warn!("jeton GitLab refusé");
            return Err(ApiError::Unauthorized);
        }
    }
    let kind = gitlab_kind(&headers, &parse_payload(&body)?);
    ingest(
        &state,
        &resolved.workflow_id,
        "gitlab",
        &kind,
        &headers,
        &body,
    )
    .await
}

// 0 = down, 1 = up, 2 = pending, 3 = maintenance dans les heartbeats d'Uptime Kuma.
pub fn uptime_kuma_event(payload: &Value) -> (String, Value) {
    let status = match payload.pointer("/heartbeat/status").and_then(Value::as_i64) {
        Some(0) => "down",
        Some(1) => "up",
        Some(2) => "pending",
        Some(3) => "maintenance",
        _ => "test",
    };
    let text = |pointer: &str| payload.pointer(pointer).cloned().unwrap_or(Value::Null);
    (
        format!("uptime-kuma.{status}"),
        json!({
            "status": status,
            "message": payload.get("msg").cloned().unwrap_or(Value::Null),
            "monitor": { "name": text("/monitor/name"), "url": text("/monitor/url") },
            "raw": payload,
        }),
    )
}

async fn uptime_kuma(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let resolved = resolve_token(&state, &token).await?;
    let (kind, normalized) = uptime_kuma_event(&parse_payload(&body)?);
    ingest(
        &state,
        &resolved.workflow_id,
        "uptime-kuma",
        &kind,
        &headers,
        &Bytes::from(normalized.to_string()),
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderValue, StatusCode};
    use serde_json::json;
    use sw_infrastructure::runs;

    use super::*;
    use crate::testing::{Harness, harness, send};

    async fn token(h: &Harness, signed: bool) -> (String, String, Option<String>) {
        let wf = h.active_workflow("conn").await;
        let (_, body) = h
            .post(
                &format!("/api/v1/workflows/{wf}/webhook-tokens"),
                json!({ "signed": signed }),
            )
            .await;
        (
            wf,
            body["token"].as_str().unwrap().to_owned(),
            body["signing_secret"].as_str().map(str::to_owned),
        )
    }

    async fn call(
        h: &Harness,
        route: &str,
        token: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> (StatusCode, Value) {
        send(
            &h.app,
            "POST",
            &format!("/api/v1/triggers/{route}/{token}"),
            headers,
            Some(("application/json", body.to_string())),
        )
        .await
    }

    async fn last_event(h: &Harness, run_id: &str) -> (String, String, Value) {
        let run = runs::get(&h.state.db, run_id).await.unwrap().unwrap();
        let event = runs::event_of(&h.state.db, &run.event_id)
            .await
            .unwrap()
            .unwrap();
        (event.source, event.kind, event.payload)
    }

    #[test]
    fn gitlab_kind_prefers_the_body_then_the_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-gitlab-event",
            HeaderValue::from_static("Merge Request Hook"),
        );
        assert_eq!(
            gitlab_kind(&headers, &json!({ "object_kind": "push" })),
            "gitlab.push"
        );
        assert_eq!(gitlab_kind(&headers, &json!({})), "gitlab.merge_request");
        assert_eq!(gitlab_kind(&HeaderMap::new(), &json!({})), "gitlab.event");
        assert_eq!(
            gitlab_kind(&HeaderMap::new(), &json!({ "object_kind": "../x y" })),
            "gitlab.x_y"
        );
    }

    #[test]
    fn uptime_kuma_statuses_are_normalised() {
        for (code, name) in [(0, "down"), (1, "up"), (2, "pending"), (3, "maintenance")] {
            let (kind, event) = uptime_kuma_event(&json!({ "heartbeat": { "status": code } }));
            assert_eq!(
                (kind.as_str(), event["status"].as_str()),
                (format!("uptime-kuma.{name}").as_str(), Some(name))
            );
        }
        let (kind, _) = uptime_kuma_event(&json!({ "msg": "Testing" }));
        assert_eq!(kind, "uptime-kuma.test");
    }

    #[tokio::test]
    async fn a_gitlab_push_queues_a_typed_run() {
        let h = harness().await;
        let (_, token, _) = token(&h, false).await;
        let payload = json!({ "object_kind": "push", "ref": "refs/heads/main" });
        let (status, body) = call(&h, "gitlab", &token, &[], &payload).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let (source, kind, stored) = last_event(&h, body["run_id"].as_str().unwrap()).await;
        assert_eq!((source.as_str(), kind.as_str()), ("gitlab", "gitlab.push"));
        assert_eq!(stored, payload);
    }

    #[tokio::test]
    async fn a_signed_token_requires_the_gitlab_secret_header() {
        let h = harness().await;
        let (_, token, secret) = token(&h, true).await;
        let secret = secret.unwrap();
        let payload = json!({ "object_kind": "pipeline" });
        let (status, _) = call(&h, "gitlab", &token, &[], &payload).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(
            &h,
            "gitlab",
            &token,
            &[("x-gitlab-token", "faux".into())],
            &payload,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(
            &h,
            "gitlab",
            &token,
            &[("x-gitlab-token", secret)],
            &payload,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn an_uptime_kuma_alert_is_normalised_into_the_event() {
        let h = harness().await;
        let (_, token, _) = token(&h, false).await;
        let payload = json!({
            "msg": "[API] [🔴 Down] timeout",
            "monitor": { "name": "API", "url": "https://api.example.com" },
            "heartbeat": { "status": 0, "msg": "timeout" }
        });
        let (status, body) = call(&h, "uptime-kuma", &token, &[], &payload).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let (source, kind, stored) = last_event(&h, body["run_id"].as_str().unwrap()).await;
        assert_eq!(
            (source.as_str(), kind.as_str()),
            ("uptime-kuma", "uptime-kuma.down")
        );
        assert_eq!(stored["status"], "down");
        assert_eq!(stored["monitor"]["name"], "API");
        assert_eq!(stored["raw"], payload);
    }

    #[tokio::test]
    async fn unknown_tokens_and_bad_bodies_are_rejected() {
        let h = harness().await;
        let (_, token, _) = token(&h, false).await;
        let (status, _) = call(&h, "gitlab", "swh_inconnu", &[], &json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &h.app,
            "POST",
            &format!("/api/v1/triggers/uptime-kuma/{token}"),
            &[],
            Some(("application/json", "pas du json".into())),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn connectors_share_the_webhook_rate_limit() {
        let h = harness().await;
        let (_, token, _) = token(&h, false).await;
        for _ in 0..120 {
            call(&h, "gitlab", &token, &[], &json!({})).await;
        }
        let (status, _) = call(&h, "uptime-kuma", &token, &[], &json!({})).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }
}
