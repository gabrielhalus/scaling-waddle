use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sw_domain::workflow::Workflow;
use sw_infrastructure::workflows::{self, WorkflowError, WorkflowRecord};

use crate::AppState;
use crate::audit;
use crate::auth::AuthUser;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/workflows", get(list).post(create))
        .route("/api/v1/workflows/import", post(import))
        .route("/api/v1/workflows/{id}", get(detail))
        .route("/api/v1/workflows/{id}/export", get(export))
        .route("/api/v1/workflows/{id}/versions", post(add_version))
        .route("/api/v1/workflows/{id}/activate", post(activate))
        .route("/api/v1/workflows/{id}/deactivate", post(deactivate))
}

#[derive(Deserialize)]
struct Activation {
    version: i64,
}

fn record_json(record: &WorkflowRecord) -> Value {
    json!({
        "id": record.id,
        "project": record.project,
        "name": record.name,
        "active_version": record.active_version,
        "latest_version": record.latest_version,
    })
}

fn parse_definition(headers: &HeaderMap, body: &Bytes) -> Result<Workflow, ApiError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| ApiError::BadRequest("le corps doit être de l'UTF-8".into()))?;
    let is_yaml = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("yaml"));
    let parsed = if is_yaml {
        Workflow::from_yaml(text).map_err(|err| err.to_string())
    } else {
        Workflow::from_json(text).map_err(|err| err.to_string())
    };
    let workflow =
        parsed.map_err(|err| ApiError::BadRequest(format!("définition illisible: {err}")))?;
    workflow.validate().map_err(|errors| ApiError::Invalid {
        message: "définition invalide".into(),
        details: errors.iter().map(ToString::to_string).collect(),
    })?;
    Ok(workflow)
}

impl From<WorkflowError> for ApiError {
    fn from(err: WorkflowError) -> Self {
        match err {
            WorkflowError::AlreadyExists => {
                Self::Conflict("un workflow porte déjà ce nom dans ce projet")
            }
            WorkflowError::NotFound => Self::NotFound,
            WorkflowError::IdentityMismatch => Self::BadRequest(
                "le nom et le projet d'une nouvelle version doivent rester identiques".into(),
            ),
            WorkflowError::Definition(_) | WorkflowError::Db(_) => {
                tracing::error!(%err, "erreur workflows");
                Self::Internal
            }
        }
    }
}

async fn list(State(state): State<AppState>, _auth: AuthUser) -> Result<Json<Value>, ApiError> {
    let records = workflows::list(&state.db).await?;
    Ok(Json(Value::Array(
        records.iter().map(record_json).collect(),
    )))
}

async fn detail(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let record = workflows::get(&state.db, &id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let versions = workflows::versions(&state.db, &id).await?;
    Ok(Json(json!({
        "workflow": record_json(&record),
        "versions": versions
            .iter()
            .map(|v| json!({ "version": v.version, "hash": v.hash, "created_at": v.created_at }))
            .collect::<Vec<_>>(),
    })))
}

async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    auth.require_writer()?;
    let workflow = parse_definition(&headers, &body)?;
    let record = workflows::create(&state.db, &workflow).await?;
    audit::record(
        &state,
        &auth.id,
        "workflow.create",
        "workflow",
        &record.id,
        json!({ "name": record.name, "project": record.project }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(record_json(&record))))
}

async fn add_version(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    auth.require_writer()?;
    let workflow = parse_definition(&headers, &body)?;
    let version = workflows::add_version(&state.db, &id, &workflow).await?;
    audit::record(
        &state,
        &auth.id,
        "workflow.version",
        "workflow",
        &id,
        json!({ "version": version }),
    )
    .await;
    Ok(Json(json!({ "version": version })))
}

#[derive(Deserialize)]
struct ExportQuery {
    version: Option<i64>,
    format: Option<String>,
}

async fn export(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let record = workflows::get(&state.db, &id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let version = query
        .version
        .or(record.active_version)
        .unwrap_or(record.latest_version);
    let workflow = workflows::get_version(&state.db, &id, version)
        .await?
        .ok_or(ApiError::NotFound)?;
    match query.format.as_deref().unwrap_or("yaml") {
        "yaml" => {
            let text = workflow.to_yaml().map_err(|err| {
                tracing::error!(%err, "export YAML en échec");
                ApiError::Internal
            })?;
            Ok(([(header::CONTENT_TYPE, "application/yaml")], text).into_response())
        }
        "json" => Ok(Json(workflow).into_response()),
        _ => Err(ApiError::BadRequest(
            "format doit valoir yaml ou json".into(),
        )),
    }
}

async fn import(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    auth.require_writer()?;
    let workflow = parse_definition(&headers, &body)?;
    let (record, version, created) = match workflows::find(
        &state.db,
        &workflow.metadata.project,
        &workflow.metadata.name,
    )
    .await?
    {
        Some(existing) => {
            let version = workflows::add_version(&state.db, &existing.id, &workflow).await?;
            (existing, version, false)
        }
        None => (workflows::create(&state.db, &workflow).await?, 1, true),
    };
    audit::record(
        &state,
        &auth.id,
        "workflow.import",
        "workflow",
        &record.id,
        json!({ "version": version, "created": created }),
    )
    .await;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(json!({ "id": record.id, "version": version, "created": created })),
    ))
}

async fn activate(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(activation): Json<Activation>,
) -> Result<StatusCode, ApiError> {
    auth.require_writer()?;
    workflows::activate(&state.db, &id, activation.version).await?;
    audit::record(
        &state,
        &auth.id,
        "workflow.activate",
        "workflow",
        &id,
        json!({ "version": activation.version }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn deactivate(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require_writer()?;
    workflows::deactivate(&state.db, &id).await?;
    audit::record(
        &state,
        &auth.id,
        "workflow.deactivate",
        "workflow",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
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

    struct Harness {
        app: axum::Router,
        state: AppState,
        admin: String,
        _dir: tempfile::TempDir,
    }

    async fn harness() -> Harness {
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
            None,
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

    async fn send(
        app: &axum::Router,
        method: &str,
        uri: &str,
        bearer: Option<&str>,
        body: Option<(&str, String)>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
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

    fn workflow(name: &str, delay: &str) -> Value {
        json!({
            "apiVersion": "scaling-waddle.dev/v1",
            "kind": "Workflow",
            "metadata": { "name": name, "project": "p" },
            "spec": {
                "trigger": { "type": "manual" },
                "steps": [{ "id": "a", "type": "delay", "with": { "for": delay } }]
            }
        })
    }

    async fn post_json(h: &Harness, uri: &str, body: Value) -> (StatusCode, Value) {
        send(
            &h.app,
            "POST",
            uri,
            Some(&h.admin),
            Some(("application/json", body.to_string())),
        )
        .await
    }

    async fn get(h: &Harness, uri: &str) -> (StatusCode, Value) {
        send(&h.app, "GET", uri, Some(&h.admin), None).await
    }

    async fn create(h: &Harness, name: &str) -> String {
        let (status, body) = post_json(h, "/api/v1/workflows", workflow(name, "1s")).await;
        assert_eq!(status, StatusCode::CREATED);
        body["id"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn routes_require_authentication() {
        let h = harness().await;
        let (status, _) = send(&h.app, "GET", "/api/v1/workflows", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_returns_the_record() {
        let h = harness().await;
        let (status, body) = post_json(&h, "/api/v1/workflows", workflow("w", "1s")).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["name"], "w");
        assert_eq!(body["latest_version"], 1);
        assert_eq!(body["active_version"], Value::Null);
    }

    #[tokio::test]
    async fn create_accepts_yaml() {
        let h = harness().await;
        let yaml = "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {name: y, project: p}\n\
                    spec:\n  trigger: {type: manual}\n  steps:\n    - {id: a, type: delay}\n";
        let (status, body) = send(
            &h.app,
            "POST",
            "/api/v1/workflows",
            Some(&h.admin),
            Some(("application/yaml", yaml.to_owned())),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["name"], "y");
    }

    #[tokio::test]
    async fn invalid_definition_is_rejected_with_details() {
        let h = harness().await;
        let mut bad = workflow("w", "1s");
        bad["spec"]["steps"][0]["needs"] = json!(["ghost"]);
        let (status, body) = post_json(&h, "/api/v1/workflows", bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["details"][0].as_str().unwrap().contains("ghost"));
    }

    #[tokio::test]
    async fn unparsable_body_is_a_400() {
        let h = harness().await;
        let (status, body) = send(
            &h.app,
            "POST",
            "/api/v1/workflows",
            Some(&h.admin),
            Some(("application/json", "{pas du json".to_owned())),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn duplicate_name_is_a_409() {
        let h = harness().await;
        create(&h, "w").await;
        let (status, _) = post_json(&h, "/api/v1/workflows", workflow("w", "2s")).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn list_and_detail_show_versions() {
        let h = harness().await;
        let id = create(&h, "w").await;
        let (status, list) = get(&h, "/api/v1/workflows").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list.as_array().unwrap().len(), 1);
        let (status, detail) = get(&h, &format!("/api/v1/workflows/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["workflow"]["id"], id.as_str());
        assert_eq!(detail["versions"].as_array().unwrap().len(), 1);
        assert_eq!(detail["versions"][0]["version"], 1);
    }

    #[tokio::test]
    async fn unknown_workflow_is_a_404() {
        let h = harness().await;
        let (status, _) = get(&h, "/api/v1/workflows/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) =
            post_json(&h, "/api/v1/workflows/nope/versions", workflow("w", "1s")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn new_version_increments_and_same_definition_is_idempotent() {
        let h = harness().await;
        let id = create(&h, "w").await;
        let uri = format!("/api/v1/workflows/{id}/versions");
        let (status, body) = post_json(&h, &uri, workflow("w", "2s")).await;
        assert_eq!((status, &body["version"]), (StatusCode::OK, &json!(2)));
        let (_, again) = post_json(&h, &uri, workflow("w", "2s")).await;
        assert_eq!(again["version"], 2);
    }

    #[tokio::test]
    async fn version_with_another_identity_is_a_400() {
        let h = harness().await;
        let id = create(&h, "w").await;
        let (status, _) = post_json(
            &h,
            &format!("/api/v1/workflows/{id}/versions"),
            workflow("other", "1s"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn activate_then_deactivate() {
        let h = harness().await;
        let id = create(&h, "w").await;
        let (status, _) = post_json(
            &h,
            &format!("/api/v1/workflows/{id}/activate"),
            json!({ "version": 1 }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, detail) = get(&h, &format!("/api/v1/workflows/{id}")).await;
        assert_eq!(detail["workflow"]["active_version"], 1);

        let (status, _) =
            post_json(&h, &format!("/api/v1/workflows/{id}/deactivate"), json!({})).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, detail) = get(&h, &format!("/api/v1/workflows/{id}")).await;
        assert_eq!(detail["workflow"]["active_version"], Value::Null);
    }

    #[tokio::test]
    async fn activating_an_unknown_version_is_a_404() {
        let h = harness().await;
        let id = create(&h, "w").await;
        let (status, _) = post_json(
            &h,
            &format!("/api/v1/workflows/{id}/activate"),
            json!({ "version": 9 }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_viewer_can_read_but_not_write() {
        let h = harness().await;
        let hash = sw_infrastructure::password::hash(PASSWORD).unwrap();
        sw_infrastructure::users::create(&h.state.db, "vic@example.com", &hash, "viewer")
            .await
            .unwrap();
        let (_, tokens) = send(
            &h.app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some((
                "application/json",
                json!({ "email": "vic@example.com", "password": PASSWORD }).to_string(),
            )),
        )
        .await;
        let viewer = tokens["access_token"].as_str().unwrap();

        let (status, _) = send(&h.app, "GET", "/api/v1/workflows", Some(viewer), None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(
            &h.app,
            "POST",
            "/api/v1/workflows",
            Some(viewer),
            Some(("application/json", workflow("w", "1s").to_string())),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    mod portability {
        use axum::http::StatusCode;
        use serde_json::json;

        use crate::testing::{bearer, harness, send, send_raw, workflow};

        const YAML: &str = "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {name: imported, project: p}\n\
                            spec:\n  trigger: {type: manual}\n  steps:\n    - {id: a, type: transform, with: {value: 1}}\n";

        #[tokio::test]
        async fn export_defaults_to_yaml_of_the_active_version() {
            let h = harness().await;
            let id = h.active_workflow("a").await;
            let (status, content_type, body) = send_raw(
                &h.app,
                "GET",
                &format!("/api/v1/workflows/{id}/export"),
                &bearer(&h.admin),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert!(content_type.contains("yaml"), "{content_type}");
            let parsed = sw_domain::workflow::Workflow::from_yaml(&body).unwrap();
            assert_eq!(parsed.metadata.name, "a");
            assert!(parsed.validate().is_ok());
        }

        #[tokio::test]
        async fn export_in_json_and_by_version() {
            let h = harness().await;
            let id = h.active_workflow("a").await;
            let mut second = workflow("a");
            second["spec"]["steps"][0]["with"] = json!({ "for": "2s" });
            h.post(&format!("/api/v1/workflows/{id}/versions"), second)
                .await;
            let (status, v1) = h
                .get(&format!(
                    "/api/v1/workflows/{id}/export?format=json&version=1"
                ))
                .await;
            assert_eq!(status, StatusCode::OK);
            assert!(
                v1["spec"]["steps"][0].get("with").is_none()
                    || v1["spec"]["steps"][0]["with"].is_null()
                    || v1["spec"]["steps"][0]["with"] != json!({ "for": "2s" })
            );
            let (_, v2) = h
                .get(&format!(
                    "/api/v1/workflows/{id}/export?format=json&version=2"
                ))
                .await;
            assert_eq!(v2["spec"]["steps"][0]["with"], json!({ "for": "2s" }));
            let (_, active) = h
                .get(&format!("/api/v1/workflows/{id}/export?format=json"))
                .await;
            assert_eq!(active, v1);
        }

        #[tokio::test]
        async fn export_rejects_unknown_formats_versions_and_workflows() {
            let h = harness().await;
            let id = h.active_workflow("a").await;
            let (status, _) = h
                .get(&format!("/api/v1/workflows/{id}/export?format=toml"))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let (status, _) = h
                .get(&format!("/api/v1/workflows/{id}/export?version=9"))
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _) = h.get("/api/v1/workflows/nope/export").await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn import_creates_then_versions_then_is_idempotent() {
            let h = harness().await;
            let call = |body: String| {
                let app = h.app.clone();
                let token = h.admin.clone();
                async move {
                    send(
                        &app,
                        "POST",
                        "/api/v1/workflows/import",
                        &bearer(&token),
                        Some(("application/yaml", body)),
                    )
                    .await
                }
            };
            let (status, first) = call(YAML.to_owned()).await;
            assert_eq!(
                (status, &first["created"], &first["version"]),
                (StatusCode::CREATED, &json!(true), &json!(1))
            );
            let (status, same) = call(YAML.to_owned()).await;
            assert_eq!(
                (status, &same["created"], &same["version"]),
                (StatusCode::OK, &json!(false), &json!(1))
            );
            assert_eq!(same["id"], first["id"]);
            let (_, changed) = call(YAML.replace("value: 1", "value: 2")).await;
            assert_eq!(changed["version"], 2);
            let (_, listed) = h.get("/api/v1/workflows").await;
            assert_eq!(listed.as_array().unwrap().len(), 1);
        }

        #[tokio::test]
        async fn an_exported_workflow_can_be_imported_elsewhere() {
            let source = harness().await;
            let id = source.active_workflow("portable").await;
            let (_, _, yaml) = send_raw(
                &source.app,
                "GET",
                &format!("/api/v1/workflows/{id}/export"),
                &bearer(&source.admin),
                None,
            )
            .await;
            let target = harness().await;
            let (status, body) = send(
                &target.app,
                "POST",
                "/api/v1/workflows/import",
                &bearer(&target.admin),
                Some(("application/yaml", yaml)),
            )
            .await;
            assert_eq!(
                (status, &body["created"]),
                (StatusCode::CREATED, &json!(true))
            );
        }

        #[tokio::test]
        async fn import_validates_authorizes_and_traces() {
            let h = harness().await;
            let (status, _) = send(
                &h.app,
                "POST",
                "/api/v1/workflows/import",
                &bearer(&h.admin),
                Some((
                    "application/yaml",
                    YAML.replace("needs", "x")
                        .replace("steps:", "steps: []\n  old:"),
                )),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let viewer = h.viewer_token().await;
            let (status, _) = send(
                &h.app,
                "POST",
                "/api/v1/workflows/import",
                &bearer(&viewer),
                Some(("application/yaml", YAML.to_owned())),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            send(
                &h.app,
                "POST",
                "/api/v1/workflows/import",
                &bearer(&h.admin),
                Some(("application/yaml", YAML.to_owned())),
            )
            .await;
            let (_, events) = h.get("/api/v1/audit?action=workflow.import").await;
            assert_eq!(events.as_array().unwrap().len(), 1);
        }
    }
}
