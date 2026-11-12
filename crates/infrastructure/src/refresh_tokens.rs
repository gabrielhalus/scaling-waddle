use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub async fn store(
    pool: &SqlitePool,
    user_id: &str,
    token: &str,
    expires_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO refresh_tokens (token_hash, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(hash_token(token))
        .bind(user_id)
        .bind(expires_at)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn consume(
    pool: &SqlitePool,
    token: &str,
    now: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "DELETE FROM refresh_tokens WHERE token_hash = ? AND expires_at > ? RETURNING user_id",
    )
    .bind(hash_token(token))
    .bind(now)
    .fetch_optional(pool)
    .await
}

pub async fn revoke_all(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM refresh_tokens WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn purge_expired(pool: &SqlitePool, now: i64) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM refresh_tokens WHERE expires_at <= ?")
            .bind(now)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool_with_user() -> (sqlx::SqlitePool, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        let user = crate::users::create(&pool, "a@b.c", "hash", "admin")
            .await
            .unwrap();
        (pool, user.id, dir)
    }

    #[test]
    fn hash_is_stable_and_not_the_token() {
        assert_eq!(hash_token("abc"), hash_token("abc"));
        assert_ne!(hash_token("abc"), "abc");
    }

    #[tokio::test]
    async fn consume_returns_user_once() {
        let (pool, user_id, _dir) = pool_with_user().await;
        store(&pool, &user_id, "tok", i64::MAX).await.unwrap();
        assert_eq!(consume(&pool, "tok", 0).await.unwrap(), Some(user_id));
        assert_eq!(consume(&pool, "tok", 0).await.unwrap(), None);
    }

    #[tokio::test]
    async fn expired_token_is_not_consumable() {
        let (pool, user_id, _dir) = pool_with_user().await;
        store(&pool, &user_id, "tok", 100).await.unwrap();
        assert_eq!(consume(&pool, "tok", 200).await.unwrap(), None);
    }

    #[tokio::test]
    async fn revoke_all_removes_every_session_of_the_user() {
        let (pool, user_id, _dir) = pool_with_user().await;
        store(&pool, &user_id, "t1", i64::MAX).await.unwrap();
        store(&pool, &user_id, "t2", i64::MAX).await.unwrap();
        revoke_all(&pool, &user_id).await.unwrap();
        assert_eq!(consume(&pool, "t1", 0).await.unwrap(), None);
        assert_eq!(consume(&pool, "t2", 0).await.unwrap(), None);
    }

    #[tokio::test]
    async fn purge_expired_drops_only_old_rows() {
        let (pool, user_id, _dir) = pool_with_user().await;
        store(&pool, &user_id, "old", 100).await.unwrap();
        store(&pool, &user_id, "new", 500).await.unwrap();
        assert_eq!(purge_expired(&pool, 200).await.unwrap(), 1);
        assert_eq!(consume(&pool, "new", 200).await.unwrap(), Some(user_id));
    }
}
