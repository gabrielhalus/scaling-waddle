use std::collections::BTreeMap;

use sqlx::{FromRow, SqlitePool};

use crate::crypto::{CryptoError, Keyring, Sealed};

const MAX_VALUE_BYTES: usize = 8192;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("nom de secret invalide (1 à 64 caractères parmi a-z A-Z 0-9 _ . -)")]
    InvalidName,
    #[error("la valeur doit contenir de 1 à {MAX_VALUE_BYTES} octets")]
    InvalidValue,
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct SecretMeta {
    pub project: String,
    pub name: String,
    pub key_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(FromRow)]
struct Row {
    project: String,
    name: String,
    key_id: String,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

impl Row {
    fn open(self, ring: &Keyring) -> Result<(String, String), SecretError> {
        let aad = aad(&self.project, &self.name);
        let plain = ring.open(
            &Sealed {
                key_id: self.key_id,
                nonce: self.nonce,
                ciphertext: self.ciphertext,
            },
            &aad,
        )?;
        let value = String::from_utf8(plain).map_err(|_| CryptoError::Decrypt)?;
        Ok((self.name, value))
    }
}

pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn aad(project: &str, name: &str) -> Vec<u8> {
    format!("{project}/{name}").into_bytes()
}

pub async fn set(
    pool: &SqlitePool,
    ring: &Keyring,
    project: &str,
    name: &str,
    value: &str,
) -> Result<(), SecretError> {
    if !valid_name(name) {
        return Err(SecretError::InvalidName);
    }
    if value.is_empty() || value.len() > MAX_VALUE_BYTES {
        return Err(SecretError::InvalidValue);
    }
    let sealed = ring.seal(value.as_bytes(), &aad(project, name))?;
    sqlx::query(
        "INSERT INTO secrets (id, project, name, key_id, nonce, ciphertext) VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT (project, name) DO UPDATE SET key_id = excluded.key_id, \
             nonce = excluded.nonce, ciphertext = excluded.ciphertext, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(project)
    .bind(name)
    .bind(&sealed.key_id)
    .bind(&sealed.nonce)
    .bind(&sealed.ciphertext)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list(
    pool: &SqlitePool,
    project: Option<&str>,
) -> Result<Vec<SecretMeta>, sqlx::Error> {
    sqlx::query_as(
        "SELECT project, name, key_id, created_at, updated_at FROM secrets \
         WHERE (?1 IS NULL OR project = ?1) ORDER BY project, name",
    )
    .bind(project)
    .fetch_all(pool)
    .await
}

pub async fn delete(pool: &SqlitePool, project: &str, name: &str) -> Result<bool, sqlx::Error> {
    let deleted = sqlx::query("DELETE FROM secrets WHERE project = ? AND name = ?")
        .bind(project)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(deleted.rows_affected() == 1)
}

pub async fn for_project(
    pool: &SqlitePool,
    ring: &Keyring,
    project: &str,
) -> Result<BTreeMap<String, String>, SecretError> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT project, name, key_id, nonce, ciphertext FROM secrets WHERE project = ?",
    )
    .bind(project)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(|row| row.open(ring)).collect()
}

// Rechiffre avec la clé active tout secret chiffré par une autre clé.
pub async fn rotate(pool: &SqlitePool, ring: &Keyring) -> Result<u64, SecretError> {
    let active = ring.active_id().ok_or(CryptoError::NoKeys)?;
    let stale: Vec<Row> = sqlx::query_as(
        "SELECT project, name, key_id, nonce, ciphertext FROM secrets WHERE key_id <> ?",
    )
    .bind(active)
    .fetch_all(pool)
    .await?;
    let mut rotated = 0;
    for row in stale {
        let project = row.project.clone();
        let (name, value) = row.open(ring)?;
        set(pool, ring, &project, &name, &value).await?;
        rotated += 1;
    }
    Ok(rotated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(active: &str) -> Keyring {
        let keys = format!(
            "k1={},k2={}",
            hex::encode([1u8; 32]),
            hex::encode([2u8; 32])
        );
        Keyring::parse(&keys, Some(active)).unwrap()
    }

    async fn pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn a_stored_secret_is_encrypted_and_readable_back() {
        let (pool, _dir) = pool().await;
        set(&pool, &ring("k1"), "p", "token", "s3cret")
            .await
            .unwrap();
        let raw: Vec<u8> = sqlx::query_scalar("SELECT ciphertext FROM secrets")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("s3cret"));
        let all = for_project(&pool, &ring("k1"), "p").await.unwrap();
        assert_eq!(all.get("token").map(String::as_str), Some("s3cret"));
        assert!(
            for_project(&pool, &ring("k1"), "autre")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn setting_again_overwrites_the_value() {
        let (pool, _dir) = pool().await;
        set(&pool, &ring("k1"), "p", "token", "v1").await.unwrap();
        set(&pool, &ring("k1"), "p", "token", "v2").await.unwrap();
        let all = for_project(&pool, &ring("k1"), "p").await.unwrap();
        assert_eq!(all.get("token").map(String::as_str), Some("v2"));
        assert_eq!(list(&pool, None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_exposes_metadata_but_never_values() {
        let (pool, _dir) = pool().await;
        set(&pool, &ring("k1"), "p", "b", "x").await.unwrap();
        set(&pool, &ring("k1"), "p", "a", "y").await.unwrap();
        set(&pool, &ring("k1"), "q", "c", "z").await.unwrap();
        let names: Vec<_> = list(&pool, Some("p"))
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(list(&pool, None).await.unwrap()[0].key_id, "k1");
    }

    #[tokio::test]
    async fn names_and_values_are_validated() {
        let (pool, _dir) = pool().await;
        for name in ["", "a b", "é", &"x".repeat(65), "a/b"] {
            assert!(
                matches!(
                    set(&pool, &ring("k1"), "p", name, "v").await,
                    Err(SecretError::InvalidName)
                ),
                "{name}"
            );
        }
        assert!(matches!(
            set(&pool, &ring("k1"), "p", "n", "").await,
            Err(SecretError::InvalidValue)
        ));
        let big = "x".repeat(MAX_VALUE_BYTES + 1);
        assert!(matches!(
            set(&pool, &ring("k1"), "p", "n", &big).await,
            Err(SecretError::InvalidValue)
        ));
    }

    #[tokio::test]
    async fn setting_needs_an_enabled_keyring() {
        let (pool, _dir) = pool().await;
        let result = set(&pool, &Keyring::empty(), "p", "n", "v").await;
        assert!(matches!(
            result,
            Err(SecretError::Crypto(CryptoError::NoKeys))
        ));
    }

    #[tokio::test]
    async fn delete_removes_one_secret() {
        let (pool, _dir) = pool().await;
        set(&pool, &ring("k1"), "p", "n", "v").await.unwrap();
        assert!(delete(&pool, "p", "n").await.unwrap());
        assert!(!delete(&pool, "p", "n").await.unwrap());
    }

    #[tokio::test]
    async fn rotate_reencrypts_with_the_active_key() {
        let (pool, _dir) = pool().await;
        set(&pool, &ring("k1"), "p", "a", "1").await.unwrap();
        set(&pool, &ring("k1"), "p", "b", "2").await.unwrap();
        assert_eq!(rotate(&pool, &ring("k1")).await.unwrap(), 0);
        assert_eq!(rotate(&pool, &ring("k2")).await.unwrap(), 2);
        assert!(
            list(&pool, None)
                .await
                .unwrap()
                .iter()
                .all(|m| m.key_id == "k2")
        );
        let only_k2 =
            Keyring::parse(&format!("k2={}", hex::encode([2u8; 32])), Some("k2")).unwrap();
        let all = for_project(&pool, &only_k2, "p").await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn a_ciphertext_moved_to_another_name_does_not_decrypt() {
        let (pool, _dir) = pool().await;
        set(&pool, &ring("k1"), "p", "a", "1").await.unwrap();
        sqlx::query("UPDATE secrets SET name = 'b'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(for_project(&pool, &ring("k1"), "p").await.is_err());
    }
}
