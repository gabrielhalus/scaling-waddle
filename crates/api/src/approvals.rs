use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sw_infrastructure::approvals::{self, Approval, ApprovalError};

use crate::AppState;
use crate::audit;
use crate::auth::AuthUser;
use crate::error::ApiError;

const STATUSES: &[&str] = &["pending", "approved", "rejected", "expired", "cancelled"];
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const MAX_COMMENT_CHARS: usize = 1000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/approvals", get(list))
        .route("/api/v1/approvals/{id}", get(detail))
        .route("/api/v1/approvals/{id}/decision", post(decide))
}

impl From<ApprovalError> for ApiError {
    fn from(err: ApprovalError) -> Self {
        match err {
            ApprovalError::NotFound => Self::NotFound,
            ApprovalError::AlreadyDecided => Self::Conflict("approbation déjà traitée"),
            ApprovalError::Db(err) => err.into(),
        }
    }
}

fn approval_json(approval: &Approval) -> Value {
    json!({
        "id": approval.id,
        "run_id": approval.run_id,
        "step_id": approval.step_id,
        "status": approval.status,
        "message": approval.message,
        "requested_at": approval.requested_at,
        "decided_by": approval.decided_by,
        "decided_at": approval.decided_at,
        "comment": approval.comment,
    })
}

#[derive(Deserialize)]
struct Filter {
    status: Option<String>,
    limit: Option<String>,
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
    let found = approvals::list(&state.db, filter.status.as_deref(), limit).await?;
    Ok(Json(Value::Array(
        found.iter().map(approval_json).collect(),
    )))
}

async fn detail(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let found = approvals::get(&state.db, &id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(approval_json(&found)))
}

#[derive(Deserialize)]
struct Decision {
    decision: String,
    comment: Option<String>,
}

async fn decide(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<Decision>,
) -> Result<Json<Value>, ApiError> {
    auth.require_writer()?;
    let approve = match body.decision.as_str() {
        "approve" => true,
        "reject" => false,
        _ => {
            return Err(ApiError::BadRequest(
                "decision doit valoir approve ou reject".into(),
            ));
        }
    };
    if body
        .comment
        .as_deref()
        .is_some_and(|comment| comment.chars().count() > MAX_COMMENT_CHARS)
    {
        return Err(ApiError::BadRequest("commentaire trop long".into()));
    }
    let decided =
        approvals::decide(&state.db, &id, approve, &auth.id, body.comment.as_deref()).await?;
    audit::record(
        &state,
        &auth.id,
        if approve {
            "approval.approve"
        } else {
            "approval.reject"
        },
        "approval",
        &id,
        json!({ "run_id": decided.run_id, "step_id": decided.step_id }),
    )
    .await;
    Ok(Json(approval_json(&decided)))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;
    use sw_infrastructure::approvals;

    use crate::testing::{Harness, bearer, harness, send};

    async fn pending(h: &Harness) -> (String, String) {
        let wf = h.active_workflow("gate").await;
        let (_, body) = h
            .post(&format!("/api/v1/workflows/{wf}/runs"), json!({}))
            .await;
        let run_id = body["run_id"].as_str().unwrap().to_owned();
        let id = approvals::request(&h.state.db, &run_id, "a", Some("Déployer ?"))
            .await
            .unwrap();
        (run_id, id)
    }

    #[tokio::test]
    async fn list_and_detail_expose_pending_requests() {
        let h = harness().await;
        let (run_id, id) = pending(&h).await;
        let (status, listed) = h.get("/api/v1/approvals?status=pending").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed[0]["id"], id.as_str());
        assert_eq!(listed[0]["run_id"], run_id.as_str());
        let (_, one) = h.get(&format!("/api/v1/approvals/{id}")).await;
        assert_eq!(one["message"], "Déployer ?");
        let (status, _) = h.get("/api/v1/approvals/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = h.get("/api/v1/approvals?status=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = h.get("/api/v1/approvals?limit=0").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_decision_records_the_deciding_user() {
        let h = harness().await;
        let (_, id) = pending(&h).await;
        let (status, body) = h
            .post(
                &format!("/api/v1/approvals/{id}/decision"),
                json!({ "decision": "approve", "comment": "ok" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "approved");
        assert_eq!(body["comment"], "ok");
        assert!(body["decided_by"].is_string());
        let (status, _) = h
            .post(
                &format!("/api/v1/approvals/{id}/decision"),
                json!({ "decision": "reject" }),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn rejection_and_invalid_decisions() {
        let h = harness().await;
        let (_, id) = pending(&h).await;
        let uri = format!("/api/v1/approvals/{id}/decision");
        let (status, _) = h.post(&uri, json!({ "decision": "peut-etre" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = h
            .post(
                &uri,
                json!({ "decision": "reject", "comment": "x".repeat(1001) }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, body) = h.post(&uri, json!({ "decision": "reject" })).await;
        assert_eq!(
            (status, &body["status"]),
            (StatusCode::OK, &json!("rejected"))
        );
        let (status, _) = h
            .post(
                "/api/v1/approvals/nope/decision",
                json!({ "decision": "approve" }),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn viewers_can_read_but_not_decide() {
        let h = harness().await;
        let (_, id) = pending(&h).await;
        let viewer = h.viewer_token().await;
        let (status, _) = send(&h.app, "GET", "/api/v1/approvals", &bearer(&viewer), None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(
            &h.app,
            "POST",
            &format!("/api/v1/approvals/{id}/decision"),
            &bearer(&viewer),
            Some((
                "application/json",
                json!({ "decision": "approve" }).to_string(),
            )),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            approvals::get(&h.state.db, &id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn approvals_require_authentication() {
        let h = harness().await;
        let (status, _) = send(&h.app, "GET", "/api/v1/approvals", &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
