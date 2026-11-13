use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sw_infrastructure::refresh_tokens;
use sw_infrastructure::users::{self, UserError, UserSummary};

use crate::AppState;
use crate::audit;
use crate::auth::{AuthUser, check_new_account, check_password, hash_blocking};
use crate::error::ApiError;

const ROLES: &[&str] = &["admin", "operator", "viewer"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/users", get(list).post(create))
        .route("/api/v1/users/{id}", patch(update).delete(remove))
}

impl From<UserError> for ApiError {
    fn from(err: UserError) -> Self {
        match err {
            UserError::NotFound => Self::NotFound,
            UserError::LastAdmin => Self::Conflict("au moins un administrateur doit rester"),
            UserError::Db(err) => err.into(),
        }
    }
}

fn user_json(user: &UserSummary) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "role": user.role,
        "created_at": user.created_at,
    })
}

fn check_role(role: &str) -> Result<(), ApiError> {
    if ROLES.contains(&role) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "role doit valoir admin, operator ou viewer".into(),
        ))
    }
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> Result<Json<Value>, ApiError> {
    auth.require_admin()?;
    let found = users::list(&state.db).await?;
    Ok(Json(Value::Array(found.iter().map(user_json).collect())))
}

#[derive(Deserialize)]
struct NewUser {
    email: String,
    password: String,
    role: String,
}

async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(input): Json<NewUser>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    auth.require_admin()?;
    check_role(&input.role)?;
    let email = check_new_account(&input.email, &input.password)?;
    if users::find_by_email(&state.db, &email).await?.is_some() {
        return Err(ApiError::Conflict("cette adresse e-mail est déjà utilisée"));
    }
    let hash = hash_blocking(input.password).await?;
    let created = users::create(&state.db, &email, &hash, &input.role).await?;
    audit::record(
        &state,
        &auth.id,
        "user.create",
        "user",
        &created.id,
        json!({ "email": created.email, "role": created.role }),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": created.id, "email": created.email, "role": created.role })),
    ))
}

#[derive(Deserialize)]
struct Update {
    role: Option<String>,
    password: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(input): Json<Update>,
) -> Result<StatusCode, ApiError> {
    auth.require_admin()?;
    if input.role.is_none() && input.password.is_none() {
        return Err(ApiError::BadRequest("rien à modifier".into()));
    }
    if let Some(role) = &input.role {
        check_role(role)?;
        if id == auth.id {
            return Err(ApiError::Conflict(
                "vous ne pouvez pas changer votre propre rôle",
            ));
        }
    }
    if let Some(password) = &input.password {
        check_password(password)?;
    }
    if users::find_by_id(&state.db, &id).await?.is_none() {
        return Err(ApiError::NotFound);
    }
    let mut changed = Vec::new();
    if let Some(role) = &input.role {
        users::set_role(&state.db, &id, role).await?;
        changed.push("role");
    }
    if let Some(password) = input.password {
        let hash = hash_blocking(password).await?;
        users::set_password_hash(&state.db, &id, &hash).await?;
        changed.push("password");
    }
    refresh_tokens::revoke_all(&state.db, &id).await?;
    audit::record(
        &state,
        &auth.id,
        "user.update",
        "user",
        &id,
        json!({ "changed": changed, "role": input.role }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require_admin()?;
    if id == auth.id {
        return Err(ApiError::Conflict(
            "vous ne pouvez pas supprimer votre propre compte",
        ));
    }
    users::delete(&state.db, &id).await?;
    audit::record(&state, &auth.id, "user.delete", "user", &id, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::testing::{Harness, PASSWORD, bearer, harness, send};

    async fn create(h: &Harness, email: &str, role: &str) -> (StatusCode, serde_json::Value) {
        h.post(
            "/api/v1/users",
            json!({ "email": email, "password": "un-mot-de-passe-long", "role": role }),
        )
        .await
    }

    async fn login(h: &Harness, email: &str, password: &str) -> StatusCode {
        send(
            &h.app,
            "POST",
            "/api/v1/auth/login",
            &[],
            Some((
                "application/json",
                json!({ "email": email, "password": password }).to_string(),
            )),
        )
        .await
        .0
    }

    async fn call(h: &Harness, method: &str, uri: &str, body: serde_json::Value) -> StatusCode {
        send(
            &h.app,
            method,
            uri,
            &bearer(&h.admin),
            Some(("application/json", body.to_string())),
        )
        .await
        .0
    }

    async fn admin_id(h: &Harness) -> String {
        h.get("/api/v1/auth/me").await.1["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn an_admin_creates_a_user_who_can_then_sign_in() {
        let h = harness().await;
        let (status, created) = create(&h, "Nouveau@Example.com", "operator").await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(
            (created["email"].as_str(), created["role"].as_str()),
            (Some("nouveau@example.com"), Some("operator"))
        );
        assert!(created.get("password_hash").is_none());
        assert_eq!(
            login(&h, "nouveau@example.com", "un-mot-de-passe-long").await,
            StatusCode::OK
        );
        let (_, listed) = h.get("/api/v1/users").await;
        assert_eq!(listed.as_array().unwrap().len(), 2);
        assert!(!listed.to_string().contains("hash"));
    }

    #[tokio::test]
    async fn creation_validates_email_password_role_and_uniqueness() {
        let h = harness().await;
        let bad = |email: &str, password: &str, role: &str| {
            let body = json!({ "email": email, "password": password, "role": role });
            let (app, token) = (h.app.clone(), h.admin.clone());
            async move {
                send(
                    &app,
                    "POST",
                    "/api/v1/users",
                    &bearer(&token),
                    Some(("application/json", body.to_string())),
                )
                .await
                .0
            }
        };
        assert_eq!(
            bad("pas-un-mail", "un-mot-de-passe-long", "viewer").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            bad("a@b.co", "court", "viewer").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            bad("a@b.co", "un-mot-de-passe-long", "root").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            create(&h, "ada@example.com", "viewer").await.0,
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn only_admins_manage_users() {
        let h = harness().await;
        let viewer = h.viewer_token().await;
        let new_user = json!({ "email": "x@example.com", "password": "un-mot-de-passe-long", "role": "viewer" });
        for (method, uri, body) in [
            ("GET", "/api/v1/users", json!({})),
            ("POST", "/api/v1/users", new_user),
            ("PATCH", "/api/v1/users/x", json!({ "role": "viewer" })),
            ("DELETE", "/api/v1/users/x", json!({})),
        ] {
            let (status, _) = send(
                &h.app,
                method,
                uri,
                &bearer(&viewer),
                Some(("application/json", body.to_string())),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        }
        let (status, _) = send(&h.app, "GET", "/api/v1/users", &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn changing_a_role_or_password_signs_the_user_out_everywhere() {
        let h = harness().await;
        let (_, created) = create(&h, "bob@example.com", "viewer").await;
        let id = created["id"].as_str().unwrap();
        let (_, tokens) = send(
            &h.app,
            "POST",
            "/api/v1/auth/login",
            &[],
            Some((
                "application/json",
                json!({ "email": "bob@example.com", "password": "un-mot-de-passe-long" })
                    .to_string(),
            )),
        )
        .await;
        let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();
        let uri = format!("/api/v1/users/{id}");
        assert_eq!(
            call(
                &h,
                "PATCH",
                &uri,
                json!({ "role": "operator", "password": "un-autre-mot-de-passe" })
            )
            .await,
            StatusCode::NO_CONTENT
        );
        let (status, _) = send(
            &h.app,
            "POST",
            "/api/v1/auth/refresh",
            &[],
            Some((
                "application/json",
                json!({ "refresh_token": refresh }).to_string(),
            )),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            login(&h, "bob@example.com", "un-mot-de-passe-long").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            login(&h, "bob@example.com", "un-autre-mot-de-passe").await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn update_validates_its_input() {
        let h = harness().await;
        let (_, created) = create(&h, "bob@example.com", "viewer").await;
        let uri = format!("/api/v1/users/{}", created["id"].as_str().unwrap());
        assert_eq!(
            call(&h, "PATCH", &uri, json!({})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&h, "PATCH", &uri, json!({ "role": "root" })).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&h, "PATCH", &uri, json!({ "password": "court" })).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(
                &h,
                "PATCH",
                "/api/v1/users/nope",
                json!({ "role": "viewer" })
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn an_admin_cannot_change_the_own_role_or_delete_themselves() {
        let h = harness().await;
        let me = admin_id(&h).await;
        create(&h, "second@example.com", "admin").await;
        assert_eq!(
            call(
                &h,
                "PATCH",
                &format!("/api/v1/users/{me}"),
                json!({ "role": "viewer" })
            )
            .await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            call(&h, "DELETE", &format!("/api/v1/users/{me}"), json!({})).await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            call(
                &h,
                "PATCH",
                &format!("/api/v1/users/{me}"),
                json!({ "password": "mon-nouveau-mot-de-passe" })
            )
            .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            login(&h, "ada@example.com", PASSWORD).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn the_last_other_admin_cannot_be_removed_from_under_the_platform() {
        let h = harness().await;
        let (_, second) = create(&h, "second@example.com", "admin").await;
        let second_id = second["id"].as_str().unwrap();
        assert_eq!(
            call(
                &h,
                "DELETE",
                &format!("/api/v1/users/{second_id}"),
                json!({})
            )
            .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            call(&h, "DELETE", "/api/v1/users/nope", json!({})).await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn user_changes_are_audited_without_passwords() {
        let h = harness().await;
        let (_, created) = create(&h, "bob@example.com", "viewer").await;
        let uri = format!("/api/v1/users/{}", created["id"].as_str().unwrap());
        call(
            &h,
            "PATCH",
            &uri,
            json!({ "role": "operator", "password": "super-secret-pass-123" }),
        )
        .await;
        call(&h, "DELETE", &uri, json!({})).await;
        let (_, events) = h.get("/api/v1/audit?limit=100").await;
        let text = events.to_string();
        for action in ["user.create", "user.update", "user.delete"] {
            assert!(text.contains(action), "{action}");
        }
        assert!(!text.contains("super-secret-pass-123") && !text.contains("un-mot-de-passe-long"));
    }
}
