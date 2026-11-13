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

pub async fn create_first_admin(
    pool: &SqlitePool,
    email: &str,
    password_hash: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO users (id, email, password_hash, role) \
         SELECT ?, ?, ?, 'admin' WHERE NOT EXISTS (SELECT 1 FROM users) \
         RETURNING id, email, password_hash, role",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(email)
    .bind(password_hash)
    .fetch_optional(pool)
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

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("utilisateur introuvable")]
    NotFound,
    #[error("au moins un administrateur doit rester")]
    LastAdmin,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct UserSummary {
    pub id: String,
    pub email: String,
    pub role: String,
    pub created_at: String,
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<UserSummary>, sqlx::Error> {
    sqlx::query_as("SELECT id, email, role, created_at FROM users ORDER BY created_at, rowid")
        .fetch_all(pool)
        .await
}

async fn missing_or_last_admin(pool: &SqlitePool, id: &str) -> Result<UserError, sqlx::Error> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(if exists.is_some() {
        UserError::LastAdmin
    } else {
        UserError::NotFound
    })
}

const OTHER_ADMIN_REMAINS: &str = "(SELECT count(*) FROM users WHERE role = 'admin') > 1";

pub async fn set_role(pool: &SqlitePool, id: &str, role: &str) -> Result<(), UserError> {
    let updated = sqlx::query(&format!(
        "UPDATE users SET role = ?1 WHERE id = ?2 AND (?1 = 'admin' OR role <> 'admin' OR {OTHER_ADMIN_REMAINS})"
    ))
    .bind(role)
    .bind(id)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 1 {
        return Ok(());
    }
    Err(missing_or_last_admin(pool, id).await?)
}

pub async fn set_password_hash(
    pool: &SqlitePool,
    id: &str,
    password_hash: &str,
) -> Result<(), UserError> {
    let updated = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(id)
        .execute(pool)
        .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(UserError::NotFound)
    }
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<(), UserError> {
    let deleted = sqlx::query(&format!(
        "DELETE FROM users WHERE id = ? AND (role <> 'admin' OR {OTHER_ADMIN_REMAINS})"
    ))
    .bind(id)
    .execute(pool)
    .await?;
    if deleted.rows_affected() == 1 {
        return Ok(());
    }
    Err(missing_or_last_admin(pool, id).await?)
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
    async fn create_first_admin_only_works_on_an_empty_table() {
        let (pool, _dir) = pool().await;
        let first = create_first_admin(&pool, "a@b.c", "hash").await.unwrap();
        assert_eq!(first.unwrap().role, "admin");
        assert!(
            create_first_admin(&pool, "d@e.f", "hash")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(count(&pool).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn find_by_id_returns_none_when_unknown() {
        let (pool, _dir) = pool().await;
        assert!(find_by_id(&pool, "nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_returns_users_in_creation_order_without_hashes() {
        let (pool, _dir) = pool().await;
        create(&pool, "a@b.c", "h1", "admin").await.unwrap();
        create(&pool, "b@b.c", "h2", "viewer").await.unwrap();
        let listed = list(&pool).await.unwrap();
        assert_eq!(
            listed.iter().map(|u| u.email.as_str()).collect::<Vec<_>>(),
            ["a@b.c", "b@b.c"]
        );
        assert_eq!(listed[1].role, "viewer");
    }

    #[tokio::test]
    async fn the_last_admin_cannot_be_demoted_or_deleted() {
        let (pool, _dir) = pool().await;
        let admin = create(&pool, "a@b.c", "h", "admin").await.unwrap();
        assert!(matches!(
            set_role(&pool, &admin.id, "viewer").await,
            Err(UserError::LastAdmin)
        ));
        assert!(matches!(
            delete(&pool, &admin.id).await,
            Err(UserError::LastAdmin)
        ));
        assert_eq!(
            find_by_id(&pool, &admin.id).await.unwrap().unwrap().role,
            "admin"
        );
    }

    #[tokio::test]
    async fn an_admin_can_be_demoted_or_deleted_when_another_remains() {
        let (pool, _dir) = pool().await;
        let first = create(&pool, "a@b.c", "h", "admin").await.unwrap();
        let second = create(&pool, "b@b.c", "h", "admin").await.unwrap();
        set_role(&pool, &first.id, "operator").await.unwrap();
        assert_eq!(
            find_by_id(&pool, &first.id).await.unwrap().unwrap().role,
            "operator"
        );
        assert!(matches!(
            delete(&pool, &second.id).await,
            Err(UserError::LastAdmin)
        ));
        delete(&pool, &first.id).await.unwrap();
        assert!(find_by_id(&pool, &first.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn roles_are_validated_and_unknown_users_reported() {
        let (pool, _dir) = pool().await;
        let user = create(&pool, "a@b.c", "h", "admin").await.unwrap();
        create(&pool, "b@b.c", "h", "admin").await.unwrap();
        assert!(set_role(&pool, &user.id, "root").await.is_err());
        assert!(matches!(
            set_role(&pool, "nope", "viewer").await,
            Err(UserError::NotFound)
        ));
        assert!(matches!(
            delete(&pool, "nope").await,
            Err(UserError::NotFound)
        ));
        assert!(matches!(
            set_password_hash(&pool, "nope", "h").await,
            Err(UserError::NotFound)
        ));
    }

    #[tokio::test]
    async fn changing_a_password_and_deleting_cascade_to_sessions() {
        let (pool, _dir) = pool().await;
        let keep = create(&pool, "a@b.c", "h", "admin").await.unwrap();
        let user = create(&pool, "b@b.c", "old", "viewer").await.unwrap();
        set_password_hash(&pool, &user.id, "new").await.unwrap();
        assert_eq!(
            find_by_id(&pool, &user.id)
                .await
                .unwrap()
                .unwrap()
                .password_hash,
            "new"
        );
        crate::refresh_tokens::store(&pool, &user.id, "tok", i64::MAX)
            .await
            .unwrap();
        delete(&pool, &user.id).await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM refresh_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
        assert!(find_by_id(&pool, &keep.id).await.unwrap().is_some());
    }
}
