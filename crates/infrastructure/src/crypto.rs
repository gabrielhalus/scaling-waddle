use std::collections::BTreeMap;

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use rand::RngCore;

const NONCE_LEN: usize = 12;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("aucune clé de chiffrement configurée")]
    NoKeys,
    #[error("clé inconnue: {0}")]
    UnknownKey(String),
    #[error("configuration des clés invalide: {0}")]
    Config(String),
    #[error("déchiffrement impossible")]
    Decrypt,
}

#[derive(Clone)]
pub struct Keyring {
    keys: BTreeMap<String, [u8; 32]>,
    active: Option<String>,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyring")
            .field("key_ids", &self.keys.keys().collect::<Vec<_>>())
            .field("active", &self.active)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub key_id: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

impl Keyring {
    pub fn empty() -> Self {
        Self {
            keys: BTreeMap::new(),
            active: None,
        }
    }

    // `keys` : `id=hex64,id2=hex64` ; `active` : l'id utilisé pour chiffrer.
    pub fn parse(keys: &str, active: Option<&str>) -> Result<Self, CryptoError> {
        let mut parsed = BTreeMap::new();
        for entry in keys.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (id, hex_key) = entry
                .split_once('=')
                .ok_or_else(|| CryptoError::Config("format attendu id=hex".into()))?;
            let valid_id = !id.is_empty()
                && id.len() <= 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if !valid_id {
                return Err(CryptoError::Config(format!(
                    "identifiant de clé invalide: {id}"
                )));
            }
            let bytes: [u8; 32] = hex::decode(hex_key)
                .ok()
                .and_then(|raw| raw.try_into().ok())
                .ok_or_else(|| {
                    CryptoError::Config(format!("la clé {id} doit faire 32 octets en hex"))
                })?;
            if parsed.insert(id.to_owned(), bytes).is_some() {
                return Err(CryptoError::Config(format!("clé en double: {id}")));
            }
        }
        let active = match (parsed.is_empty(), active) {
            (true, None) => None,
            (true, Some(_)) => return Err(CryptoError::Config("clé active sans clés".into())),
            (false, Some(id)) if parsed.contains_key(id) => Some(id.to_owned()),
            (false, Some(id)) => return Err(CryptoError::UnknownKey(id.to_owned())),
            (false, None) => {
                return Err(CryptoError::Config("clé active requise".into()));
            }
        };
        Ok(Self {
            keys: parsed,
            active,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.active.is_some()
    }

    pub fn active_id(&self) -> Option<&str> {
        self.active.as_deref()
    }

    fn cipher(&self, key_id: &str) -> Result<Aes256Gcm, CryptoError> {
        let key = self
            .keys
            .get(key_id)
            .ok_or_else(|| CryptoError::UnknownKey(key_id.to_owned()))?;
        Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::Decrypt)
    }

    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<Sealed, CryptoError> {
        let key_id = self.active.clone().ok_or(CryptoError::NoKeys)?;
        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher(&key_id)?
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Decrypt)?;
        Ok(Sealed {
            key_id,
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    pub fn open(&self, sealed: &Sealed, aad: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if sealed.nonce.len() != NONCE_LEN {
            return Err(CryptoError::Decrypt);
        }
        self.cipher(&sealed.key_id)?
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> String {
        hex::encode([byte; 32])
    }

    fn ring() -> Keyring {
        Keyring::parse(&format!("k1={},k2={}", key(1), key(2)), Some("k1")).unwrap()
    }

    #[test]
    fn seal_then_open_roundtrips() {
        let sealed = ring().seal(b"hunter2", b"p/db").unwrap();
        assert_eq!(sealed.key_id, "k1");
        assert_ne!(sealed.ciphertext, b"hunter2");
        assert_eq!(ring().open(&sealed, b"p/db").unwrap(), b"hunter2");
    }

    #[test]
    fn each_seal_uses_a_fresh_nonce() {
        let a = ring().seal(b"x", b"").unwrap();
        let b = ring().seal(b"x", b"").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn a_different_aad_or_tampered_data_is_rejected() {
        let sealed = ring().seal(b"x", b"p/db").unwrap();
        assert_eq!(ring().open(&sealed, b"p/autre"), Err(CryptoError::Decrypt));
        let mut tampered = sealed.clone();
        tampered.ciphertext[0] ^= 1;
        assert_eq!(ring().open(&tampered, b"p/db"), Err(CryptoError::Decrypt));
    }

    #[test]
    fn old_keys_still_open_after_the_active_key_changes() {
        let sealed = ring().seal(b"x", b"").unwrap();
        let rotated = Keyring::parse(&format!("k1={},k2={}", key(1), key(2)), Some("k2")).unwrap();
        assert_eq!(rotated.open(&sealed, b"").unwrap(), b"x");
        assert_eq!(rotated.seal(b"x", b"").unwrap().key_id, "k2");
    }

    #[test]
    fn a_removed_key_cannot_open_its_data() {
        let sealed = ring().seal(b"x", b"").unwrap();
        let only_k2 = Keyring::parse(&format!("k2={}", key(2)), Some("k2")).unwrap();
        assert_eq!(
            only_k2.open(&sealed, b""),
            Err(CryptoError::UnknownKey("k1".into()))
        );
    }

    #[test]
    fn an_empty_keyring_is_disabled() {
        let empty = Keyring::parse("", None).unwrap();
        assert!(!empty.is_enabled());
        assert_eq!(empty.seal(b"x", b""), Err(CryptoError::NoKeys));
        assert!(!Keyring::empty().is_enabled());
    }

    #[test]
    fn invalid_configurations_are_refused() {
        for (keys, active) in [
            ("k1=zz".to_owned(), Some("k1")),
            (format!("k1={}", "ab".repeat(16)), Some("k1")),
            (format!("k1={}", key(1)), None),
            (format!("k1={}", key(1)), Some("k9")),
            (format!("k1={},k1={}", key(1), key(2)), Some("k1")),
            (format!("bad id={}", key(1)), Some("bad id")),
            (key(1), Some("k1")),
            (String::new(), Some("k1")),
        ] {
            assert!(Keyring::parse(&keys, active).is_err(), "{keys} {active:?}");
        }
    }

    #[test]
    fn debug_never_prints_key_material() {
        let text = format!("{:?}", ring());
        assert!(!text.contains(&key(1)));
        assert!(text.contains("k1"));
    }
}
