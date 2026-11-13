use std::future::Future;
use std::time::Duration;

use sqlx::SqlitePool;

pub async fn run(pool: SqlitePool, poll: Duration, shutdown: impl Future<Output = ()>) {
    tokio::pin!(shutdown);
    let mut ticker = tokio::time::interval(poll);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                tracing::info!("worker arrêté");
                return;
            }
            _ = ticker.tick() => {
                if let Err(err) = sqlx::query("SELECT 1").execute(&pool).await {
                    tracing::warn!(%err, "worker: base injoignable");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stops_when_shutdown_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let pool = sw_infrastructure::db::open(&dir.path().join("sw.db"))
            .await
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(run(pool, Duration::from_millis(10), async {
            let _ = rx.await;
        }));
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
    }
}
pub mod processor;
