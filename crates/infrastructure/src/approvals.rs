use serde_json::json;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("approbation introuvable")]
    NotFound,
    #[error("approbation déjà traitée")]
    AlreadyDecided,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct Approval {
    pub id: String,
    pub run_id: String,
    pub step_id: String,
    pub status: String,
    pub message: Option<String>,
    pub requested_at: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub comment: Option<String>,
}

const COLUMNS: &str =
    "id, run_id, step_id, status, message, requested_at, decided_by, decided_at, comment";

pub async fn request(
    pool: &SqlitePool,
    run_id: &str,
    step_id: &str,
    message: Option<&str>,
) -> Result<String, sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO approvals (id, run_id, step_id, message) VALUES (?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(run_id)
    .bind(step_id)
    .bind(message)
    .execute(pool)
    .await?;
    sqlx::query_scalar("SELECT id FROM approvals WHERE run_id = ? AND step_id = ?")
        .bind(run_id)
        .bind(step_id)
        .fetch_one(pool)
        .await
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<Approval>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM approvals WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn list(
    pool: &SqlitePool,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<Approval>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM approvals WHERE (?1 IS NULL OR status = ?1) \
         ORDER BY requested_at DESC, rowid DESC LIMIT ?2"
    ))
    .bind(status)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn is_pending(
    pool: &SqlitePool,
    run_id: &str,
    step_id: &str,
) -> Result<bool, sqlx::Error> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM approvals WHERE run_id = ? AND step_id = ? AND status = 'pending'",
    )
    .bind(run_id)
    .bind(step_id)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

pub async fn expire(pool: &SqlitePool, run_id: &str, step_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE approvals SET status = 'expired', decided_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE run_id = ? AND step_id = ? AND status = 'pending'",
    )
    .bind(run_id)
    .bind(step_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn decide(
    pool: &SqlitePool,
    id: &str,
    approve: bool,
    user_id: &str,
    comment: Option<&str>,
) -> Result<Approval, ApprovalError> {
    let mut tx = pool.begin().await?;
    let status = if approve { "approved" } else { "rejected" };
    let updated = sqlx::query_as::<_, (String, String)>(
        "UPDATE approvals SET status = ?2, decided_by = ?3, comment = ?4, \
         decided_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE id = ?1 AND status = 'pending' RETURNING run_id, step_id",
    )
    .bind(id)
    .bind(status)
    .bind(user_id)
    .bind(comment)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((run_id, step_id)) = updated else {
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM approvals WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        return Err(if exists.is_some() {
            ApprovalError::AlreadyDecided
        } else {
            ApprovalError::NotFound
        });
    };
    let output = json!({ "approved": approve, "decided_by": user_id, "comment": comment });
    let error = (!approve).then(|| format!("approbation refusée par {user_id}"));
    sqlx::query(
        "UPDATE step_runs SET status = ?3, output = ?4, error = ?5, wake_at = NULL, \
         finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE run_id = ?1 AND step_id = ?2 AND status = 'waiting'",
    )
    .bind(&run_id)
    .bind(&step_id)
    .bind(if approve { "succeeded" } else { "failed" })
    .bind(output.to_string())
    .bind(error)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE runs SET wake_at = 0 WHERE id = ? AND status = 'waiting'")
        .bind(&run_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    get(pool, id).await?.ok_or(ApprovalError::NotFound)
}

#[cfg(test)]
mod tests {
    use sw_domain::workflow::Workflow;

    use super::*;
    use crate::runs::{self, NewEvent};
    use crate::workflows;

    async fn setup() -> (sqlx::SqlitePool, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        let workflow = Workflow::from_yaml(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {name: w, project: p}\n\
             spec:\n  trigger: {type: manual}\n  steps:\n    - {id: ask, type: approval}\n",
        )
        .unwrap();
        let record = workflows::create(&pool, &workflow).await.unwrap();
        workflows::activate(&pool, &record.id, 1).await.unwrap();
        let run = runs::ingest(
            &pool,
            &NewEvent {
                workflow_id: record.id,
                source: "manual".into(),
                kind: "manual.run".into(),
                payload: json!({}),
                correlation_id: "c".into(),
                idempotency_key: None,
            },
        )
        .await
        .unwrap()
        .run_id;
        (pool, run, dir)
    }

    // Place le run en attente d'une décision, comme le fait le worker.
    async fn parked(pool: &sqlx::SqlitePool, run: &str) -> String {
        runs::claim(pool, "w", 0, 1000).await.unwrap().unwrap();
        let id = request(pool, run, "ask", Some("Déployer ?")).await.unwrap();
        runs::wait_step(pool, run, "ask", None, &json!({ "approval_id": id }))
            .await
            .unwrap();
        runs::park(pool, run, "w", None).await.unwrap();
        id
    }

    #[tokio::test]
    async fn request_is_idempotent_per_step() {
        let (pool, run, _dir) = setup().await;
        let first = request(&pool, &run, "ask", Some("m")).await.unwrap();
        let again = request(&pool, &run, "ask", Some("autre")).await.unwrap();
        assert_eq!(first, again);
        let found = get(&pool, &first).await.unwrap().unwrap();
        assert_eq!(
            (found.status.as_str(), found.message.as_deref()),
            ("pending", Some("m"))
        );
        assert!(is_pending(&pool, &run, "ask").await.unwrap());
    }

    #[tokio::test]
    async fn approving_completes_the_step_and_wakes_the_run() {
        let (pool, run, _dir) = setup().await;
        let id = parked(&pool, &run).await;
        assert!(
            runs::claim(&pool, "w", 10_000_000, 1000)
                .await
                .unwrap()
                .is_none()
        );

        let decided = decide(&pool, &id, true, "user-1", Some("ok"))
            .await
            .unwrap();

        assert_eq!(decided.status, "approved");
        assert_eq!(decided.decided_by.as_deref(), Some("user-1"));
        let step = &runs::steps(&pool, &run).await.unwrap()[0];
        assert_eq!(step.status, "succeeded");
        assert!(
            step.output
                .as_deref()
                .unwrap()
                .contains("\"approved\":true")
        );
        assert_eq!(
            runs::claim(&pool, "w2", 1, 1000).await.unwrap().unwrap().id,
            run
        );
    }

    #[tokio::test]
    async fn rejecting_fails_the_step_with_the_approver_identity() {
        let (pool, run, _dir) = setup().await;
        let id = parked(&pool, &run).await;
        decide(&pool, &id, false, "user-2", None).await.unwrap();
        let step = &runs::steps(&pool, &run).await.unwrap()[0];
        assert_eq!(step.status, "failed");
        assert_eq!(
            step.error.as_deref(),
            Some("approbation refusée par user-2")
        );
    }

    #[tokio::test]
    async fn a_decision_can_only_be_taken_once() {
        let (pool, run, _dir) = setup().await;
        let id = parked(&pool, &run).await;
        decide(&pool, &id, true, "u", None).await.unwrap();
        assert!(matches!(
            decide(&pool, &id, false, "u", None).await,
            Err(ApprovalError::AlreadyDecided)
        ));
        assert!(matches!(
            decide(&pool, "nope", true, "u", None).await,
            Err(ApprovalError::NotFound)
        ));
    }

    #[tokio::test]
    async fn cancelling_the_run_cancels_its_pending_approvals() {
        let (pool, run, _dir) = setup().await;
        let id = parked(&pool, &run).await;
        runs::request_cancel(&pool, &run).await.unwrap();
        assert_eq!(get(&pool, &id).await.unwrap().unwrap().status, "cancelled");
    }

    #[tokio::test]
    async fn expire_marks_only_pending_requests() {
        let (pool, run, _dir) = setup().await;
        let id = request(&pool, &run, "ask", None).await.unwrap();
        expire(&pool, &run, "ask").await.unwrap();
        assert_eq!(get(&pool, &id).await.unwrap().unwrap().status, "expired");
        assert!(!is_pending(&pool, &run, "ask").await.unwrap());
    }

    #[tokio::test]
    async fn list_filters_by_status_newest_first() {
        let (pool, run, _dir) = setup().await;
        let id = request(&pool, &run, "ask", None).await.unwrap();
        assert_eq!(list(&pool, Some("pending"), 10).await.unwrap().len(), 1);
        assert!(list(&pool, Some("approved"), 10).await.unwrap().is_empty());
        decide(&pool, &id, true, "u", None).await.unwrap();
        assert_eq!(list(&pool, Some("approved"), 10).await.unwrap().len(), 1);
        assert_eq!(list(&pool, None, 10).await.unwrap().len(), 1);
    }
}
