use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{AppState, router};

pub const PASSWORD: &str = "correct horse battery";

pub struct Harness {
    pub app: axum::Router,
    pub state: AppState,
    pub admin: String,
    _dir: tempfile::TempDir,
}

pub async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let db = sw_infrastructure::db::open(&dir.path().join("sw.db"))
        .await
        .unwrap();
    let state = AppState::new(db, "0123456789abcdef0123456789abcdef");
    let app = router(state.clone());
    let (_, tokens) = send(
        &app,
        "POST",
        "/api/v1/auth/setup",
        &[],
        Some((
            "application/json",
            json!({ "email": "ada@example.com", "password": PASSWORD }).to_string(),
        )),
    )
    .await;
    let admin = tokens["access_token"].as_str().unwrap().to_owned();
    Harness {
        app,
        state,
        admin,
        _dir: dir,
    }
}

pub async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
    body: Option<(&str, String)>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, value);
    }
    let req = match body {
        Some((content_type, text)) => req
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(text)),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

pub fn bearer(token: &str) -> Vec<(&'static str, String)> {
    vec![("authorization", format!("Bearer {token}"))]
}

pub fn workflow(name: &str) -> Value {
    json!({
        "apiVersion": "scaling-waddle.dev/v1",
        "kind": "Workflow",
        "metadata": { "name": name, "project": "p" },
        "spec": { "trigger": { "type": "webhook" }, "steps": [{ "id": "a", "type": "delay" }] }
    })
}

impl Harness {
    pub async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        send(
            &self.app,
            "POST",
            uri,
            &bearer(&self.admin),
            Some(("application/json", body.to_string())),
        )
        .await
    }

    pub async fn get(&self, uri: &str) -> (StatusCode, Value) {
        send(&self.app, "GET", uri, &bearer(&self.admin), None).await
    }

    pub async fn active_workflow(&self, name: &str) -> String {
        let (_, created) = self.post("/api/v1/workflows", workflow(name)).await;
        let id = created["id"].as_str().unwrap().to_owned();
        let (status, _) = self
            .post(
                &format!("/api/v1/workflows/{id}/activate"),
                json!({ "version": 1 }),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        id
    }

    pub async fn viewer_token(&self) -> String {
        let hash = sw_infrastructure::password::hash(PASSWORD).unwrap();
        sw_infrastructure::users::create(&self.state.db, "vic@example.com", &hash, "viewer")
            .await
            .unwrap();
        let (_, tokens) = send(
            &self.app,
            "POST",
            "/api/v1/auth/login",
            &[],
            Some((
                "application/json",
                json!({ "email": "vic@example.com", "password": PASSWORD }).to_string(),
            )),
        )
        .await;
        tokens["access_token"].as_str().unwrap().to_owned()
    }
}
