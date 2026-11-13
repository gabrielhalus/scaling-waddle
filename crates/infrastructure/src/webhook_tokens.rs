use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};

use crate::crypto::{CryptoError, Keyring, Sealed};

#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

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

const ENCRYPTED_PREFIX: &str = "enc:";

fn aad(token_id: &str) -> Vec<u8> {
    format!("webhook-token/{token_id}").into_bytes()
}

fn encode_secret(ring: &Keyring, token_id: &str, secret: &str) -> Result<String, CryptoError> {
    if !ring.is_enabled() {
        return Ok(secret.to_owned());
    }
    let sealed = ring.seal(secret.as_bytes(), &aad(token_id))?;
    Ok(format!(
        "{ENCRYPTED_PREFIX}{}:{}:{}",
        sealed.key_id,
        hex::encode(&sealed.nonce),
        hex::encode(&sealed.ciphertext)
    ))
}

fn decode_secret(ring: &Keyring, token_id: &str, stored: &str) -> Result<String, CryptoError> {
    let Some(body) = stored.strip_prefix(ENCRYPTED_PREFIX) else {
        return Ok(stored.to_owned());
    };
    let mut parts = body.splitn(3, ':');
    let (Some(key_id), Some(nonce), Some(ciphertext)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(CryptoError::Decrypt);
    };
    let sealed = Sealed {
        key_id: key_id.to_owned(),
        nonce: hex::decode(nonce).map_err(|_| CryptoError::Decrypt)?,
        ciphertext: hex::decode(ciphertext).map_err(|_| CryptoError::Decrypt)?,
    };
    let plain = ring.open(&sealed, &aad(token_id))?;
    String::from_utf8(plain).map_err(|_| CryptoError::Decrypt)
}

pub async fn issue(
    pool: &SqlitePool,
    ring: &Keyring,
    workflow_id: &str,
    signed: bool,
) -> Result<Issued, WebhookError> {
    let id = uuid::Uuid::new_v4().to_string();
    let token = generate("swh");
    let signing_secret = signed.then(|| generate("whsec"));
    let stored = signing_secret
        .as_deref()
        .map(|secret| encode_secret(ring, &id, secret))
        .transpose()?;
    sqlx::query(
        "INSERT INTO webhook_tokens (id, workflow_id, token_hash, signing_secret) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(workflow_id)
    .bind(hash(&token))
    .bind(&stored)
    .execute(pool)
    .await?;
    Ok(Issued {
        id,
        token,
        signing_secret,
    })
}

pub async fn resolve(
    pool: &SqlitePool,
    ring: &Keyring,
    token: &str,
) -> Result<Option<Resolved>, WebhookError> {
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, workflow_id, signing_secret FROM webhook_tokens WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(hash(token))
    .fetch_optional(pool)
    .await?;
    row.map(|(id, workflow_id, stored)| {
        let signing_secret = stored
            .map(|stored| decode_secret(ring, &id, &stored))
            .transpose()?;
        Ok(Resolved {
            workflow_id,
            signing_secret,
        })
    })
    .transpose()
}

// Rechiffre avec la clé active les secrets de signature chiffrés par une autre clé, ou en clair.
pub async fn rotate(pool: &SqlitePool, ring: &Keyring) -> Result<u64, WebhookError> {
    let active = ring.active_id().ok_or(CryptoError::NoKeys)?;
    let current = format!("{ENCRYPTED_PREFIX}{active}:");
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, signing_secret FROM webhook_tokens WHERE signing_secret IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut rotated = 0;
    for (id, stored) in rows.into_iter().filter(|(_, s)| !s.starts_with(&current)) {
        let plain = decode_secret(ring, &id, &stored)?;
        let reencoded = encode_secret(ring, &id, &plain)?;
        sqlx::query("UPDATE webhook_tokens SET signing_secret = ? WHERE id = ?")
            .bind(reencoded)
            .bind(&id)
            .execute(pool)
            .await?;
        rotated += 1;
    }
    Ok(rotated)
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
        let issued = issue(&pool, &Keyring::empty(), &wf, false).await.unwrap();
        assert!(issued.token.len() >= 40);
        assert_eq!(
            resolve(&pool, &Keyring::empty(), &issued.token)
                .await
                .unwrap(),
            Some(Resolved {
                workflow_id: wf,
                signing_secret: None
            })
        );
    }

    #[tokio::test]
    async fn token_is_stored_hashed() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &Keyring::empty(), &wf, false).await.unwrap();
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
        assert_eq!(
            resolve(&pool, &Keyring::empty(), "inconnu").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn two_tokens_are_distinct() {
        let (pool, wf, _dir) = setup().await;
        let a = issue(&pool, &Keyring::empty(), &wf, false).await.unwrap();
        let b = issue(&pool, &Keyring::empty(), &wf, false).await.unwrap();
        assert_ne!(a.token, b.token);
        assert_ne!(a.id, b.id);
    }

    #[tokio::test]
    async fn revoked_token_no_longer_resolves() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &Keyring::empty(), &wf, false).await.unwrap();
        assert!(revoke(&pool, &wf, &issued.id).await.unwrap());
        assert_eq!(
            resolve(&pool, &Keyring::empty(), &issued.token)
                .await
                .unwrap(),
            None
        );
        assert!(!revoke(&pool, &wf, &issued.id).await.unwrap());
    }

    #[tokio::test]
    async fn revoke_is_scoped_to_the_workflow() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &Keyring::empty(), &wf, false).await.unwrap();
        assert!(!revoke(&pool, "autre", &issued.id).await.unwrap());
        assert_eq!(
            resolve(&pool, &Keyring::empty(), &issued.token)
                .await
                .unwrap(),
            Some(Resolved {
                workflow_id: wf,
                signing_secret: None
            })
        );
    }

    #[tokio::test]
    async fn list_shows_ids_without_secrets() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &Keyring::empty(), &wf, false).await.unwrap();
        let listed = list(&pool, &wf).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, issued.id);
        assert!(listed[0].revoked_at.is_none());
    }

    #[tokio::test]
    async fn signed_tokens_carry_a_secret_returned_on_resolve() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &Keyring::empty(), &wf, true).await.unwrap();
        let secret = issued.signing_secret.clone().unwrap();
        assert!(secret.starts_with("whsec_") && secret.len() >= 40);
        let resolved = resolve(&pool, &Keyring::empty(), &issued.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.signing_secret, Some(secret));
    }

    #[tokio::test]
    async fn list_flags_signed_tokens_without_exposing_the_secret() {
        let (pool, wf, _dir) = setup().await;
        issue(&pool, &Keyring::empty(), &wf, false).await.unwrap();
        issue(&pool, &Keyring::empty(), &wf, true).await.unwrap();
        let listed = list(&pool, &wf).await.unwrap();
        assert_eq!(
            listed.iter().map(|t| t.signed).collect::<Vec<_>>(),
            [false, true]
        );
    }

    fn ring(active: &str) -> Keyring {
        let keys = format!(
            "k1={},k2={}",
            hex::encode([1u8; 32]),
            hex::encode([2u8; 32])
        );
        Keyring::parse(&keys, Some(active)).unwrap()
    }

    #[tokio::test]
    async fn signing_secrets_are_encrypted_at_rest_when_keys_are_configured() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &ring("k1"), &wf, true).await.unwrap();
        let secret = issued.signing_secret.clone().unwrap();
        let stored: String = sqlx::query_scalar("SELECT signing_secret FROM webhook_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(stored.starts_with("enc:k1:") && !stored.contains(&secret));
        let resolved = resolve(&pool, &ring("k1"), &issued.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.signing_secret, Some(secret));
    }

    #[tokio::test]
    async fn a_secret_stored_in_clear_still_resolves_after_keys_are_added() {
        let (pool, wf, _dir) = setup().await;
        let issued = issue(&pool, &Keyring::empty(), &wf, true).await.unwrap();
        let resolved = resolve(&pool, &ring("k1"), &issued.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.signing_secret, issued.signing_secret);
    }

    #[tokio::test]
    async fn rotate_encrypts_clear_secrets_and_moves_old_keys() {
        let (pool, wf, _dir) = setup().await;
        let clear = issue(&pool, &Keyring::empty(), &wf, true).await.unwrap();
        let old = issue(&pool, &ring("k1"), &wf, true).await.unwrap();
        assert_eq!(rotate(&pool, &ring("k2")).await.unwrap(), 2);
        assert_eq!(rotate(&pool, &ring("k2")).await.unwrap(), 0);
        let only_k2 =
            Keyring::parse(&format!("k2={}", hex::encode([2u8; 32])), Some("k2")).unwrap();
        for issued in [clear, old] {
            let resolved = resolve(&pool, &only_k2, &issued.token)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(resolved.signing_secret, issued.signing_secret);
        }
    }

    #[tokio::test]
    async fn a_secret_bound_to_another_token_does_not_decrypt() {
        let (pool, wf, _dir) = setup().await;
        let a = issue(&pool, &ring("k1"), &wf, true).await.unwrap();
        let b = issue(&pool, &ring("k1"), &wf, true).await.unwrap();
        sqlx::query("UPDATE webhook_tokens SET signing_secret = (SELECT signing_secret FROM webhook_tokens WHERE id = ?1) WHERE id = ?2")
            .bind(&a.id)
            .bind(&b.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(resolve(&pool, &ring("k1"), &b.token).await.is_err());
    }
}
