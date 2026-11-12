use std::collections::BTreeMap;

use sqlx::SqlitePool;

#[derive(Debug, thiserror::Error)]
pub enum IntegrationError {
    #[error("une intégration porte déjà ce nom dans ce projet")]
    AlreadyExists,
    #[error("en-têtes illisibles: {0}")]
    Headers(#[from] serde_json::Error),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRecord {
    pub id: String,
    pub project: String,
    pub name: String,
    pub base_url: String,
    pub headers: BTreeMap<String, String>,
    pub created_at: String,
}

type Row = (String, String, String, String, String, String);

const COLUMNS: &str = "id, project, name, base_url, headers, created_at";

fn record(row: Row) -> Result<IntegrationRecord, serde_json::Error> {
    let (id, project, name, base_url, headers, created_at) = row;
    Ok(IntegrationRecord {
        id,
        project,
        name,
        base_url,
        headers: serde_json::from_str(&headers)?,
        created_at,
    })
}

pub async fn create(
    pool: &SqlitePool,
    project: &str,
    name: &str,
    base_url: &str,
    headers: &BTreeMap<String, String>,
) -> Result<IntegrationRecord, IntegrationError> {
    let row: Result<Row, sqlx::Error> = sqlx::query_as(&format!(
        "INSERT INTO integrations (id, project, name, base_url, headers) VALUES (?, ?, ?, ?, ?) \
         RETURNING {COLUMNS}"
    ))
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(project)
    .bind(name)
    .bind(base_url)
    .bind(serde_json::to_string(headers)?)
    .fetch_one(pool)
    .await;
    match row {
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            Err(IntegrationError::AlreadyExists)
        }
        other => Ok(record(other?)?),
    }
}

pub async fn for_project(
    pool: &SqlitePool,
    project: &str,
) -> Result<Vec<IntegrationRecord>, IntegrationError> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM integrations WHERE project = ? ORDER BY name"
    ))
    .bind(project)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(record).collect::<Result<_, _>>()?)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<IntegrationRecord>, IntegrationError> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM integrations ORDER BY project, name"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(record).collect::<Result<_, _>>()?)
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM integrations WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        == 1)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    async fn pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("sw.db")).await.unwrap();
        (pool, dir)
    }

    fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[tokio::test]
    async fn create_then_read_back_with_headers() {
        let (pool, _dir) = pool().await;
        let created = create(
            &pool,
            "p",
            "hook",
            "https://example.com",
            &headers(&[("x-team", "a")]),
        )
        .await
        .unwrap();
        let listed = for_project(&pool, "p").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
        assert_eq!(listed[0].base_url, "https://example.com");
        assert_eq!(listed[0].headers, headers(&[("x-team", "a")]));
    }

    #[tokio::test]
    async fn names_are_unique_per_project_only() {
        let (pool, _dir) = pool().await;
        create(&pool, "p", "hook", "https://a.example", &BTreeMap::new())
            .await
            .unwrap();
        assert!(matches!(
            create(&pool, "p", "hook", "https://b.example", &BTreeMap::new()).await,
            Err(IntegrationError::AlreadyExists)
        ));
        create(&pool, "q", "hook", "https://b.example", &BTreeMap::new())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn projects_do_not_see_each_others_integrations() {
        let (pool, _dir) = pool().await;
        create(&pool, "p", "a", "https://a.example", &BTreeMap::new())
            .await
            .unwrap();
        create(&pool, "q", "b", "https://b.example", &BTreeMap::new())
            .await
            .unwrap();
        let names: Vec<String> = for_project(&pool, "p")
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(names, ["a"]);
        assert_eq!(list(&pool).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn delete_reports_whether_something_was_removed() {
        let (pool, _dir) = pool().await;
        let created = create(&pool, "p", "a", "https://a.example", &BTreeMap::new())
            .await
            .unwrap();
        assert!(delete(&pool, &created.id).await.unwrap());
        assert!(!delete(&pool, &created.id).await.unwrap());
        assert!(for_project(&pool, "p").await.unwrap().is_empty());
    }
}
