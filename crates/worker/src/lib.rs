pub mod processor;

use std::future::Future;
use std::time::Duration;

use sqlx::SqlitePool;
use sw_infrastructure::{cron, runs};
use tokio::task::JoinSet;

pub use processor::Settings;

const PURGE_EVERY: Duration = Duration::from_secs(3600);

pub async fn run(
    pool: SqlitePool,
    settings: Settings,
    poll: Duration,
    shutdown: impl Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    let capacity = settings.max_concurrent_runs.max(1);
    let mut tasks = JoinSet::new();
    let mut purge = tokio::time::interval(PURGE_EVERY);
    loop {
        while tasks.len() < capacity {
            let claimed = runs::claim(
                &pool,
                &settings.owner,
                processor::now_ms(),
                settings.lease_ms,
            )
            .await;
            match claimed {
                Ok(Some(run)) => {
                    let (pool, settings) = (pool.clone(), settings.clone());
                    tasks.spawn(async move {
                        if let Err(err) = processor::process_run(&pool, &run, &settings).await {
                            tracing::error!(run_id = %run.id, %err, "traitement du run en échec");
                        }
                    });
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!(%err, "worker: file injoignable");
                    break;
                }
            }
        }
        tokio::select! {
            () = &mut shutdown => {
                while tasks.join_next().await.is_some() {}
                tracing::info!("worker arrêté");
                return;
            }
            Some(joined) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(err) = joined {
                    tracing::error!(%err, "tâche de run interrompue");
                }
            }
            _ = purge.tick(), if settings.retention_days > 0 => {
                match runs::purge_finished(&pool, settings.retention_days).await {
                    Ok(0) => {}
                    Ok(purged) => tracing::info!(purged, "runs expirés supprimés"),
                    Err(err) => tracing::warn!(%err, "purge des runs en échec"),
                }
            }
            () = tokio::time::sleep(poll) => {
                if let Err(err) = cron::fire_due(&pool, processor::now_ms() / 1000).await {
                    tracing::warn!(%err, "ordonnanceur cron en échec");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sw_domain::workflow::Workflow;
    use sw_engine::netguard::Policy;
    use sw_infrastructure::runs::NewEvent;
    use sw_infrastructure::workflows;

    use super::*;

    fn settings() -> Settings {
        Settings {
            owner: "w1".into(),
            lease_ms: 30_000,
            backoff_unit: Duration::from_millis(1),
            policy: Policy::default(),
            max_concurrent_runs: 1,
            retention_days: 0,
        }
    }

    async fn pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = sw_infrastructure::db::open(&dir.path().join("sw.db"))
            .await
            .unwrap();
        (pool, dir)
    }

    async fn enqueue(pool: &SqlitePool, name: &str) -> String {
        let workflow = Workflow::from_yaml(&format!(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {{name: {name}, project: p}}\n\
             spec:\n  trigger: {{type: manual}}\n  steps:\n    - {{id: a, type: transform, with: {{value: 1}}}}\n"
        ))
        .unwrap();
        let record = workflows::create(pool, &workflow).await.unwrap();
        workflows::activate(pool, &record.id, 1).await.unwrap();
        runs::ingest(
            pool,
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
        .run_id
    }

    async fn wait_for_status(pool: &SqlitePool, run_id: &str, status: &str) {
        for _ in 0..200 {
            if runs::get(pool, run_id).await.unwrap().unwrap().status == status {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("le run {run_id} n'a jamais atteint {status}");
    }

    #[tokio::test]
    async fn stops_when_shutdown_resolves() {
        let (pool, _dir) = pool().await;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(pool, settings(), Duration::from_millis(10), async {
            let _ = rx.await;
        }));
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn queued_runs_are_picked_up_and_finished() {
        let (pool, _dir) = pool().await;
        let first = enqueue(&pool, "one").await;
        let second = enqueue(&pool, "two").await;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(
            pool.clone(),
            settings(),
            Duration::from_millis(10),
            async {
                let _ = rx.await;
            },
        ));

        wait_for_status(&pool, &first, "succeeded").await;
        wait_for_status(&pool, &second, "succeeded").await;

        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn a_run_enqueued_while_running_is_picked_up_later() {
        let (pool, _dir) = pool().await;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(
            pool.clone(),
            settings(),
            Duration::from_millis(10),
            async {
                let _ = rx.await;
            },
        ));
        tokio::time::sleep(Duration::from_millis(40)).await;
        let late = enqueue(&pool, "late").await;

        wait_for_status(&pool, &late, "succeeded").await;

        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn a_run_abandoned_by_a_dead_worker_is_recovered() {
        let (pool, _dir) = pool().await;
        let run_id = enqueue(&pool, "crashed").await;
        runs::claim(&pool, "mort", 1, 1).await.unwrap().unwrap();
        runs::set_step(&pool, &run_id, "a", "running", None, None, 1)
            .await
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(
            pool.clone(),
            settings(),
            Duration::from_millis(10),
            async {
                let _ = rx.await;
            },
        ));

        wait_for_status(&pool, &run_id, "succeeded").await;

        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }

    async fn slow_workflow(pool: &SqlitePool, name: &str, server: &wiremock::MockServer) -> String {
        sw_infrastructure::integrations::create(
            pool,
            "p",
            &format!("hook-{name}"),
            &server.uri(),
            &std::collections::BTreeMap::new(),
        )
        .await
        .unwrap();
        let workflow = Workflow::from_yaml(&format!(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {{name: {name}, project: p}}\n\
             spec:\n  trigger: {{type: manual}}\n  steps:\n    - {{id: a, type: http.request, with: {{integration: hook-{name}, path: /slow}}}}\n"
        ))
        .unwrap();
        let record = workflows::create(pool, &workflow).await.unwrap();
        workflows::activate(pool, &record.id, 1).await.unwrap();
        runs::ingest(
            pool,
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
        .run_id
    }

    async fn peak_running(pool: &SqlitePool, window: Duration) -> i64 {
        let mut peak = 0;
        let deadline = tokio::time::Instant::now() + window;
        while tokio::time::Instant::now() < deadline {
            let running: i64 =
                sqlx::query_scalar("SELECT count(*) FROM runs WHERE status = 'running'")
                    .fetch_one(pool)
                    .await
                    .unwrap();
            peak = peak.max(running);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        peak
    }

    async fn peak_with_capacity(capacity: usize) -> i64 {
        let (pool, _dir) = pool().await;
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path("/slow"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_delay(Duration::from_millis(300)),
            )
            .mount(&server)
            .await;
        for name in ["one", "two", "three"] {
            slow_workflow(&pool, name, &server).await;
        }
        let mut config = settings();
        config.policy = sw_engine::netguard::Policy::parse_allowed("127.0.0.0/8").unwrap();
        config.max_concurrent_runs = capacity;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(
            pool.clone(),
            config,
            Duration::from_millis(10),
            async {
                let _ = rx.await;
            },
        ));
        let peak = peak_running(&pool, Duration::from_millis(250)).await;
        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .unwrap()
            .unwrap();
        peak
    }

    #[tokio::test]
    async fn runs_are_processed_in_parallel_up_to_the_configured_capacity() {
        assert_eq!(peak_with_capacity(2).await, 2);
    }

    #[tokio::test]
    async fn a_capacity_of_one_processes_runs_one_at_a_time() {
        assert_eq!(peak_with_capacity(1).await, 1);
    }

    #[tokio::test]
    async fn expired_runs_are_purged_when_retention_is_enabled() {
        let (pool, _dir) = pool().await;
        let old = enqueue(&pool, "old").await;
        sqlx::query(
            "UPDATE runs SET status = 'succeeded', \
             finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-40 days') WHERE id = ?",
        )
        .bind(&old)
        .execute(&pool)
        .await
        .unwrap();
        let mut config = settings();
        config.retention_days = 30;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(
            pool.clone(),
            config,
            Duration::from_millis(10),
            async {
                let _ = rx.await;
            },
        ));
        for _ in 0..200 {
            if runs::get(&pool, &old).await.unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(runs::get(&pool, &old).await.unwrap().is_none());
        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn nothing_is_purged_when_retention_is_disabled() {
        let (pool, _dir) = pool().await;
        let old = enqueue(&pool, "old").await;
        sqlx::query(
            "UPDATE runs SET status = 'succeeded', \
             finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-400 days') WHERE id = ?",
        )
        .bind(&old)
        .execute(&pool)
        .await
        .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(
            pool.clone(),
            settings(),
            Duration::from_millis(10),
            async {
                let _ = rx.await;
            },
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(runs::get(&pool, &old).await.unwrap().is_some());
        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn a_delayed_run_is_released_then_resumed_by_the_loop() {
        let (pool, _dir) = pool().await;
        let workflow = Workflow::from_yaml(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {name: napper, project: p}\n\
             spec:\n  trigger: {type: manual}\n  steps:\n    - {id: nap, type: delay, with: {seconds: 1}}\n    \
             - {id: after, type: transform, needs: [nap], with: {value: 1}}\n",
        )
        .unwrap();
        let record = workflows::create(&pool, &workflow).await.unwrap();
        workflows::activate(&pool, &record.id, 1).await.unwrap();
        let run_id = runs::ingest(
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
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(
            pool.clone(),
            settings(),
            Duration::from_millis(10),
            async {
                let _ = rx.await;
            },
        ));

        wait_for_status(&pool, &run_id, "waiting").await;
        wait_for_status(&pool, &run_id, "succeeded").await;

        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn a_due_cron_workflow_is_fired_and_processed() {
        let (pool, _dir) = pool().await;
        let workflow = Workflow::from_yaml(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {name: ticker, project: p}\n\
             spec:\n  trigger: {type: cron, config: {schedule: '* * * * *'}}\n  steps:\n    \
             - {id: a, type: transform, with: {value: 1}}\n",
        )
        .unwrap();
        let record = workflows::create(&pool, &workflow).await.unwrap();
        workflows::activate(&pool, &record.id, 1).await.unwrap();
        let now = processor::now_ms() / 1000;
        sqlx::query("INSERT INTO schedules (workflow_id, last_fire) VALUES (?, ?)")
            .bind(&record.id)
            .bind(now - 180)
            .execute(&pool)
            .await
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(
            pool.clone(),
            settings(),
            Duration::from_millis(10),
            async {
                let _ = rx.await;
            },
        ));

        let mut seen = None;
        for _ in 0..200 {
            let listed = runs::list(&pool, Some(&record.id), Some("succeeded"), 10)
                .await
                .unwrap();
            if let Some(first) = listed.first() {
                seen = Some(first.clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let done = seen.expect("aucun run cron terminé");
        let event = runs::event_of(&pool, &done.event_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.source, "cron");

        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }
}
