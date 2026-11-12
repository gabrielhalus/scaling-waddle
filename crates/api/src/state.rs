use sqlx::SqlitePool;
use sw_infrastructure::jwt::JwtKeys;

pub const ACCESS_TTL_SECS: i64 = 15 * 60;
pub const REFRESH_TTL_SECS: i64 = 30 * 24 * 60 * 60;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub jwt: JwtKeys,
}

impl AppState {
    pub fn new(db: SqlitePool, jwt_secret: &str) -> Self {
        Self {
            db,
            jwt: JwtKeys::new(jwt_secret),
        }
    }
}
