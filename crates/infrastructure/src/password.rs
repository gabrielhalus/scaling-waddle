use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

pub fn hash(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

pub fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_succeeds() {
        let hash = hash("correct horse battery").unwrap();
        assert!(verify("correct horse battery", &hash));
    }

    #[test]
    fn wrong_password_fails() {
        let hash = hash("correct horse battery").unwrap();
        assert!(!verify("wrong", &hash));
    }

    #[test]
    fn two_hashes_of_same_password_differ() {
        assert_ne!(hash("same").unwrap(), hash("same").unwrap());
    }

    #[test]
    fn malformed_hash_never_verifies() {
        assert!(!verify("anything", "not-a-phc-string"));
    }
}
