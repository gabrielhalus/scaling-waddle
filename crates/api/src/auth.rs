use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sw_infrastructure::jwt::TokenKind;
use sw_infrastructure::{password, refresh_tokens, users};

use crate::AppState;
use crate::error::ApiError;
use crate::state::{ACCESS_TTL_SECS, REFRESH_TTL_SECS};

const MIN_PASSWORD_LEN: usize = 12;
const MAX_PASSWORD_LEN: usize = 256;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/setup", get(setup_status).post(setup))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(Serialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

#[derive(Serialize)]
struct Me {
    id: String,
    email: String,
    role: String,
}

pub struct AuthUser {
    pub id: String,
    pub role: String,
}

impl AuthUser {
    pub fn require_writer(&self) -> Result<(), ApiError> {
        match self.role.as_str() {
            "admin" | "operator" => Ok(()),
            _ => Err(ApiError::Forbidden),
        }
    }
}

impl AuthUser {
    pub fn require_admin(&self) -> Result<(), ApiError> {
        if self.role == "admin" {
            Ok(())
        } else {
            Err(ApiError::Forbidden)
        }
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;
        let claims = state
            .jwt
            .verify(token, TokenKind::Access)
            .ok_or(ApiError::Unauthorized)?;
        Ok(Self {
            id: claims.sub,
            role: claims.role,
        })
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn validate(credentials: &Credentials) -> Result<String, ApiError> {
    let email = credentials.email.trim().to_lowercase();
    let valid_email = email.len() <= 254
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if !valid_email {
        return Err(ApiError::BadRequest("adresse e-mail invalide".into()));
    }
    let len = credentials.password.chars().count();
    if !(MIN_PASSWORD_LEN..=MAX_PASSWORD_LEN).contains(&len) {
        return Err(ApiError::BadRequest(
            "le mot de passe doit faire entre 12 et 256 caractères".into(),
        ));
    }
    Ok(email)
}

async fn issue(state: &AppState, user: &users::User) -> Result<Tokens, ApiError> {
    let access_token = state
        .jwt
        .issue(&user.id, &user.role, TokenKind::Access, ACCESS_TTL_SECS)
        .map_err(|_| ApiError::Internal)?;
    let refresh_token = state
        .jwt
        .issue(&user.id, &user.role, TokenKind::Refresh, REFRESH_TTL_SECS)
        .map_err(|_| ApiError::Internal)?;
    refresh_tokens::store(
        &state.db,
        &user.id,
        &refresh_token,
        now() + REFRESH_TTL_SECS,
    )
    .await?;
    Ok(Tokens {
        access_token,
        refresh_token,
        expires_in: ACCESS_TTL_SECS,
    })
}

async fn setup_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let required = users::count(&state.db).await? == 0;
    Ok(Json(serde_json::json!({ "required": required })))
}

async fn setup(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<(StatusCode, Json<Tokens>), ApiError> {
    let email = validate(&credentials)?;
    let hash = hash_blocking(credentials.password).await?;
    let user = users::create_first_admin(&state.db, &email, &hash)
        .await?
        .ok_or(ApiError::Conflict("l'installation est déjà faite"))?;
    Ok((StatusCode::CREATED, Json(issue(&state, &user).await?)))
}

async fn login(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<Json<Tokens>, ApiError> {
    let email = credentials.email.trim().to_lowercase();
    if let Some(wait) = state.login_limiter.retry_after(&email, Instant::now()) {
        return Err(ApiError::TooManyRequests {
            retry_after_secs: wait.as_secs().max(1),
        });
    }
    let user = users::find_by_email(&state.db, &email).await?;
    let stored = match &user {
        Some(user) => user.password_hash.clone(),
        None => decoy_hash_blocking().await?,
    };
    let ok = verify_blocking(credentials.password, stored).await?;
    match user {
        Some(user) if ok => {
            state.login_limiter.clear(&email);
            Ok(Json(issue(&state, &user).await?))
        }
        _ => {
            state.login_limiter.record_failure(&email, Instant::now());
            Err(ApiError::Unauthorized)
        }
    }
}

async fn refresh(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<Tokens>, ApiError> {
    state
        .jwt
        .verify(&request.refresh_token, TokenKind::Refresh)
        .ok_or(ApiError::Unauthorized)?;
    let user_id = refresh_tokens::consume(&state.db, &request.refresh_token, now())
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let user = users::find_by_id(&state.db, &user_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    Ok(Json(issue(&state, &user).await?))
}

async fn logout(State(state): State<AppState>, auth: AuthUser) -> Result<StatusCode, ApiError> {
    refresh_tokens::revoke_all(&state.db, &auth.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(State(state): State<AppState>, auth: AuthUser) -> Result<Json<Me>, ApiError> {
    let user = users::find_by_id(&state.db, &auth.id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    Ok(Json(Me {
        id: user.id,
        email: user.email,
        role: user.role,
    }))
}

async fn hash_blocking(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || password::hash(&password))
        .await
        .map_err(|_| ApiError::Internal)?
        .map_err(|_| ApiError::Internal)
}

async fn decoy_hash_blocking() -> Result<String, ApiError> {
    tokio::task::spawn_blocking(|| decoy_hash().to_owned())
        .await
        .map_err(|_| ApiError::Internal)
}

async fn verify_blocking(password: String, hash: String) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || password::verify(&password, &hash))
        .await
        .map_err(|_| ApiError::Internal)
}

fn decoy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| password::hash("mot de passe leurre").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::{AppState, router};

    const PASSWORD: &str = "correct horse battery";

    async fn app() -> (axum::Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = sw_infrastructure::db::open(&dir.path().join("sw.db"))
            .await
            .unwrap();
        let state = AppState::new(db, "0123456789abcdef0123456789abcdef");
        (router(state), dir)
    }

    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        bearer: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let req = match body {
            Some(json) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    async fn setup(app: &axum::Router) -> Value {
        let (status, body) = call(
            app,
            "POST",
            "/api/v1/auth/setup",
            None,
            Some(json!({ "email": "ada@example.com", "password": PASSWORD })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        body
    }

    #[tokio::test]
    async fn setup_status_flips_after_first_admin() {
        let (app, _dir) = app().await;
        let (_, before) = call(&app, "GET", "/api/v1/auth/setup", None, None).await;
        assert_eq!(before["required"], true);
        setup(&app).await;
        let (_, after) = call(&app, "GET", "/api/v1/auth/setup", None, None).await;
        assert_eq!(after["required"], false);
    }

    #[tokio::test]
    async fn setup_is_refused_once_a_user_exists() {
        let (app, _dir) = app().await;
        setup(&app).await;
        let (status, _) = call(
            &app,
            "POST",
            "/api/v1/auth/setup",
            None,
            Some(json!({ "email": "eve@example.com", "password": PASSWORD })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn setup_rejects_short_password_and_bad_email() {
        let (app, _dir) = app().await;
        for body in [
            json!({ "email": "ada@example.com", "password": "court" }),
            json!({ "email": "pas-un-email", "password": PASSWORD }),
        ] {
            let (status, _) = call(&app, "POST", "/api/v1/auth/setup", None, Some(body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn login_then_me_returns_the_admin() {
        let (app, _dir) = app().await;
        setup(&app).await;
        let (status, tokens) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "email": "ADA@example.com", "password": PASSWORD })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let access = tokens["access_token"].as_str().unwrap();
        let (status, me) = call(&app, "GET", "/api/v1/auth/me", Some(access), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["email"], "ada@example.com");
        assert_eq!(me["role"], "admin");
    }

    #[tokio::test]
    async fn wrong_password_and_unknown_email_get_the_same_401() {
        let (app, _dir) = app().await;
        setup(&app).await;
        let mut answers = Vec::new();
        for email in ["ada@example.com", "inconnu@example.com"] {
            answers.push(
                call(
                    &app,
                    "POST",
                    "/api/v1/auth/login",
                    None,
                    Some(json!({ "email": email, "password": "mauvais mot de passe" })),
                )
                .await,
            );
        }
        assert_eq!(answers[0].0, StatusCode::UNAUTHORIZED);
        assert_eq!(answers[0], answers[1]);
    }

    #[tokio::test]
    async fn me_requires_a_valid_access_token() {
        let (app, _dir) = app().await;
        let tokens = setup(&app).await;
        let (status, _) = call(&app, "GET", "/api/v1/auth/me", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(&app, "GET", "/api/v1/auth/me", Some("garbage"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let refresh = tokens["refresh_token"].as_str().unwrap();
        let (status, _) = call(&app, "GET", "/api/v1/auth/me", Some(refresh), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn refresh_rotates_and_replay_is_rejected() {
        let (app, _dir) = app().await;
        let tokens = setup(&app).await;
        let old = tokens["refresh_token"].as_str().unwrap();
        let (status, fresh) = call(
            &app,
            "POST",
            "/api/v1/auth/refresh",
            None,
            Some(json!({ "refresh_token": old })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(fresh["refresh_token"], tokens["refresh_token"]);
        let (status, _) = call(
            &app,
            "POST",
            "/api/v1/auth/refresh",
            None,
            Some(json!({ "refresh_token": old })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn access_token_cannot_be_used_to_refresh() {
        let (app, _dir) = app().await;
        let tokens = setup(&app).await;
        let (status, _) = call(
            &app,
            "POST",
            "/api/v1/auth/refresh",
            None,
            Some(json!({ "refresh_token": tokens["access_token"] })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn logout_revokes_every_refresh_token() {
        let (app, _dir) = app().await;
        let tokens = setup(&app).await;
        let access = tokens["access_token"].as_str().unwrap();
        let (status, _) = call(&app, "POST", "/api/v1/auth/logout", Some(access), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = call(
            &app,
            "POST",
            "/api/v1/auth/refresh",
            None,
            Some(json!({ "refresh_token": tokens["refresh_token"] })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_locks_out_after_repeated_failures() {
        let (app, _dir) = app().await;
        setup(&app).await;
        let attempt = |password: &'static str| {
            call(
                &app,
                "POST",
                "/api/v1/auth/login",
                None,
                Some(json!({ "email": "ada@example.com", "password": password })),
            )
        };
        for _ in 0..5 {
            assert_eq!(attempt("mauvais").await.0, StatusCode::UNAUTHORIZED);
        }
        assert_eq!(attempt("mauvais").await.0, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(attempt(PASSWORD).await.0, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn lockout_is_per_email_and_a_success_resets_the_count() {
        let (app, _dir) = app().await;
        setup(&app).await;
        let attempt = |email: &'static str, password: &'static str| {
            call(
                &app,
                "POST",
                "/api/v1/auth/login",
                None,
                Some(json!({ "email": email, "password": password })),
            )
        };
        for _ in 0..4 {
            attempt("ada@example.com", "mauvais").await;
        }
        assert_eq!(attempt("ada@example.com", PASSWORD).await.0, StatusCode::OK);
        for _ in 0..4 {
            attempt("ada@example.com", "mauvais").await;
        }
        assert_eq!(attempt("ada@example.com", PASSWORD).await.0, StatusCode::OK);
        for _ in 0..6 {
            attempt("inconnu@example.com", "x").await;
        }
        assert_eq!(attempt("ada@example.com", PASSWORD).await.0, StatusCode::OK);
    }
}
