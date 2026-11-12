use serde_json::Value;
use sqlx::{FromRow, SqlitePool};
use sw_domain::workflow::Workflow;

const TERMINAL: &[&str] = &["succeeded", "failed", "cancelled"];

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("workflow introuvable")]
    NotFound,
    #[error("le workflow n'a pas de version active")]
    NotActive,
    #[error("statut final invalide: {0}")]
    InvalidStatus(String),
    #[error("définition illisible: {0}")]
    Definition(#[from] serde_json::Error),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub workflow_id: String,
    pub source: String,
    pub kind: String,
    pub payload: Value,
    pub correlation_id: String,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    pub event_id: String,
    pub run_id: String,
    pub duplicate: bool,
}

#[derive(Debug, Clone, FromRow)]
pub struct Run {
    pub id: String,
    pub workflow_id: String,
    pub version: i64,
    pub event_id: String,
    pub status: String,
    pub error: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct StepRun {
    pub step_id: String,
    pub status: String,
    pub attempts: i64,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EventRecord {
    pub source: String,
    pub kind: String,
    pub payload: Value,
    pub correlation_id: String,
}

const RUN_COLUMNS: &str = "id, workflow_id, version, event_id, status, error, lease_owner, lease_expires_at, created_at, finished_at";

pub async fn ingest(pool: &SqlitePool, event: &NewEvent) -> Result<Ingested, RunError> {
    match ingest_once(pool, event).await {
        Err(RunError::Db(sqlx::Error::Database(err)))
            if err.is_unique_violation() && event.idempotency_key.is_some() =>
        {
            existing(pool, event).await?.ok_or(RunError::NotFound)
        }
        other => other,
    }
}

async fn existing(pool: &SqlitePool, event: &NewEvent) -> Result<Option<Ingested>, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT e.id, r.id FROM events e JOIN runs r ON r.event_id = e.id \
         WHERE e.workflow_id = ? AND e.idempotency_key = ?",
    )
    .bind(&event.workflow_id)
    .bind(&event.idempotency_key)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(event_id, run_id)| Ingested {
        event_id,
        run_id,
        duplicate: true,
    }))
}

async fn ingest_once(pool: &SqlitePool, event: &NewEvent) -> Result<Ingested, RunError> {
    if let Some(found) = existing(pool, event).await? {
        return Ok(found);
    }
    let mut tx = pool.begin().await?;
    let active: Option<Option<i64>> =
        sqlx::query_scalar("SELECT active_version FROM workflows WHERE id = ?")
            .bind(&event.workflow_id)
            .fetch_optional(&mut *tx)
            .await?;
    let version = active
        .ok_or(RunError::NotFound)?
        .ok_or(RunError::NotActive)?;
    let definition: String = sqlx::query_scalar(
        "SELECT definition FROM workflow_versions WHERE workflow_id = ? AND version = ?",
    )
    .bind(&event.workflow_id)
    .bind(version)
    .fetch_one(&mut *tx)
    .await?;
    let workflow: Workflow = serde_json::from_str(&definition)?;

    let event_id = uuid::Uuid::new_v4().to_string();
    let run_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO events (id, workflow_id, source, type, payload, correlation_id, idempotency_key) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&event_id)
    .bind(&event.workflow_id)
    .bind(&event.source)
    .bind(&event.kind)
    .bind(event.payload.to_string())
    .bind(&event.correlation_id)
    .bind(&event.idempotency_key)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO runs (id, workflow_id, version, event_id) VALUES (?, ?, ?, ?)")
        .bind(&run_id)
        .bind(&event.workflow_id)
        .bind(version)
        .bind(&event_id)
        .execute(&mut *tx)
        .await?;
    for step in &workflow.spec.steps {
        sqlx::query("INSERT INTO step_runs (run_id, step_id) VALUES (?, ?)")
            .bind(&run_id)
            .bind(&step.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Ingested {
        event_id,
        run_id,
        duplicate: false,
    })
}

pub async fn claim(
    pool: &SqlitePool,
    owner: &str,
    now_ms: i64,
    lease_ms: i64,
) -> Result<Option<Run>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE runs SET status = 'running', lease_owner = ?1, lease_expires_at = ?2 + ?3 \
         WHERE id = (SELECT id FROM runs \
             WHERE status = 'queued' OR (status = 'running' AND lease_expires_at < ?2) \
             ORDER BY created_at, rowid LIMIT 1) \
         RETURNING {RUN_COLUMNS}"
    ))
    .bind(owner)
    .bind(now_ms)
    .bind(lease_ms)
    .fetch_optional(pool)
    .await
}

pub async fn heartbeat(
    pool: &SqlitePool,
    run_id: &str,
    owner: &str,
    now_ms: i64,
    lease_ms: i64,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE runs SET lease_expires_at = ?3 + ?4 WHERE id = ?1 AND lease_owner = ?2 AND status = 'running'",
    )
    .bind(run_id)
    .bind(owner)
    .bind(now_ms)
    .bind(lease_ms)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn finish(
    pool: &SqlitePool,
    run_id: &str,
    owner: &str,
    status: &str,
    error: Option<&str>,
) -> Result<bool, RunError> {
    if !TERMINAL.contains(&status) {
        return Err(RunError::InvalidStatus(status.to_owned()));
    }
    let updated = sqlx::query(
        "UPDATE runs SET status = ?3, error = ?4, lease_owner = NULL, lease_expires_at = NULL, \
         finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE id = ?1 AND lease_owner = ?2 AND status = 'running'",
    )
    .bind(run_id)
    .bind(owner)
    .bind(status)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<Run>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn event_of(pool: &SqlitePool, event_id: &str) -> Result<Option<EventRecord>, RunError> {
    let row: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT source, type, payload, correlation_id FROM events WHERE id = ?")
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
    row.map(|(source, kind, payload, correlation_id)| {
        Ok(EventRecord {
            source,
            kind,
            payload: serde_json::from_str(&payload)?,
            correlation_id,
        })
    })
    .transpose()
}

pub async fn set_step(
    pool: &SqlitePool,
    run_id: &str,
    step_id: &str,
    status: &str,
    output: Option<&Value>,
    error: Option<&str>,
    attempts: i64,
) -> Result<(), RunError> {
    let updated = sqlx::query(
        "UPDATE step_runs SET status = ?3, output = ?4, error = ?5, attempts = ?6, \
         started_at = CASE WHEN ?3 = 'running' AND started_at IS NULL \
             THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE started_at END, \
         finished_at = CASE WHEN ?3 IN ('succeeded', 'failed', 'skipped', 'cancelled') \
             THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE finished_at END \
         WHERE run_id = ?1 AND step_id = ?2",
    )
    .bind(run_id)
    .bind(step_id)
    .bind(status)
    .bind(output.map(Value::to_string))
    .bind(error)
    .bind(attempts)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(RunError::NotFound);
    }
    Ok(())
}

pub async fn reset_running_steps(pool: &SqlitePool, run_id: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE step_runs SET status = 'pending' WHERE run_id = ? AND status = 'running'",
    )
    .bind(run_id)
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn steps(pool: &SqlitePool, run_id: &str) -> Result<Vec<StepRun>, sqlx::Error> {
    sqlx::query_as(
        "SELECT step_id, status, attempts, output, error, started_at, finished_at FROM step_runs \
         WHERE run_id = ? ORDER BY rowid",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sw_domain::workflow::Workflow;

    use super::*;
    use crate::workflows;

    fn definition() -> Workflow {
        Workflow::from_yaml(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {name: w, project: p}\n\
             spec:\n  trigger: {type: manual}\n  steps:\n    - {id: a, type: delay}\n    \
             - {id: b, type: delay, needs: [a]}\n",
        )
        .unwrap()
    }

    fn event(workflow_id: &str, key: Option<&str>) -> NewEvent {
        NewEvent {
            workflow_id: workflow_id.to_owned(),
            source: "manual".into(),
            kind: "run".into(),
            payload: json!({ "n": 1 }),
            correlation_id: "corr-1".into(),
            idempotency_key: key.map(str::to_owned),
        }
    }

    async fn setup() -> (sqlx::SqlitePool, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        let record = workflows::create(&pool, &definition()).await.unwrap();
        workflows::activate(&pool, &record.id, 1).await.unwrap();
        (pool, record.id, dir)
    }

    #[tokio::test]
    async fn ingest_creates_a_queued_run_with_pending_steps() {
        let (pool, wf, _dir) = setup().await;
        let ingested = ingest(&pool, &event(&wf, None)).await.unwrap();
        assert!(!ingested.duplicate);
        let run = get(&pool, &ingested.run_id).await.unwrap().unwrap();
        assert_eq!((run.status.as_str(), run.version), ("queued", 1));
        let steps = steps(&pool, &run.id).await.unwrap();
        assert_eq!(
            steps
                .iter()
                .map(|s| (s.step_id.as_str(), s.status.as_str()))
                .collect::<Vec<_>>(),
            [("a", "pending"), ("b", "pending")]
        );
    }

    #[tokio::test]
    async fn same_idempotency_key_returns_the_existing_run() {
        let (pool, wf, _dir) = setup().await;
        let first = ingest(&pool, &event(&wf, Some("k"))).await.unwrap();
        let second = ingest(&pool, &event(&wf, Some("k"))).await.unwrap();
        assert!(second.duplicate);
        assert_eq!(second.run_id, first.run_id);
        let other = ingest(&pool, &event(&wf, Some("k2"))).await.unwrap();
        assert_ne!(other.run_id, first.run_id);
    }

    #[tokio::test]
    async fn events_without_a_key_are_never_deduplicated() {
        let (pool, wf, _dir) = setup().await;
        let a = ingest(&pool, &event(&wf, None)).await.unwrap();
        let b = ingest(&pool, &event(&wf, None)).await.unwrap();
        assert_ne!(a.run_id, b.run_id);
    }

    #[tokio::test]
    async fn ingest_rejects_unknown_or_inactive_workflows() {
        let (pool, wf, _dir) = setup().await;
        assert!(matches!(
            ingest(&pool, &event("nope", None)).await,
            Err(RunError::NotFound)
        ));
        workflows::deactivate(&pool, &wf).await.unwrap();
        assert!(matches!(
            ingest(&pool, &event(&wf, None)).await,
            Err(RunError::NotActive)
        ));
    }

    #[tokio::test]
    async fn run_is_pinned_to_the_version_active_at_ingestion() {
        let (pool, wf, _dir) = setup().await;
        let mut v2 = definition();
        v2.spec.steps.pop();
        workflows::add_version(&pool, &wf, &v2).await.unwrap();
        let first = ingest(&pool, &event(&wf, None)).await.unwrap();
        workflows::activate(&pool, &wf, 2).await.unwrap();
        let second = ingest(&pool, &event(&wf, None)).await.unwrap();
        assert_eq!(get(&pool, &first.run_id).await.unwrap().unwrap().version, 1);
        assert_eq!(
            get(&pool, &second.run_id).await.unwrap().unwrap().version,
            2
        );
        assert_eq!(steps(&pool, &second.run_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn claim_is_fifo_and_exclusive() {
        let (pool, wf, _dir) = setup().await;
        let first = ingest(&pool, &event(&wf, None)).await.unwrap();
        let second = ingest(&pool, &event(&wf, None)).await.unwrap();
        let a = claim(&pool, "w1", 1_000, 30_000).await.unwrap().unwrap();
        let b = claim(&pool, "w2", 1_000, 30_000).await.unwrap().unwrap();
        assert_eq!(
            (a.id.as_str(), a.status.as_str()),
            (first.run_id.as_str(), "running")
        );
        assert_eq!(b.id, second.run_id);
        assert!(claim(&pool, "w3", 1_000, 30_000).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_lease_makes_the_run_claimable_again() {
        let (pool, wf, _dir) = setup().await;
        let ingested = ingest(&pool, &event(&wf, None)).await.unwrap();
        claim(&pool, "w1", 1_000, 30_000).await.unwrap().unwrap();
        assert!(claim(&pool, "w2", 20_000, 30_000).await.unwrap().is_none());
        let taken = claim(&pool, "w2", 31_001, 30_000).await.unwrap().unwrap();
        assert_eq!(taken.id, ingested.run_id);
        assert_eq!(taken.lease_owner.as_deref(), Some("w2"));
    }

    #[tokio::test]
    async fn heartbeat_extends_only_for_the_owner() {
        let (pool, wf, _dir) = setup().await;
        let ingested = ingest(&pool, &event(&wf, None)).await.unwrap();
        claim(&pool, "w1", 1_000, 30_000).await.unwrap().unwrap();
        assert!(
            !heartbeat(&pool, &ingested.run_id, "intrus", 2_000, 30_000)
                .await
                .unwrap()
        );
        assert!(
            heartbeat(&pool, &ingested.run_id, "w1", 25_000, 30_000)
                .await
                .unwrap()
        );
        assert!(claim(&pool, "w2", 40_000, 30_000).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn finish_requires_the_lease_owner_and_is_final() {
        let (pool, wf, _dir) = setup().await;
        let ingested = ingest(&pool, &event(&wf, None)).await.unwrap();
        claim(&pool, "w1", 1_000, 30_000).await.unwrap().unwrap();
        assert!(
            !finish(&pool, &ingested.run_id, "intrus", "succeeded", None)
                .await
                .unwrap()
        );
        assert!(
            finish(&pool, &ingested.run_id, "w1", "failed", Some("boom"))
                .await
                .unwrap()
        );
        let run = get(&pool, &ingested.run_id).await.unwrap().unwrap();
        assert_eq!(
            (run.status.as_str(), run.error.as_deref()),
            ("failed", Some("boom"))
        );
        assert!(run.finished_at.is_some());
        assert!(
            claim(&pool, "w2", 99_999_999, 30_000)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn event_returns_the_stored_payload_and_metadata() {
        let (pool, wf, _dir) = setup().await;
        let ingested = ingest(&pool, &event(&wf, None)).await.unwrap();
        let stored = event_of(&pool, &ingested.event_id).await.unwrap().unwrap();
        assert_eq!(stored.payload, json!({ "n": 1 }));
        assert_eq!(
            (stored.source.as_str(), stored.kind.as_str()),
            ("manual", "run")
        );
        assert_eq!(stored.correlation_id, "corr-1");
        assert!(event_of(&pool, "nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_step_records_status_output_error_and_timestamps() {
        let (pool, wf, _dir) = setup().await;
        let run_id = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        set_step(&pool, &run_id, "a", "running", None, None, 1)
            .await
            .unwrap();
        let running = &steps(&pool, &run_id).await.unwrap()[0];
        assert_eq!((running.status.as_str(), running.attempts), ("running", 1));
        assert!(running.started_at.is_some() && running.finished_at.is_none());

        set_step(
            &pool,
            &run_id,
            "a",
            "succeeded",
            Some(&json!({ "ok": 1 })),
            None,
            1,
        )
        .await
        .unwrap();
        let done = &steps(&pool, &run_id).await.unwrap()[0];
        assert_eq!(done.output.as_deref(), Some("{\"ok\":1}"));
        assert!(done.finished_at.is_some());

        set_step(&pool, &run_id, "b", "failed", None, Some("boom"), 3)
            .await
            .unwrap();
        let failed = &steps(&pool, &run_id).await.unwrap()[1];
        assert_eq!(
            (failed.error.as_deref(), failed.attempts),
            (Some("boom"), 3)
        );
    }

    #[tokio::test]
    async fn set_step_rejects_unknown_steps() {
        let (pool, wf, _dir) = setup().await;
        let run_id = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        assert!(
            set_step(&pool, &run_id, "ghost", "running", None, None, 1)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn reset_running_steps_requeues_only_interrupted_work() {
        let (pool, wf, _dir) = setup().await;
        let run_id = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        set_step(&pool, &run_id, "a", "succeeded", None, None, 1)
            .await
            .unwrap();
        set_step(&pool, &run_id, "b", "running", None, None, 1)
            .await
            .unwrap();
        assert_eq!(reset_running_steps(&pool, &run_id).await.unwrap(), 1);
        let statuses: Vec<String> = steps(&pool, &run_id)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.status)
            .collect();
        assert_eq!(statuses, ["succeeded", "pending"]);
    }

    #[tokio::test]
    async fn finish_rejects_a_non_terminal_status() {
        let (pool, wf, _dir) = setup().await;
        let ingested = ingest(&pool, &event(&wf, None)).await.unwrap();
        claim(&pool, "w1", 1_000, 30_000).await.unwrap().unwrap();
        assert!(
            finish(&pool, &ingested.run_id, "w1", "queued", None)
                .await
                .is_err()
        );
    }
}
