use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub role: String,
}

pub async fn create(
    pool: &SqlitePool,
    email: &str,
    password_hash: &str,
    role: &str,
) -> Result<User, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO users (id, email, password_hash, role) VALUES (?, ?, ?, ?) \
         RETURNING id, email, password_hash, role",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(email)
    .bind(password_hash)
    .bind(role)
    .fetch_one(pool)
    .await
}

pub async fn find_by_email(pool: &SqlitePool, email: &str) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as("SELECT id, email, password_hash, role FROM users WHERE email = ?")
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn find_by_id(pool: &SqlitePool, id: &str) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as("SELECT id, email, password_hash, role FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn create_then_find_by_email_ignores_case() {
        let (pool, _dir) = pool().await;
        let created = create(&pool, "Ada@Example.com", "hash", "admin")
            .await
            .unwrap();
        let found = find_by_email(&pool, "ada@example.COM")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, created.id);
        assert_eq!(found.role, "admin");
    }

    #[tokio::test]
    async fn duplicate_email_is_rejected() {
        let (pool, _dir) = pool().await;
        create(&pool, "a@b.c", "hash", "admin").await.unwrap();
        assert!(create(&pool, "A@B.C", "hash", "viewer").await.is_err());
    }

    #[tokio::test]
    async fn unknown_role_is_rejected() {
        let (pool, _dir) = pool().await;
        assert!(create(&pool, "a@b.c", "hash", "root").await.is_err());
    }

    #[tokio::test]
    async fn count_reflects_inserts() {
        let (pool, _dir) = pool().await;
        assert_eq!(count(&pool).await.unwrap(), 0);
        create(&pool, "a@b.c", "hash", "admin").await.unwrap();
        assert_eq!(count(&pool).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn find_by_id_returns_none_when_unknown() {
        let (pool, _dir) = pool().await;
        assert!(find_by_id(&pool, "nope").await.unwrap().is_none());
    }
}
