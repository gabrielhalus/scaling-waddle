pub mod processor;

use std::future::Future;
use std::time::Duration;

use sqlx::SqlitePool;
use sw_infrastructure::runs;

pub use processor::Settings;

pub async fn run(
    pool: SqlitePool,
    settings: Settings,
    poll: Duration,
    shutdown: impl Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    loop {
        // Vide la file avant d'attendre ; un arrêt demandé est pris en compte entre deux runs.
        loop {
            let claimed = runs::claim(
                &pool,
                &settings.owner,
                processor::now_ms(),
                settings.lease_ms,
            )
            .await;
            match claimed {
                Ok(Some(run)) => {
                    if let Err(err) = processor::process_run(&pool, &run, &settings).await {
                        tracing::error!(run_id = %run.id, %err, "traitement du run en échec");
                    }
                    if futures_ready(&mut shutdown) {
                        tracing::info!("worker arrêté");
                        return;
                    }
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
                tracing::info!("worker arrêté");
                return;
            }
            () = tokio::time::sleep(poll) => {}
        }
    }
}

fn futures_ready(shutdown: &mut std::pin::Pin<&mut impl Future<Output = ()>>) -> bool {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    shutdown.as_mut().poll(&mut context).is_ready()
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
}
