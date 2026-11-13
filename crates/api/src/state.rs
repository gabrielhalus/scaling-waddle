use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use sw_infrastructure::crypto::Keyring;
use sw_infrastructure::jwt::JwtKeys;

use crate::ratelimit::FailureLimiter;

pub const ACCESS_TTL_SECS: i64 = 15 * 60;
pub const REFRESH_TTL_SECS: i64 = 30 * 24 * 60 * 60;
const LOGIN_MAX_FAILURES: usize = 5;
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
const WEBHOOK_MAX_PER_MINUTE: usize = 120;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub jwt: JwtKeys,
    pub login_limiter: Arc<FailureLimiter>,
    pub webhook_limiter: Arc<FailureLimiter>,
    pub keyring: Keyring,
}

impl AppState {
    pub fn new(db: SqlitePool, jwt_secret: &str) -> Self {
        Self {
            db,
            jwt: JwtKeys::new(jwt_secret),
            login_limiter: Arc::new(FailureLimiter::new(LOGIN_MAX_FAILURES, LOGIN_WINDOW)),
            webhook_limiter: Arc::new(FailureLimiter::new(
                WEBHOOK_MAX_PER_MINUTE,
                Duration::from_secs(60),
            )),
            keyring: Keyring::empty(),
        }
    }

    pub fn with_keyring(mut self, keyring: Keyring) -> Self {
        self.keyring = keyring;
        self
    }
}
