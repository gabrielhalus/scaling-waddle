use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone)]
pub struct Issued {
    pub id: String,
    pub token: String,
    pub signing_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub workflow_id: String,
    pub signing_secret: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct TokenRecord {
    pub id: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub signed: bool,
}

pub fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn generate(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}

pub async fn issue(
    pool: &SqlitePool,
    workflow_id: &str,
    signed: bool,
) -> Result<Issued, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let token = generate("swh");
    let signing_secret = signed.then(|| generate("whsec"));
    sqlx::query(
        "INSERT INTO webhook_tokens (id, workflow_id, token_hash, signing_secret) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(workflow_id)
    .bind(hash(&token))
    .bind(&signing_secret)
    .execute(pool)
    .await?;
    Ok(Issued {
        id,
        token,
        signing_secret,
    })
}

pub async fn resolve(pool: &SqlitePool, token: &str) -> Result<Option<Resolved>, sqlx::Error> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT workflow_id, signing_secret FROM webhook_tokens WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(hash(token))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(workflow_id, signing_secret)| Resolved {
        workflow_id,
        signing_secret,
    }))
}

pub async fn revoke(pool: &SqlitePool, workflow_id: &str, id: &str) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE webhook_tokens SET revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE id = ? AND workflow_id = ? AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(workflow_id)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn list(pool: &SqlitePool, workflow_id: &str) -> Result<Vec<TokenRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, created_at, revoked_at, signing_secret IS NOT NULL AS signed FROM webhook_tokens \
         WHERE workflow_id = ? ORDER BY created_at, rowid",
    )
    .bind(workflow_id)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use sw_domain::workflow::Workflow;

    use super::*;
    use crate::workflows;

    async fn setup() -> (sqlx::SqlitePool, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        let workflow = Workflow::from_yaml(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {name: w, project: p}\n\
             spec:\n  trigger: {type: webhook}\n  steps:\n    - {id: a, type: delay}\n",
        )
        .unwrap();
        let record = workflows::create(&pool, &workflow).await.unwrap();
        (pool, record.id, dir)
    }

    #[tokio::test]
    async fn issued_token_resolves_to_its_workflow() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &wf, false).await.unwrap();
        assert!(issued.token.len() >= 40);
        assert_eq!(
            resolve(&pool, &issued.token).await.unwrap(),
            Some(Resolved {
                workflow_id: wf,
                signing_secret: None
            })
        );
    }

    #[tokio::test]
    async fn token_is_stored_hashed() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &wf, false).await.unwrap();
        let stored: String =
            sqlx::query_scalar("SELECT token_hash FROM webhook_tokens WHERE id = ?")
                .bind(&issued.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_ne!(stored, issued.token);
        assert_eq!(stored, hash(&issued.token));
    }

    #[tokio::test]
    async fn unknown_token_resolves_to_none() {
        let (pool, _wf, _dir) = setup().await;
        assert_eq!(resolve(&pool, "inconnu").await.unwrap(), None);
    }

    #[tokio::test]
    async fn two_tokens_are_distinct() {
        let (pool, wf, _dir) = setup().await;
        let a = issue(&pool, &wf, false).await.unwrap();
        let b = issue(&pool, &wf, false).await.unwrap();
        assert_ne!(a.token, b.token);
        assert_ne!(a.id, b.id);
    }

    #[tokio::test]
    async fn revoked_token_no_longer_resolves() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &wf, false).await.unwrap();
        assert!(revoke(&pool, &wf, &issued.id).await.unwrap());
        assert_eq!(resolve(&pool, &issued.token).await.unwrap(), None);
        assert!(!revoke(&pool, &wf, &issued.id).await.unwrap());
    }

    #[tokio::test]
    async fn revoke_is_scoped_to_the_workflow() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &wf, false).await.unwrap();
        assert!(!revoke(&pool, "autre", &issued.id).await.unwrap());
        assert_eq!(
            resolve(&pool, &issued.token).await.unwrap(),
            Some(Resolved {
                workflow_id: wf,
                signing_secret: None
            })
        );
    }

    #[tokio::test]
    async fn list_shows_ids_without_secrets() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &wf, false).await.unwrap();
        let listed = list(&pool, &wf).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, issued.id);
        assert!(listed[0].revoked_at.is_none());
    }

    #[tokio::test]
    async fn signed_tokens_carry_a_secret_returned_on_resolve() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &wf, true).await.unwrap();
        let secret = issued.signing_secret.clone().unwrap();
        assert!(secret.starts_with("whsec_") && secret.len() >= 40);
        let resolved = resolve(&pool, &issued.token).await.unwrap().unwrap();
        assert_eq!(resolved.signing_secret, Some(secret));
    }

    #[tokio::test]
    async fn list_flags_signed_tokens_without_exposing_the_secret() {
        let (pool, wf, _dir) = setup().await;
        issue(&pool, &wf, false).await.unwrap();
        issue(&pool, &wf, true).await.unwrap();
        let listed = list(&pool, &wf).await.unwrap();
        assert_eq!(
            listed.iter().map(|t| t.signed).collect::<Vec<_>>(),
            [false, true]
        );
    }
}
