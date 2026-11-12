use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenKind {
    Access,
    Refresh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub kind: TokenKind,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
}

#[derive(Clone)]
pub struct JwtKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl JwtKeys {
    pub fn new(secret: &str) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
        }
    }

    pub fn issue(
        &self,
        sub: &str,
        role: &str,
        kind: TokenKind,
        ttl_secs: i64,
    ) -> Result<String, jsonwebtoken::errors::Error> {
        let now = now_secs();
        let claims = Claims {
            sub: sub.to_owned(),
            role: role.to_owned(),
            kind,
            iat: now,
            exp: now + ttl_secs,
            jti: uuid::Uuid::new_v4().to_string(),
        };
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
    }

    pub fn verify(&self, token: &str, kind: TokenKind) -> Option<Claims> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = 0;
        let claims = decode::<Claims>(token, &self.decoding, &validation)
            .ok()?
            .claims;
        (claims.kind == kind).then_some(claims)
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> JwtKeys {
        JwtKeys::new("un-secret-de-test-assez-long-pour-hs256")
    }

    #[test]
    fn issued_token_verifies_with_same_kind() {
        let token = keys().issue("u1", "admin", TokenKind::Access, 900).unwrap();
        let claims = keys().verify(&token, TokenKind::Access).unwrap();
        assert_eq!(claims.sub, "u1");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn access_token_is_rejected_as_refresh() {
        let token = keys().issue("u1", "admin", TokenKind::Access, 900).unwrap();
        assert!(keys().verify(&token, TokenKind::Refresh).is_none());
    }

    #[test]
    fn expired_token_is_rejected() {
        let token = keys()
            .issue("u1", "admin", TokenKind::Access, -120)
            .unwrap();
        assert!(keys().verify(&token, TokenKind::Access).is_none());
    }

    #[test]
    fn token_signed_with_another_secret_is_rejected() {
        let token = JwtKeys::new("autre-secret-de-test-assez-long-hs256")
            .issue("u1", "admin", TokenKind::Access, 900)
            .unwrap();
        assert!(keys().verify(&token, TokenKind::Access).is_none());
    }

    #[test]
    fn two_tokens_in_same_second_differ() {
        let a = keys()
            .issue("u1", "admin", TokenKind::Refresh, 900)
            .unwrap();
        let b = keys()
            .issue("u1", "admin", TokenKind::Refresh, 900)
            .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(keys().verify("pas.un.jwt", TokenKind::Access).is_none());
    }
}
