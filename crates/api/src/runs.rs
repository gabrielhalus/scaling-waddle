use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Value, json};
use sw_infrastructure::runs::{self, Run, StepRun};

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::ApiError;

const STATUSES: &[&str] = &[
    "queued",
    "running",
    "waiting",
    "succeeded",
    "failed",
    "cancelled",
];
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/runs", get(list))
        .route("/api/v1/runs/{id}", get(detail))
}

#[derive(Deserialize)]
struct Filter {
    workflow_id: Option<String>,
    status: Option<String>,
    limit: Option<String>,
}

fn run_json(run: &Run) -> Value {
    json!({
        "id": run.id,
        "workflow_id": run.workflow_id,
        "version": run.version,
        "event_id": run.event_id,
        "status": run.status,
        "error": run.error,
        "created_at": run.created_at,
        "finished_at": run.finished_at,
    })
}

fn step_json(step: &StepRun) -> Value {
    let output: Value = step
        .output
        .as_deref()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null);
    json!({
        "step_id": step.step_id,
        "status": step.status,
        "attempts": step.attempts,
        "output": output,
        "error": step.error,
        "started_at": step.started_at,
        "finished_at": step.finished_at,
    })
}

async fn list(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(filter): Query<Filter>,
) -> Result<Json<Value>, ApiError> {
    if filter
        .status
        .as_deref()
        .is_some_and(|status| !STATUSES.contains(&status))
    {
        return Err(ApiError::BadRequest("statut inconnu".into()));
    }
    let limit = match &filter.limit {
        None => DEFAULT_LIMIT,
        Some(text) => text
            .parse::<i64>()
            .ok()
            .filter(|limit| *limit >= 1)
            .ok_or_else(|| ApiError::BadRequest("limit doit être un entier positif".into()))?
            .min(MAX_LIMIT),
    };
    let found = runs::list(
        &state.db,
        filter.workflow_id.as_deref(),
        filter.status.as_deref(),
        limit,
    )
    .await?;
    Ok(Json(Value::Array(found.iter().map(run_json).collect())))
}

async fn detail(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let run = runs::get(&state.db, &id).await?.ok_or(ApiError::NotFound)?;
    let event = runs::event_of(&state.db, &run.event_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let steps = runs::steps(&state.db, &id).await?;
    Ok(Json(json!({
        "run": run_json(&run),
        "event": {
            "source": event.source,
            "type": event.kind,
            "payload": event.payload,
            "correlation_id": event.correlation_id,
        },
        "steps": steps.iter().map(step_json).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::testing::{Harness, bearer, harness, send};

    async fn triggered(h: &Harness, name: &str) -> (String, String) {
        let wf = h.active_workflow(name).await;
        let (_, body) = h
            .post(&format!("/api/v1/workflows/{wf}/runs"), json!({ "n": 1 }))
            .await;
        (wf, body["run_id"].as_str().unwrap().to_owned())
    }

    #[tokio::test]
    async fn routes_require_authentication() {
        let h = harness().await;
        let (status, _) = send(&h.app, "GET", "/api/v1/runs", &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_returns_newest_first_with_workflow_filter() {
        let h = harness().await;
        let (wf_a, run_a) = triggered(&h, "a").await;
        let (_, run_b) = triggered(&h, "b").await;
        let (status, all) = h.get("/api/v1/runs").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(all[0]["id"], run_b.as_str());
        assert_eq!(all[1]["id"], run_a.as_str());
        assert_eq!(all[0]["status"], "queued");
        let (_, only_a) = h.get(&format!("/api/v1/runs?workflow_id={wf_a}")).await;
        assert_eq!(only_a.as_array().unwrap().len(), 1);
        assert_eq!(only_a[0]["id"], run_a.as_str());
    }

    #[tokio::test]
    async fn list_filters_by_status_and_validates_it() {
        let h = harness().await;
        triggered(&h, "a").await;
        let (_, queued) = h.get("/api/v1/runs?status=queued").await;
        assert_eq!(queued.as_array().unwrap().len(), 1);
        let (_, failed) = h.get("/api/v1/runs?status=failed").await;
        assert!(failed.as_array().unwrap().is_empty());
        let (status, _) = h.get("/api/v1/runs?status=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_limit_is_clamped_and_validated() {
        let h = harness().await;
        triggered(&h, "a").await;
        triggered(&h, "b").await;
        let (_, limited) = h.get("/api/v1/runs?limit=1").await;
        assert_eq!(limited.as_array().unwrap().len(), 1);
        let (status, _) = h.get("/api/v1/runs?limit=0").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = h.get("/api/v1/runs?limit=abc").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = h.get("/api/v1/runs?limit=100000").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn detail_shows_the_run_the_event_and_its_steps() {
        let h = harness().await;
        let (wf, run_id) = triggered(&h, "a").await;
        let (status, detail) = h.get(&format!("/api/v1/runs/{run_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["run"]["workflow_id"], wf.as_str());
        assert_eq!(detail["run"]["version"], 1);
        assert_eq!(detail["event"]["source"], "manual");
        assert_eq!(detail["event"]["payload"], json!({ "n": 1 }));
        assert_eq!(detail["steps"][0]["step_id"], "a");
        assert_eq!(detail["steps"][0]["status"], "pending");
    }

    #[tokio::test]
    async fn detail_parses_step_outputs() {
        let h = harness().await;
        let (_, run_id) = triggered(&h, "a").await;
        sw_infrastructure::runs::set_step(
            &h.state.db,
            &run_id,
            "a",
            "succeeded",
            Some(&json!({ "status_code": 200 })),
            None,
            1,
        )
        .await
        .unwrap();
        let (_, detail) = h.get(&format!("/api/v1/runs/{run_id}")).await;
        assert_eq!(detail["steps"][0]["output"], json!({ "status_code": 200 }));
        assert_eq!(detail["steps"][0]["attempts"], 1);
    }

    #[tokio::test]
    async fn unknown_run_is_a_404_and_viewers_can_read() {
        let h = harness().await;
        let (_, run_id) = triggered(&h, "a").await;
        let (status, _) = h.get("/api/v1/runs/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let viewer = h.viewer_token().await;
        let (status, _) = send(
            &h.app,
            "GET",
            &format!("/api/v1/runs/{run_id}"),
            &bearer(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}
