use serde_json::json;
use sqlx::SqlitePool;
use sw_domain::cron;

use crate::runs::{self, Ingested, NewEvent};

const ACTIVE_CRON: &str = "SELECT w.id, json_extract(v.definition, '$.spec.trigger.config.schedule') \
     FROM workflows w JOIN workflow_versions v \
         ON v.workflow_id = w.id AND v.version = w.active_version \
     WHERE json_extract(v.definition, '$.spec.trigger.type') = 'cron'";

// Déclenche au plus une occurrence par workflow : la plus récente échue, sans rattrapage.
pub async fn fire_due(pool: &SqlitePool, now_secs: i64) -> Result<Vec<Ingested>, sqlx::Error> {
    let active: Vec<(String, Option<String>)> = sqlx::query_as(ACTIVE_CRON).fetch_all(pool).await?;
    let ids: Vec<&str> = active.iter().map(|(id, _)| id.as_str()).collect();
    let known: Vec<String> = sqlx::query_scalar("SELECT workflow_id FROM schedules")
        .fetch_all(pool)
        .await?;
    for stale in known.iter().filter(|id| !ids.contains(&id.as_str())) {
        sqlx::query("DELETE FROM schedules WHERE workflow_id = ?")
            .bind(stale)
            .execute(pool)
            .await?;
    }

    let mut fired = Vec::new();
    for (workflow_id, schedule) in &active {
        let Some(schedule) = schedule else { continue };
        let last: Option<i64> =
            sqlx::query_scalar("SELECT last_fire FROM schedules WHERE workflow_id = ?")
                .bind(workflow_id)
                .fetch_optional(pool)
                .await?;
        let Some(last) = last else {
            sqlx::query("INSERT INTO schedules (workflow_id, last_fire) VALUES (?, ?)")
                .bind(workflow_id)
                .bind(now_secs)
                .execute(pool)
                .await?;
            continue;
        };
        let Ok(Some(due)) = cron::latest_between(schedule, last, now_secs) else {
            continue;
        };
        let event = NewEvent {
            workflow_id: workflow_id.clone(),
            source: "cron".into(),
            kind: "cron.tick".into(),
            payload: json!({ "scheduled_for": due }),
            correlation_id: uuid::Uuid::new_v4().to_string(),
            idempotency_key: Some(format!("cron:{due}")),
        };
        match runs::ingest(pool, &event).await {
            Ok(ingested) => {
                if !ingested.duplicate {
                    fired.push(ingested);
                }
            }
            Err(err) => tracing::warn!(%workflow_id, %err, "déclenchement cron en échec"),
        }
        sqlx::query("UPDATE schedules SET last_fire = ? WHERE workflow_id = ?")
            .bind(due)
            .bind(workflow_id)
            .execute(pool)
            .await?;
    }
    Ok(fired)
}

#[cfg(test)]
mod tests {
    use sw_domain::workflow::Workflow;

    use super::*;
    use crate::workflows;

    // 2026-11-13T10:00:00Z, aligné sur 5 minutes.
    const T: i64 = 1_794_564_000;

    async fn setup(trigger: &str) -> (sqlx::SqlitePool, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        let workflow = Workflow::from_yaml(&format!(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {{name: w, project: p}}\n\
             spec:\n  trigger: {trigger}\n  steps:\n    - {{id: a, type: transform, with: {{value: 1}}}}\n"
        ))
        .unwrap();
        let record = workflows::create(&pool, &workflow).await.unwrap();
        workflows::activate(&pool, &record.id, 1).await.unwrap();
        (pool, record.id, dir)
    }

    const EVERY_5: &str = "{type: cron, config: {schedule: '*/5 * * * *'}}";

    #[tokio::test]
    async fn the_first_pass_only_records_the_starting_point() {
        let (pool, _, _dir) = setup(EVERY_5).await;
        assert!(fire_due(&pool, T).await.unwrap().is_empty());
        assert!(fire_due(&pool, T + 299).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_due_occurrence_queues_one_cron_run() {
        let (pool, wf, _dir) = setup(EVERY_5).await;
        fire_due(&pool, T).await.unwrap();
        let fired = fire_due(&pool, T + 301).await.unwrap();
        assert_eq!(fired.len(), 1);
        let run = runs::get(&pool, &fired[0].run_id).await.unwrap().unwrap();
        assert_eq!((run.status.as_str(), run.workflow_id), ("queued", wf));
        let event = runs::event_of(&pool, &run.event_id).await.unwrap().unwrap();
        assert_eq!(
            (event.source.as_str(), event.kind.as_str()),
            ("cron", "cron.tick")
        );
        assert_eq!(event.payload, json!({ "scheduled_for": T + 300 }));
    }

    #[tokio::test]
    async fn the_same_occurrence_never_fires_twice() {
        let (pool, _, _dir) = setup(EVERY_5).await;
        fire_due(&pool, T).await.unwrap();
        assert_eq!(fire_due(&pool, T + 301).await.unwrap().len(), 1);
        assert!(fire_due(&pool, T + 302).await.unwrap().is_empty());
        sqlx::query("UPDATE schedules SET last_fire = ?")
            .bind(T)
            .execute(&pool)
            .await
            .unwrap();
        assert!(fire_due(&pool, T + 302).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn missed_occurrences_collapse_into_a_single_run() {
        let (pool, _, _dir) = setup(EVERY_5).await;
        fire_due(&pool, T).await.unwrap();
        let fired = fire_due(&pool, T + 3600).await.unwrap();
        assert_eq!(fired.len(), 1);
        let run = runs::get(&pool, &fired[0].run_id).await.unwrap().unwrap();
        let event = runs::event_of(&pool, &run.event_id).await.unwrap().unwrap();
        assert_eq!(event.payload, json!({ "scheduled_for": T + 3600 }));
    }

    #[tokio::test]
    async fn non_cron_and_inactive_workflows_are_ignored() {
        let (pool, _, _dir) = setup("{type: manual}").await;
        fire_due(&pool, T).await.unwrap();
        assert!(fire_due(&pool, T + 3600).await.unwrap().is_empty());
        let (pool, wf2, _dir2) = setup(EVERY_5).await;
        workflows::deactivate(&pool, &wf2).await.unwrap();
        fire_due(&pool, T).await.unwrap();
        assert!(fire_due(&pool, T + 3600).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn reactivation_restarts_the_schedule_without_backfill() {
        let (pool, wf, _dir) = setup(EVERY_5).await;
        fire_due(&pool, T).await.unwrap();
        workflows::deactivate(&pool, &wf).await.unwrap();
        fire_due(&pool, T + 10).await.unwrap();
        workflows::activate(&pool, &wf, 1).await.unwrap();
        assert!(fire_due(&pool, T + 7200).await.unwrap().is_empty());
        assert_eq!(fire_due(&pool, T + 7200 + 300).await.unwrap().len(), 1);
    }
}
