use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sw_infrastructure::audit;

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::ApiError;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/audit", get(list))
}

// Un échec d'écriture est journalisé sans bloquer l'action déjà effectuée.
pub async fn record(
    state: &AppState,
    actor: &str,
    action: &str,
    target_type: &str,
    target_id: &str,
    details: Value,
) {
    if let Err(err) =
        audit::record(&state.db, actor, action, target_type, target_id, &details).await
    {
        tracing::error!(%err, action, "écriture d'audit en échec");
    }
}

#[derive(Deserialize)]
struct Filter {
    action: Option<String>,
    actor: Option<String>,
    limit: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(filter): Query<Filter>,
) -> Result<Json<Value>, ApiError> {
    auth.require_admin()?;
    let limit = match &filter.limit {
        None => DEFAULT_LIMIT,
        Some(text) => text
            .parse::<i64>()
            .ok()
            .filter(|limit| *limit >= 1)
            .ok_or_else(|| ApiError::BadRequest("limit doit être un entier positif".into()))?
            .min(MAX_LIMIT),
    };
    let events = audit::list(
        &state.db,
        filter.action.as_deref(),
        filter.actor.as_deref(),
        limit,
    )
    .await?;
    Ok(Json(Value::Array(
        events
            .iter()
            .map(|event| {
                json!({
                    "id": event.id,
                    "at": event.at,
                    "actor": event.actor,
                    "action": event.action,
                    "target_type": event.target_type,
                    "target_id": event.target_id,
                    "details": serde_json::from_str::<Value>(&event.details).unwrap_or(Value::Null),
                })
            })
            .collect(),
    )))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::testing::{bearer, harness, send};

    #[tokio::test]
    async fn only_admins_can_read_the_trail() {
        let h = harness().await;
        let viewer = h.viewer_token().await;
        let (status, _) = send(&h.app, "GET", "/api/v1/audit", &bearer(&viewer), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(&h.app, "GET", "/api/v1/audit", &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, events) = h.get("/api/v1/audit").await;
        assert_eq!(status, StatusCode::OK);
        assert!(events.is_array());
    }

    #[tokio::test]
    async fn filters_and_limit_are_validated() {
        let h = harness().await;
        let (status, _) = h.get("/api/v1/audit?limit=0").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = h.get("/api/v1/audit?limit=abc").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        h.active_workflow("a").await;
        let (_, only) = h.get("/api/v1/audit?action=workflow.create").await;
        assert_eq!(only.as_array().unwrap().len(), 1);
        let (_, none) = h.get("/api/v1/audit?action=nope").await;
        assert!(none.as_array().unwrap().is_empty());
    }

    async fn actions(h: &crate::testing::Harness) -> Vec<String> {
        let (_, events) = h.get("/api/v1/audit?limit=500").await;
        events
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["action"].as_str().unwrap().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn every_sensitive_action_leaves_a_trace_with_the_actor() {
        let h = harness().await;
        let wf = h.active_workflow("a").await;
        let (_, me) = h.get("/api/v1/auth/me").await;
        let (_, run) = h
            .post(&format!("/api/v1/workflows/{wf}/runs"), json!({}))
            .await;
        let run_id = run["run_id"].as_str().unwrap();
        h.post(&format!("/api/v1/runs/{run_id}/cancel"), json!({}))
            .await;
        h.post(&format!("/api/v1/runs/{run_id}/retry"), json!({}))
            .await;
        let (_, token) = h
            .post(
                &format!("/api/v1/workflows/{wf}/webhook-tokens"),
                json!({ "signed": true }),
            )
            .await;
        send(
            &h.app,
            "DELETE",
            &format!(
                "/api/v1/workflows/{wf}/webhook-tokens/{}",
                token["id"].as_str().unwrap()
            ),
            &bearer(&h.admin),
            None,
        )
        .await;
        let (_, integration) = h
            .post(
                "/api/v1/integrations",
                json!({ "project": "p", "name": "hook", "base_url": "https://x.example.com", "headers": { "authorization": "Bearer TOPSECRET-HEADER" } }),
            )
            .await;
        send(
            &h.app,
            "DELETE",
            &format!(
                "/api/v1/integrations/{}",
                integration["id"].as_str().unwrap()
            ),
            &bearer(&h.admin),
            None,
        )
        .await;
        send(
            &h.app,
            "PUT",
            "/api/v1/secrets/p/tok",
            &bearer(&h.admin),
            Some((
                "application/json",
                json!({ "value": "TOPSECRET-VALUE" }).to_string(),
            )),
        )
        .await;
        h.post("/api/v1/secrets/rotate", json!({})).await;
        send(
            &h.app,
            "DELETE",
            "/api/v1/secrets/p/tok",
            &bearer(&h.admin),
            None,
        )
        .await;

        let seen = actions(&h).await;
        for expected in [
            "auth.setup",
            "workflow.create",
            "workflow.activate",
            "run.trigger",
            "run.cancel",
            "run.retry",
            "webhook_token.issue",
            "webhook_token.revoke",
            "integration.create",
            "integration.delete",
            "secret.set",
            "secret.rotate",
            "secret.delete",
        ] {
            assert!(
                seen.iter().any(|a| a == expected),
                "manque {expected} dans {seen:?}"
            );
        }
        let (_, events) = h.get("/api/v1/audit?action=run.cancel").await;
        assert_eq!(events[0]["actor"], me["id"]);
        assert_eq!(events[0]["target_id"], run_id);
        let (_, all) = h.get("/api/v1/audit?limit=500").await;
        let text = all.to_string();
        assert!(!text.contains("TOPSECRET-VALUE") && !text.contains("TOPSECRET-HEADER"));
        assert!(!text.contains(token["signing_secret"].as_str().unwrap()));
        assert!(!text.contains(token["token"].as_str().unwrap()));
    }

    #[tokio::test]
    async fn versions_deactivation_and_approvals_are_traced() {
        let h = harness().await;
        let wf = h.active_workflow("a").await;
        h.post(
            &format!("/api/v1/workflows/{wf}/versions"),
            crate::testing::workflow("a"),
        )
        .await;
        h.post(&format!("/api/v1/workflows/{wf}/deactivate"), json!({}))
            .await;
        let (_, run) = {
            h.post(
                &format!("/api/v1/workflows/{wf}/activate"),
                json!({ "version": 1 }),
            )
            .await;
            h.post(&format!("/api/v1/workflows/{wf}/runs"), json!({}))
                .await
        };
        let id = sw_infrastructure::approvals::request(
            &h.state.db,
            run["run_id"].as_str().unwrap(),
            "a",
            None,
        )
        .await
        .unwrap();
        h.post(
            &format!("/api/v1/approvals/{id}/decision"),
            json!({ "decision": "reject" }),
        )
        .await;
        let seen = actions(&h).await;
        for expected in ["workflow.version", "workflow.deactivate", "approval.reject"] {
            assert!(
                seen.iter().any(|a| a == expected),
                "manque {expected} dans {seen:?}"
            );
        }
    }

    #[tokio::test]
    async fn login_attempts_are_traced_with_no_password() {
        let h = harness().await;
        let attempt = |password: &'static str| {
            send(
                &h.app,
                "POST",
                "/api/v1/auth/login",
                &[],
                Some((
                    "application/json",
                    json!({ "email": "ada@example.com", "password": password }).to_string(),
                )),
            )
        };
        attempt("mauvais-mot-de-passe").await;
        attempt(crate::testing::PASSWORD).await;
        let seen = actions(&h).await;
        assert!(seen.iter().any(|a| a == "auth.login_failed"));
        assert!(seen.iter().any(|a| a == "auth.login"));
        let (_, all) = h.get("/api/v1/audit").await;
        assert!(!all.to_string().contains("mauvais-mot-de-passe"));
    }

    #[tokio::test]
    async fn rejected_actions_leave_no_trace() {
        let h = harness().await;
        let viewer = h.viewer_token().await;
        let before = actions(&h).await.len();
        let (status, _) = send(
            &h.app,
            "POST",
            "/api/v1/workflows",
            &bearer(&viewer),
            Some((
                "application/json",
                crate::testing::workflow("x").to_string(),
            )),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = h.post("/api/v1/runs/nope/cancel", json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(actions(&h).await.len(), before);
    }
}
