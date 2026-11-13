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
    #[error("run déjà terminé")]
    AlreadyFinished,
    #[error("seul un run échoué ou annulé peut être relancé")]
    NotRetryable,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancellation {
    Cancelled,
    Requested,
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
    pub wake_at: Option<i64>,
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

const WORKFLOW_LIMIT: &str = "(SELECT json_extract(v.definition, '$.spec.concurrency') \
    FROM workflow_versions v WHERE v.workflow_id = r.workflow_id AND v.version = r.version)";

pub async fn claim(
    pool: &SqlitePool,
    owner: &str,
    now_ms: i64,
    lease_ms: i64,
) -> Result<Option<Run>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE runs SET status = 'running', lease_owner = ?1, lease_expires_at = ?2 + ?3, wake_at = NULL \
         WHERE id = (SELECT r.id FROM runs r \
             WHERE (r.status = 'queued' OR (r.status = 'running' AND r.lease_expires_at < ?2) \
                 OR (r.status = 'waiting' AND r.wake_at <= ?2)) \
             AND ({WORKFLOW_LIMIT} IS NULL OR \
                 (SELECT count(*) FROM runs o WHERE o.workflow_id = r.workflow_id \
                     AND o.status = 'running' AND o.lease_expires_at >= ?2) < {WORKFLOW_LIMIT}) \
             ORDER BY r.created_at, r.rowid LIMIT 1) \
         RETURNING {RUN_COLUMNS}"
    ))
    .bind(owner)
    .bind(now_ms)
    .bind(lease_ms)
    .fetch_optional(pool)
    .await
}

pub async fn park(
    pool: &SqlitePool,
    run_id: &str,
    owner: &str,
    wake_at_ms: i64,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE runs SET status = 'waiting', wake_at = ?3, lease_owner = NULL, lease_expires_at = NULL \
         WHERE id = ?1 AND lease_owner = ?2 AND status = 'running'",
    )
    .bind(run_id)
    .bind(owner)
    .bind(wake_at_ms)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn wait_step(
    pool: &SqlitePool,
    run_id: &str,
    step_id: &str,
    wake_at_ms: i64,
    output: &Value,
) -> Result<(), RunError> {
    let updated = sqlx::query(
        "UPDATE step_runs SET status = 'waiting', wake_at = ?3, output = ?4, \
         started_at = COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) \
         WHERE run_id = ?1 AND step_id = ?2",
    )
    .bind(run_id)
    .bind(step_id)
    .bind(wake_at_ms)
    .bind(output.to_string())
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(RunError::NotFound);
    }
    Ok(())
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

pub async fn list(
    pool: &SqlitePool,
    workflow_id: Option<&str>,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<Run>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {RUN_COLUMNS} FROM runs \
         WHERE (?1 IS NULL OR workflow_id = ?1) AND (?2 IS NULL OR status = ?2) \
         ORDER BY created_at DESC, rowid DESC LIMIT ?3"
    ))
    .bind(workflow_id)
    .bind(status)
    .bind(limit)
    .fetch_all(pool)
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

pub async fn request_cancel(pool: &SqlitePool, run_id: &str) -> Result<Cancellation, RunError> {
    let mut tx = pool.begin().await?;
    let cancelled = sqlx::query(
        "UPDATE runs SET status = 'cancelled', finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE id = ? AND status IN ('queued', 'waiting')",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    if cancelled.rows_affected() == 1 {
        sqlx::query(
            "UPDATE step_runs SET status = 'cancelled', \
             finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE run_id = ? AND status IN ('pending', 'waiting')",
        )
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(Cancellation::Cancelled);
    }
    let requested =
        sqlx::query("UPDATE runs SET cancel_requested = 1 WHERE id = ? AND status = 'running'")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
    if requested.rows_affected() == 1 {
        tx.commit().await?;
        return Ok(Cancellation::Requested);
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?;
    Err(if exists.is_some() {
        RunError::AlreadyFinished
    } else {
        RunError::NotFound
    })
}

pub async fn cancel_requested(pool: &SqlitePool, run_id: &str) -> Result<bool, sqlx::Error> {
    let flag: Option<i64> = sqlx::query_scalar("SELECT cancel_requested FROM runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(pool)
        .await?;
    Ok(flag == Some(1))
}

pub async fn cancel_pending_steps(pool: &SqlitePool, run_id: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE step_runs SET status = 'cancelled', \
         finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE run_id = ? AND status IN ('pending', 'waiting')",
    )
    .bind(run_id)
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn retry(pool: &SqlitePool, run_id: &str) -> Result<String, RunError> {
    let mut tx = pool.begin().await?;
    let original: Run = sqlx::query_as(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?"))
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RunError::NotFound)?;
    if !matches!(original.status.as_str(), "failed" | "cancelled") {
        return Err(RunError::NotRetryable);
    }
    let new_id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO runs (id, workflow_id, version, event_id) VALUES (?, ?, ?, ?)")
        .bind(&new_id)
        .bind(&original.workflow_id)
        .bind(original.version)
        .bind(&original.event_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO step_runs (run_id, step_id) SELECT ?, step_id FROM step_runs WHERE run_id = ? ORDER BY rowid",
    )
    .bind(&new_id)
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(new_id)
}

pub async fn purge_finished(pool: &SqlitePool, older_than_days: u32) -> Result<u64, sqlx::Error> {
    let age = format!("-{older_than_days} days");
    let mut tx = pool.begin().await?;
    let runs = sqlx::query(
        "DELETE FROM runs WHERE status IN ('succeeded', 'failed', 'cancelled') \
         AND finished_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?)",
    )
    .bind(&age)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query(
        "DELETE FROM events WHERE created_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?) \
         AND NOT EXISTS (SELECT 1 FROM runs WHERE runs.event_id = events.id)",
    )
    .bind(&age)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(runs)
}

pub async fn steps(pool: &SqlitePool, run_id: &str) -> Result<Vec<StepRun>, sqlx::Error> {
    sqlx::query_as(
        "SELECT step_id, status, attempts, output, error, started_at, finished_at, wake_at FROM step_runs \
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
    async fn list_is_newest_first_and_filterable() {
        let (pool, wf, _dir) = setup().await;
        let first = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        let second = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        let third = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        claim(&pool, "w1", 1_000, 30_000).await.unwrap().unwrap();
        finish(&pool, &first, "w1", "failed", Some("boom"))
            .await
            .unwrap();

        let all = list(&pool, None, None, 10).await.unwrap();
        assert_eq!(
            all.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            [&third, &second, &first]
        );
        let failed = list(&pool, None, Some("failed"), 10).await.unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].id, first);
        assert_eq!(
            list(&pool, Some(&wf), Some("queued"), 10)
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(
            list(&pool, Some("autre"), None, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(list(&pool, None, None, 2).await.unwrap().len(), 2);
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

    #[tokio::test]
    async fn cancelling_a_queued_run_ends_it_at_once() {
        let (pool, wf, _dir) = setup().await;
        let run = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        assert_eq!(
            request_cancel(&pool, &run).await.unwrap(),
            Cancellation::Cancelled
        );
        let found = get(&pool, &run).await.unwrap().unwrap();
        assert_eq!(found.status, "cancelled");
        assert!(found.finished_at.is_some());
        assert!(
            steps(&pool, &run)
                .await
                .unwrap()
                .iter()
                .all(|s| s.status == "cancelled")
        );
        assert!(claim(&pool, "w", 0, 1000).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn cancelling_a_running_run_only_raises_the_flag() {
        let (pool, wf, _dir) = setup().await;
        let run = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        assert!(!cancel_requested(&pool, &run).await.unwrap());
        assert_eq!(
            request_cancel(&pool, &run).await.unwrap(),
            Cancellation::Requested
        );
        assert!(cancel_requested(&pool, &run).await.unwrap());
        assert_eq!(get(&pool, &run).await.unwrap().unwrap().status, "running");
        assert_eq!(cancel_pending_steps(&pool, &run).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn cancel_rejects_finished_and_unknown_runs() {
        let (pool, wf, _dir) = setup().await;
        let run = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        finish(&pool, &run, "w", "succeeded", None).await.unwrap();
        assert!(matches!(
            request_cancel(&pool, &run).await,
            Err(RunError::AlreadyFinished)
        ));
        assert!(matches!(
            request_cancel(&pool, "nope").await,
            Err(RunError::NotFound)
        ));
    }

    #[tokio::test]
    async fn retry_clones_a_failed_run_with_fresh_steps() {
        let (pool, wf, _dir) = setup().await;
        let first = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        set_step(&pool, &first, "a", "failed", None, Some("boom"), 1)
            .await
            .unwrap();
        finish(&pool, &first, "w", "failed", Some("boom"))
            .await
            .unwrap();
        let second = retry(&pool, &first).await.unwrap();
        let (old, new) = (
            get(&pool, &first).await.unwrap().unwrap(),
            get(&pool, &second).await.unwrap().unwrap(),
        );
        assert_ne!(first, second);
        assert_eq!(
            (new.status.as_str(), new.event_id, new.version),
            ("queued", old.event_id, old.version)
        );
        let fresh = steps(&pool, &second).await.unwrap();
        assert_eq!(
            fresh.iter().map(|s| s.step_id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(
            fresh
                .iter()
                .all(|s| s.status == "pending" && s.attempts == 0)
        );
        assert_eq!(get(&pool, &first).await.unwrap().unwrap().status, "failed");
    }

    #[tokio::test]
    async fn retry_only_applies_to_failed_or_cancelled_runs() {
        let (pool, wf, _dir) = setup().await;
        let run = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        assert!(matches!(
            retry(&pool, &run).await,
            Err(RunError::NotRetryable)
        ));
        assert!(matches!(
            retry(&pool, "nope").await,
            Err(RunError::NotFound)
        ));
        request_cancel(&pool, &run).await.unwrap();
        assert!(retry(&pool, &run).await.is_ok());
    }

    async fn limited_workflow(pool: &sqlx::SqlitePool, name: &str, limit: u32) -> String {
        let mut workflow = definition();
        workflow.metadata.name = name.into();
        workflow.spec.concurrency = Some(limit);
        let record = workflows::create(pool, &workflow).await.unwrap();
        workflows::activate(pool, &record.id, 1).await.unwrap();
        record.id
    }

    #[tokio::test]
    async fn claim_respects_the_workflow_concurrency_limit() {
        let (pool, _, _dir) = setup().await;
        let limited = limited_workflow(&pool, "limited", 1).await;
        let first = ingest(&pool, &event(&limited, None)).await.unwrap().run_id;
        let second = ingest(&pool, &event(&limited, None)).await.unwrap().run_id;
        assert_eq!(claim(&pool, "w", 0, 1000).await.unwrap().unwrap().id, first);
        assert!(claim(&pool, "w", 1, 1000).await.unwrap().is_none());
        finish(&pool, &first, "w", "succeeded", None).await.unwrap();
        assert_eq!(
            claim(&pool, "w", 2, 1000).await.unwrap().unwrap().id,
            second
        );
    }

    #[tokio::test]
    async fn a_limited_workflow_does_not_block_other_workflows() {
        let (pool, free, _dir) = setup().await;
        let limited = limited_workflow(&pool, "limited", 1).await;
        ingest(&pool, &event(&limited, None)).await.unwrap();
        ingest(&pool, &event(&limited, None)).await.unwrap();
        let other = ingest(&pool, &event(&free, None)).await.unwrap().run_id;
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        assert_eq!(claim(&pool, "w", 1, 1000).await.unwrap().unwrap().id, other);
    }

    #[tokio::test]
    async fn a_crashed_run_does_not_hold_a_concurrency_slot() {
        let (pool, _, _dir) = setup().await;
        let limited = limited_workflow(&pool, "limited", 1).await;
        let first = ingest(&pool, &event(&limited, None)).await.unwrap().run_id;
        ingest(&pool, &event(&limited, None)).await.unwrap();
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        let reclaimed = claim(&pool, "w2", 5000, 1000).await.unwrap().unwrap();
        assert_eq!(reclaimed.id, first);
    }

    #[tokio::test]
    async fn a_higher_limit_allows_parallel_runs() {
        let (pool, _, _dir) = setup().await;
        let limited = limited_workflow(&pool, "limited", 2).await;
        for _ in 0..3 {
            ingest(&pool, &event(&limited, None)).await.unwrap();
        }
        assert!(claim(&pool, "w", 0, 1000).await.unwrap().is_some());
        assert!(claim(&pool, "w", 1, 1000).await.unwrap().is_some());
        assert!(claim(&pool, "w", 2, 1000).await.unwrap().is_none());
    }

    async fn aged_run(pool: &sqlx::SqlitePool, wf: &str, status: &str, days: i64) -> String {
        let run = ingest(pool, &event(wf, None)).await.unwrap();
        let stamp = format!("strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-{days} days')");
        sqlx::query(&format!(
            "UPDATE runs SET status = ?, finished_at = {stamp} WHERE id = ?"
        ))
        .bind(status)
        .bind(&run.run_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "UPDATE events SET created_at = {stamp} WHERE id = ?"
        ))
        .bind(&run.event_id)
        .execute(pool)
        .await
        .unwrap();
        run.run_id
    }

    #[tokio::test]
    async fn purge_removes_old_finished_runs_with_their_steps_and_events() {
        let (pool, wf, _dir) = setup().await;
        let old = aged_run(&pool, &wf, "succeeded", 40).await;
        let recent = aged_run(&pool, &wf, "failed", 5).await;
        assert_eq!(purge_finished(&pool, 30).await.unwrap(), 1);
        assert!(get(&pool, &old).await.unwrap().is_none());
        assert!(steps(&pool, &old).await.unwrap().is_empty());
        assert!(get(&pool, &recent).await.unwrap().is_some());
        let events: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(events, 1);
    }

    #[tokio::test]
    async fn purge_never_touches_unfinished_runs() {
        let (pool, wf, _dir) = setup().await;
        let queued = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        sqlx::query("UPDATE runs SET created_at = '2000-01-01T00:00:00.000Z' WHERE id = ?")
            .bind(&queued)
            .execute(&pool)
            .await
            .unwrap();
        let running = aged_run(&pool, &wf, "running", 90).await;
        assert_eq!(purge_finished(&pool, 30).await.unwrap(), 0);
        assert!(get(&pool, &queued).await.unwrap().is_some());
        assert!(get(&pool, &running).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn purge_keeps_events_still_referenced_by_a_run() {
        let (pool, wf, _dir) = setup().await;
        let old = aged_run(&pool, &wf, "failed", 40).await;
        let retried = retry(&pool, &old).await.unwrap();
        purge_finished(&pool, 30).await.unwrap();
        assert!(get(&pool, &retried).await.unwrap().is_some());
        assert!(
            event_of(
                &pool,
                &get(&pool, &retried).await.unwrap().unwrap().event_id
            )
            .await
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test]
    async fn a_parked_run_is_claimed_only_once_its_wake_time_has_come() {
        let (pool, wf, _dir) = setup().await;
        let run = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        wait_step(&pool, &run, "a", 5000, &json!({ "seconds": 5 }))
            .await
            .unwrap();
        assert!(park(&pool, &run, "w", 5000).await.unwrap());
        let parked = get(&pool, &run).await.unwrap().unwrap();
        assert_eq!(
            (parked.status.as_str(), parked.lease_owner),
            ("waiting", None)
        );
        assert!(claim(&pool, "w", 4999, 1000).await.unwrap().is_none());
        let woken = claim(&pool, "w", 5000, 1000).await.unwrap().unwrap();
        assert_eq!(woken.id, run);
        assert_eq!(woken.status, "running");
        let step = &steps(&pool, &run).await.unwrap()[0];
        assert_eq!(
            (step.status.as_str(), step.wake_at),
            ("waiting", Some(5000))
        );
    }

    #[tokio::test]
    async fn park_requires_the_lease_owner() {
        let (pool, wf, _dir) = setup().await;
        let run = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        assert!(!park(&pool, &run, "intrus", 10).await.unwrap());
        assert_eq!(get(&pool, &run).await.unwrap().unwrap().status, "running");
    }

    #[tokio::test]
    async fn a_waiting_run_can_be_cancelled_at_once() {
        let (pool, wf, _dir) = setup().await;
        let run = ingest(&pool, &event(&wf, None)).await.unwrap().run_id;
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        wait_step(&pool, &run, "a", 9000, &json!({})).await.unwrap();
        park(&pool, &run, "w", 9000).await.unwrap();
        assert_eq!(
            request_cancel(&pool, &run).await.unwrap(),
            Cancellation::Cancelled
        );
        assert!(claim(&pool, "w", 99_999, 1000).await.unwrap().is_none());
        assert!(
            steps(&pool, &run)
                .await
                .unwrap()
                .iter()
                .all(|s| s.status == "cancelled")
        );
    }

    #[tokio::test]
    async fn waiting_runs_do_not_hold_a_concurrency_slot() {
        let (pool, _, _dir) = setup().await;
        let limited = limited_workflow(&pool, "limited", 1).await;
        let first = ingest(&pool, &event(&limited, None)).await.unwrap().run_id;
        let second = ingest(&pool, &event(&limited, None)).await.unwrap().run_id;
        claim(&pool, "w", 0, 1000).await.unwrap().unwrap();
        wait_step(&pool, &first, "a", 60_000, &json!({}))
            .await
            .unwrap();
        park(&pool, &first, "w", 60_000).await.unwrap();
        assert_eq!(
            claim(&pool, "w", 1, 1000).await.unwrap().unwrap().id,
            second
        );
    }
}
