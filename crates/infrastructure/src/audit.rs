use serde_json::Value;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct AuditEvent {
    pub id: i64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub target_type: String,
    pub target_id: String,
    pub details: String,
}

pub async fn record(
    pool: &SqlitePool,
    actor: &str,
    action: &str,
    target_type: &str,
    target_id: &str,
    details: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_events (actor, action, target_type, target_id, details) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(actor)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(details.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list(
    pool: &SqlitePool,
    action: Option<&str>,
    actor: Option<&str>,
    limit: i64,
) -> Result<Vec<AuditEvent>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, at, actor, action, target_type, target_id, details FROM audit_events \
         WHERE (?1 IS NULL OR action = ?1) AND (?2 IS NULL OR actor = ?2) \
         ORDER BY id DESC LIMIT ?3",
    )
    .bind(action)
    .bind(actor)
    .bind(limit)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    async fn pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn events_are_listed_newest_first_with_their_details() {
        let (pool, _dir) = pool().await;
        record(
            &pool,
            "u1",
            "run.cancel",
            "run",
            "r1",
            &json!({ "why": "x" }),
        )
        .await
        .unwrap();
        record(&pool, "u2", "run.retry", "run", "r1", &json!({}))
            .await
            .unwrap();
        let events = list(&pool, None, None, 10).await.unwrap();
        assert_eq!(
            events.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(),
            ["run.retry", "run.cancel"]
        );
        assert_eq!(events[1].details, r#"{"why":"x"}"#);
        assert!(!events[0].at.is_empty());
    }

    #[tokio::test]
    async fn list_filters_by_action_and_actor() {
        let (pool, _dir) = pool().await;
        record(&pool, "u1", "a", "t", "1", &json!({}))
            .await
            .unwrap();
        record(&pool, "u2", "a", "t", "2", &json!({}))
            .await
            .unwrap();
        record(&pool, "u1", "b", "t", "3", &json!({}))
            .await
            .unwrap();
        assert_eq!(list(&pool, Some("a"), None, 10).await.unwrap().len(), 2);
        assert_eq!(list(&pool, None, Some("u1"), 10).await.unwrap().len(), 2);
        assert_eq!(
            list(&pool, Some("a"), Some("u1"), 10).await.unwrap().len(),
            1
        );
        assert_eq!(list(&pool, None, None, 1).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_trail_is_append_only() {
        let (pool, _dir) = pool().await;
        record(&pool, "u1", "a", "t", "1", &json!({}))
            .await
            .unwrap();
        let update = sqlx::query("UPDATE audit_events SET actor = 'x'")
            .execute(&pool)
            .await;
        let delete = sqlx::query("DELETE FROM audit_events").execute(&pool).await;
        assert!(update.is_err() && delete.is_err());
        assert_eq!(list(&pool, None, None, 10).await.unwrap()[0].actor, "u1");
    }
}
