use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("connexion SQLite: {0}")]
    Connect(#[from] sqlx::Error),
    #[error("migrations: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub async fn open(path: &Path) -> Result<SqlitePool, DbError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_applies_migrations_in_wal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("sw.db")).await.unwrap();

        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode, "wal");

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sw.db");
        open(&path).await.unwrap();
        open(&path).await.unwrap();
    }
}
