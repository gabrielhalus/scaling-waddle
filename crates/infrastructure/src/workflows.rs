use sqlx::{FromRow, SqlitePool};
use sw_domain::workflow::Workflow;

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("un workflow porte déjà ce nom dans ce projet")]
    AlreadyExists,
    #[error("workflow ou version introuvable")]
    NotFound,
    #[error("le nom et le projet d'une nouvelle version doivent rester identiques")]
    IdentityMismatch,
    #[error("définition illisible: {0}")]
    Definition(#[from] serde_json::Error),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct WorkflowRecord {
    pub id: String,
    pub project: String,
    pub name: String,
    pub active_version: Option<i64>,
    pub latest_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct VersionRecord {
    pub version: i64,
    pub hash: String,
    pub created_at: String,
}

const SELECT: &str = "SELECT w.id, w.project, w.name, w.active_version, \
     COALESCE((SELECT MAX(version) FROM workflow_versions WHERE workflow_id = w.id), 0) AS latest_version \
     FROM workflows w";

pub async fn create(
    pool: &SqlitePool,
    workflow: &Workflow,
) -> Result<WorkflowRecord, WorkflowError> {
    let id = uuid::Uuid::new_v4().to_string();
    let definition = serde_json::to_string(workflow)?;
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query("INSERT INTO workflows (id, project, name) VALUES (?, ?, ?)")
        .bind(&id)
        .bind(&workflow.metadata.project)
        .bind(&workflow.metadata.name)
        .execute(&mut *tx)
        .await;
    match inserted {
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            return Err(WorkflowError::AlreadyExists);
        }
        other => {
            other?;
        }
    }
    sqlx::query("INSERT INTO workflow_versions (workflow_id, version, hash, definition) VALUES (?, 1, ?, ?)")
        .bind(&id)
        .bind(workflow.hash())
        .bind(definition)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    get(pool, &id).await?.ok_or(WorkflowError::NotFound)
}

pub async fn add_version(
    pool: &SqlitePool,
    workflow_id: &str,
    workflow: &Workflow,
) -> Result<i64, WorkflowError> {
    let record = get(pool, workflow_id)
        .await?
        .ok_or(WorkflowError::NotFound)?;
    if record.project != workflow.metadata.project || record.name != workflow.metadata.name {
        return Err(WorkflowError::IdentityMismatch);
    }
    let hash = workflow.hash();
    let definition = serde_json::to_string(workflow)?;
    let mut tx = pool.begin().await?;
    let latest: Option<(i64, String)> = sqlx::query_as(
        "SELECT version, hash FROM workflow_versions WHERE workflow_id = ? ORDER BY version DESC LIMIT 1",
    )
    .bind(workflow_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((version, _)) = latest.filter(|(_, latest_hash)| *latest_hash == hash) {
        return Ok(version);
    }
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO workflow_versions (workflow_id, version, hash, definition) \
         SELECT ?1, COALESCE(MAX(version), 0) + 1, ?2, ?3 FROM workflow_versions WHERE workflow_id = ?1 \
         RETURNING version",
    )
    .bind(workflow_id)
    .bind(hash)
    .bind(definition)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(version)
}

pub async fn activate(
    pool: &SqlitePool,
    workflow_id: &str,
    version: i64,
) -> Result<(), WorkflowError> {
    let updated = sqlx::query(
        "UPDATE workflows SET active_version = ?2 WHERE id = ?1 \
         AND EXISTS (SELECT 1 FROM workflow_versions WHERE workflow_id = ?1 AND version = ?2)",
    )
    .bind(workflow_id)
    .bind(version)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(WorkflowError::NotFound);
    }
    Ok(())
}

pub async fn deactivate(pool: &SqlitePool, workflow_id: &str) -> Result<(), WorkflowError> {
    let updated = sqlx::query("UPDATE workflows SET active_version = NULL WHERE id = ?")
        .bind(workflow_id)
        .execute(pool)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(WorkflowError::NotFound);
    }
    Ok(())
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<WorkflowRecord>, sqlx::Error> {
    sqlx::query_as(&format!("{SELECT} WHERE w.id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn find(
    pool: &SqlitePool,
    project: &str,
    name: &str,
) -> Result<Option<WorkflowRecord>, sqlx::Error> {
    sqlx::query_as(&format!("{SELECT} WHERE w.project = ? AND w.name = ?"))
        .bind(project)
        .bind(name)
        .fetch_optional(pool)
        .await
}

// Motifs `trigger.config.events` de la version active ; `None` si aucun filtre n'est défini.
pub async fn accepted_events(
    pool: &SqlitePool,
    workflow_id: &str,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    let raw: Option<Option<String>> = sqlx::query_scalar(
        "SELECT json_extract(v.definition, '$.spec.trigger.config.events') \
         FROM workflows w JOIN workflow_versions v \
             ON v.workflow_id = w.id AND v.version = w.active_version \
         WHERE w.id = ?",
    )
    .bind(workflow_id)
    .fetch_optional(pool)
    .await?;
    Ok(raw
        .flatten()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok()))
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<WorkflowRecord>, sqlx::Error> {
    sqlx::query_as(&format!("{SELECT} ORDER BY w.project, w.name"))
        .fetch_all(pool)
        .await
}

pub async fn versions(
    pool: &SqlitePool,
    workflow_id: &str,
) -> Result<Vec<VersionRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT version, hash, created_at FROM workflow_versions WHERE workflow_id = ? ORDER BY version",
    )
    .bind(workflow_id)
    .fetch_all(pool)
    .await
}

pub async fn get_version(
    pool: &SqlitePool,
    workflow_id: &str,
    version: i64,
) -> Result<Option<Workflow>, WorkflowError> {
    let row: Option<String> = sqlx::query_scalar(
        "SELECT definition FROM workflow_versions WHERE workflow_id = ? AND version = ?",
    )
    .bind(workflow_id)
    .bind(version)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|json| serde_json::from_str(&json)).transpose()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sw_domain::workflow::Workflow;

    fn definition(name: &str, delay: &str) -> Workflow {
        Workflow::from_yaml(&format!(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata:\n  name: {name}\n  project: p\n\
             spec:\n  trigger:\n    type: manual\n  steps:\n    - id: a\n      type: delay\n      with:\n        for: {delay}\n"
        ))
        .unwrap()
    }

    async fn pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn create_stores_version_one_inactive() {
        let (pool, _dir) = pool().await;
        let record = create(&pool, &definition("w", "1s")).await.unwrap();
        assert_eq!(record.latest_version, 1);
        assert_eq!(record.active_version, None);
        assert_eq!((record.project.as_str(), record.name.as_str()), ("p", "w"));
        let stored = get_version(&pool, &record.id, 1).await.unwrap().unwrap();
        assert_eq!(stored, definition("w", "1s"));
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_project_and_name() {
        let (pool, _dir) = pool().await;
        create(&pool, &definition("w", "1s")).await.unwrap();
        assert!(matches!(
            create(&pool, &definition("w", "2s")).await,
            Err(WorkflowError::AlreadyExists)
        ));
    }

    #[tokio::test]
    async fn add_version_increments_and_is_idempotent_on_same_hash() {
        let (pool, _dir) = pool().await;
        let record = create(&pool, &definition("w", "1s")).await.unwrap();
        assert_eq!(
            add_version(&pool, &record.id, &definition("w", "2s"))
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            add_version(&pool, &record.id, &definition("w", "2s"))
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            add_version(&pool, &record.id, &definition("w", "3s"))
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            get(&pool, &record.id)
                .await
                .unwrap()
                .unwrap()
                .latest_version,
            3
        );
    }

    #[tokio::test]
    async fn add_version_rejects_another_identity_or_unknown_workflow() {
        let (pool, _dir) = pool().await;
        let record = create(&pool, &definition("w", "1s")).await.unwrap();
        assert!(matches!(
            add_version(&pool, &record.id, &definition("other", "1s")).await,
            Err(WorkflowError::IdentityMismatch)
        ));
        assert!(matches!(
            add_version(&pool, "nope", &definition("w", "1s")).await,
            Err(WorkflowError::NotFound)
        ));
    }

    #[tokio::test]
    async fn activate_requires_an_existing_version() {
        let (pool, _dir) = pool().await;
        let record = create(&pool, &definition("w", "1s")).await.unwrap();
        assert!(matches!(
            activate(&pool, &record.id, 2).await,
            Err(WorkflowError::NotFound)
        ));
        activate(&pool, &record.id, 1).await.unwrap();
        assert_eq!(
            get(&pool, &record.id)
                .await
                .unwrap()
                .unwrap()
                .active_version,
            Some(1)
        );
    }

    #[tokio::test]
    async fn deactivate_clears_the_active_version() {
        let (pool, _dir) = pool().await;
        let record = create(&pool, &definition("w", "1s")).await.unwrap();
        activate(&pool, &record.id, 1).await.unwrap();
        deactivate(&pool, &record.id).await.unwrap();
        assert_eq!(
            get(&pool, &record.id)
                .await
                .unwrap()
                .unwrap()
                .active_version,
            None
        );
    }

    #[tokio::test]
    async fn list_and_versions_are_ordered() {
        let (pool, _dir) = pool().await;
        let b = create(&pool, &definition("b", "1s")).await.unwrap();
        create(&pool, &definition("a", "1s")).await.unwrap();
        add_version(&pool, &b.id, &definition("b", "2s"))
            .await
            .unwrap();
        let names: Vec<String> = list(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|w| w.name)
            .collect();
        assert_eq!(names, ["a", "b"]);
        let versions = versions(&pool, &b.id).await.unwrap();
        assert_eq!(
            versions.iter().map(|v| v.version).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(versions[0].hash, definition("b", "1s").hash());
    }

    #[tokio::test]
    async fn unknown_workflow_reads_as_none() {
        let (pool, _dir) = pool().await;
        assert!(get(&pool, "nope").await.unwrap().is_none());
        assert!(get_version(&pool, "nope", 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn find_looks_a_workflow_up_by_project_and_name() {
        let (pool, _dir) = pool().await;
        let created = create(&pool, &definition("w", "1s")).await.unwrap();
        let found = find(&pool, "p", "w").await.unwrap().unwrap();
        assert_eq!(found.id, created.id);
        assert!(find(&pool, "p", "autre").await.unwrap().is_none());
        assert!(find(&pool, "q", "w").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn accepted_events_come_from_the_active_version_only() {
        let (pool, _dir) = pool().await;
        let with = |events: &str| {
            Workflow::from_yaml(&format!(
                "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {{name: w, project: p}}\n\
                 spec:\n  trigger: {{type: gitlab{events}}}\n  steps:\n    - {{id: a, type: transform, with: {{value: 1}}}}\n"
            ))
            .unwrap()
        };
        let record = create(
            &pool,
            &with(", config: {events: ['gitlab.push', 'gitlab.tag_push']}"),
        )
        .await
        .unwrap();
        assert_eq!(accepted_events(&pool, &record.id).await.unwrap(), None);
        activate(&pool, &record.id, 1).await.unwrap();
        assert_eq!(
            accepted_events(&pool, &record.id).await.unwrap(),
            Some(vec!["gitlab.push".to_owned(), "gitlab.tag_push".to_owned()])
        );
        add_version(&pool, &record.id, &with("")).await.unwrap();
        assert_eq!(
            accepted_events(&pool, &record.id)
                .await
                .unwrap()
                .map(|e| e.len()),
            Some(2)
        );
        activate(&pool, &record.id, 2).await.unwrap();
        assert_eq!(accepted_events(&pool, &record.id).await.unwrap(), None);
        assert_eq!(accepted_events(&pool, "inconnu").await.unwrap(), None);
    }
}
