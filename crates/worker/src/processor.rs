use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sqlx::SqlitePool;
use sw_domain::workflow::Step;
use sw_engine::netguard::Policy;
use sw_engine::scheduler::{self, Action, Plan, RunOutcome, StepState};
use sw_engine::steps::{self, Integration, Services, StepOutcome};
use sw_infrastructure::runs::{self, Run, RunError};
use sw_infrastructure::workflows::WorkflowError;
use sw_infrastructure::{integrations, workflows};
use tokio::task::{JoinHandle, JoinSet};

const MAX_BACKOFF_SECS: u64 = 3600;

#[derive(Debug, Clone)]
pub struct Settings {
    pub owner: String,
    pub lease_ms: i64,
    pub backoff_unit: Duration,
    pub policy: Policy,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Workflow(#[from] WorkflowError),
    #[error(transparent)]
    Integration(#[from] integrations::IntegrationError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("données du run introuvables: {0}")]
    Missing(&'static str),
    #[error("état d'étape inconnu: {0}")]
    UnknownState(String),
    #[error("tâche d'étape interrompue")]
    Task,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

struct Heartbeat(JoinHandle<()>);

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn start_heartbeat(pool: SqlitePool, run_id: String, settings: &Settings) -> Heartbeat {
    let owner = settings.owner.clone();
    let lease_ms = settings.lease_ms;
    let every = Duration::from_millis(u64::try_from(lease_ms / 3).unwrap_or(1000).max(1));
    Heartbeat(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match runs::heartbeat(&pool, &run_id, &owner, now_ms(), lease_ms).await {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(%run_id, "bail perdu, le heartbeat s'arrête");
                    return;
                }
                Err(err) => tracing::warn!(%run_id, %err, "heartbeat en échec"),
            }
        }
    }))
}

pub async fn process_run(
    pool: &SqlitePool,
    run: &Run,
    settings: &Settings,
) -> Result<(), ProcessError> {
    runs::reset_running_steps(pool, &run.id).await?;
    let workflow = workflows::get_version(pool, &run.workflow_id, run.version)
        .await?
        .ok_or(ProcessError::Missing("version du workflow"))?;
    let event = runs::event_of(pool, &run.event_id)
        .await?
        .ok_or(ProcessError::Missing("événement"))?;
    let known = integrations::for_project(pool, &workflow.metadata.project).await?;
    let services = Arc::new(Services::new(
        settings.policy.clone(),
        known
            .into_iter()
            .map(|i| {
                (
                    i.name,
                    Integration {
                        base_url: i.base_url,
                        headers: i.headers,
                    },
                )
            })
            .collect(),
    ));
    let plan = Plan::from_workflow(&workflow);
    let definitions: BTreeMap<String, Step> = workflow
        .spec
        .steps
        .iter()
        .map(|step| (step.id.clone(), step.clone()))
        .collect();
    let _heartbeat = start_heartbeat(pool.clone(), run.id.clone(), settings);

    loop {
        if runs::cancel_requested(pool, &run.id).await? {
            runs::cancel_pending_steps(pool, &run.id).await?;
        }
        let rows = runs::steps(pool, &run.id).await?;
        let mut states = BTreeMap::new();
        let mut step_vars = serde_json::Map::new();
        for row in &rows {
            let state = StepState::parse(&row.status)
                .ok_or_else(|| ProcessError::UnknownState(row.status.clone()))?;
            states.insert(row.step_id.clone(), state);
            let output: Value = row
                .output
                .as_deref()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            let result = output.get("result").cloned().unwrap_or(Value::Null);
            step_vars.insert(
                row.step_id.clone(),
                json!({ "status": row.status, "output": output, "result": result }),
            );
        }
        if let Some(outcome) = scheduler::outcome(&states) {
            let (status, error) = summarise(outcome, &rows);
            if !runs::finish(pool, &run.id, &settings.owner, status, error.as_deref()).await? {
                tracing::warn!(run_id = %run.id, "run non terminé : bail perdu");
            }
            return Ok(());
        }
        let vars = json!({
            "event": {
                "source": event.source,
                "type": event.kind,
                "payload": event.payload,
                "correlation_id": event.correlation_id,
            },
            "steps": step_vars,
        });

        let actions = scheduler::decide(&plan, &states, &vars);
        if actions.is_empty() {
            runs::finish(
                pool,
                &run.id,
                &settings.owner,
                "failed",
                Some("plan bloqué : aucune étape exécutable"),
            )
            .await?;
            return Ok(());
        }
        let mut tasks = JoinSet::new();
        for action in actions {
            match action {
                Action::Skip(id, reason) => {
                    runs::set_step(pool, &run.id, &id, "skipped", None, Some(&reason), 0).await?;
                }
                Action::Fail(id, error) => {
                    runs::set_step(pool, &run.id, &id, "failed", None, Some(&error), 0).await?;
                }
                Action::Start(id) => {
                    let Some(step) = definitions.get(&id).cloned() else {
                        continue;
                    };
                    tasks.spawn(run_step(
                        pool.clone(),
                        run.id.clone(),
                        step,
                        vars.clone(),
                        Arc::clone(&services),
                        settings.backoff_unit,
                    ));
                }
            }
        }
        while let Some(joined) = tasks.join_next().await {
            joined.map_err(|_| ProcessError::Task)??;
        }
    }
}

fn summarise(outcome: RunOutcome, rows: &[runs::StepRun]) -> (&'static str, Option<String>) {
    match outcome {
        RunOutcome::Succeeded => ("succeeded", None),
        RunOutcome::Cancelled => ("cancelled", None),
        RunOutcome::Failed => {
            let error = rows.iter().find(|row| row.status == "failed").map(|row| {
                format!(
                    "étape {} : {}",
                    row.step_id,
                    row.error.as_deref().unwrap_or("échec")
                )
            });
            ("failed", error)
        }
    }
}

async fn run_step(
    pool: SqlitePool,
    run_id: String,
    step: Step,
    vars: Value,
    services: Arc<Services>,
    backoff_unit: Duration,
) -> Result<(), RunError> {
    let max_attempts = step.max_attempts();
    let base_backoff = step.retry.map_or(1, |retry| retry.backoff_seconds);
    let mut attempt = 1;
    loop {
        runs::set_step(
            &pool,
            &run_id,
            &step.id,
            "running",
            None,
            None,
            i64::from(attempt),
        )
        .await?;
        match steps::execute(&step.kind, &step.with, &vars, &services).await {
            StepOutcome::Succeeded(output) => {
                return runs::set_step(
                    &pool,
                    &run_id,
                    &step.id,
                    "succeeded",
                    Some(&output),
                    None,
                    i64::from(attempt),
                )
                .await;
            }
            StepOutcome::Failed { error, retryable } if retryable && attempt < max_attempts => {
                tracing::warn!(%run_id, step = %step.id, attempt, %error, "étape en échec, nouvelle tentative");
                let secs = base_backoff
                    .saturating_mul(1 << (attempt - 1).min(20))
                    .min(MAX_BACKOFF_SECS);
                tokio::time::sleep(backoff_unit * u32::try_from(secs).unwrap_or(u32::MAX)).await;
                attempt += 1;
            }
            StepOutcome::Failed { error, .. } => {
                return runs::set_step(
                    &pool,
                    &run_id,
                    &step.id,
                    "failed",
                    None,
                    Some(&error),
                    i64::from(attempt),
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};
    use sw_domain::workflow::Workflow;
    use sw_engine::netguard::Policy;
    use sw_infrastructure::runs::{self, NewEvent};
    use sw_infrastructure::{integrations, workflows};
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    struct Env {
        pool: sqlx::SqlitePool,
        settings: Settings,
        _dir: tempfile::TempDir,
    }

    async fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let pool = sw_infrastructure::db::open(&dir.path().join("sw.db"))
            .await
            .unwrap();
        let settings = Settings {
            owner: "w1".into(),
            lease_ms: 30_000,
            backoff_unit: Duration::from_millis(1),
            policy: Policy::parse_allowed("127.0.0.0/8").unwrap(),
        };
        Env {
            pool,
            settings,
            _dir: dir,
        }
    }

    fn workflow(steps: &str) -> Workflow {
        Workflow::from_yaml(&format!(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {{name: w, project: p}}\n\
             spec:\n  trigger: {{type: webhook}}\n  steps:\n{steps}"
        ))
        .unwrap()
    }

    // Crée, active, ingère et réserve le run, comme le ferait la boucle du worker.
    async fn queued(env: &Env, steps: &str, payload: Value) -> runs::Run {
        let record = workflows::create(&env.pool, &workflow(steps))
            .await
            .unwrap();
        workflows::activate(&env.pool, &record.id, 1).await.unwrap();
        runs::ingest(
            &env.pool,
            &NewEvent {
                workflow_id: record.id,
                source: "webhook".into(),
                kind: "webhook.received".into(),
                payload,
                correlation_id: "corr".into(),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
        runs::claim(&env.pool, "w1", now_ms(), env.settings.lease_ms)
            .await
            .unwrap()
            .unwrap()
    }

    async fn statuses(env: &Env, run_id: &str) -> Vec<(String, String)> {
        runs::steps(&env.pool, run_id)
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.step_id, s.status))
            .collect()
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    async fn add_integration(env: &Env, server: &MockServer) {
        integrations::create(
            &env.pool,
            "p",
            "hook",
            &server.uri(),
            &std::collections::BTreeMap::new(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_linear_run_succeeds_and_stores_outputs() {
        let env = env().await;
        let run = queued(
            &env,
            "    - {id: check, type: condition, with: {expression: 'event.payload.n == 2'}}\n    \
             - {id: shape, type: transform, needs: [check], with: {value: {double: '${{ event.payload.n * 2 }}'}}}\n",
            json!({ "n": 2 }),
        )
        .await;

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        let done = runs::get(&env.pool, &run.id).await.unwrap().unwrap();
        assert_eq!(done.status, "succeeded");
        assert_eq!(
            statuses(&env, &run.id).await,
            pairs(&[("check", "succeeded"), ("shape", "succeeded")])
        );
        let shape = &runs::steps(&env.pool, &run.id).await.unwrap()[1];
        assert_eq!(shape.output.as_deref(), Some("{\"value\":{\"double\":4}}"));
    }

    #[tokio::test]
    async fn a_false_condition_skips_dependents_and_the_run_still_succeeds() {
        let env = env().await;
        let run = queued(
            &env,
            "    - {id: check, type: condition, with: {expression: 'event.payload.n == 99'}}\n    \
             - {id: shape, type: transform, needs: [check], if: 'steps.check.result == true', with: {value: 1}}\n",
            json!({ "n": 2 }),
        )
        .await;

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        assert_eq!(
            runs::get(&env.pool, &run.id).await.unwrap().unwrap().status,
            "succeeded"
        );
        assert_eq!(
            statuses(&env, &run.id).await,
            pairs(&[("check", "succeeded"), ("shape", "skipped")])
        );
    }

    #[tokio::test]
    async fn http_step_uses_the_integration_and_the_event_payload() {
        let env = env().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/deployments"))
            .and(body_json(json!({ "service": "api" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 7 })))
            .expect(1)
            .mount(&server)
            .await;
        add_integration(&env, &server).await;
        let run = queued(
            &env,
            "    - id: notify\n      type: http.request\n      with:\n        integration: hook\n        \
             method: POST\n        path: /deployments\n        body: {service: '${{ event.payload.service }}'}\n",
            json!({ "service": "api" }),
        )
        .await;

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        assert_eq!(
            runs::get(&env.pool, &run.id).await.unwrap().unwrap().status,
            "succeeded"
        );
        let step = &runs::steps(&env.pool, &run.id).await.unwrap()[0];
        assert!(step.output.as_deref().unwrap().contains("\"id\":7"));
    }

    #[tokio::test]
    async fn retryable_failures_are_retried_up_to_max_attempts() {
        let env = env().await;
        let server = MockServer::start().await;
        Mock::given(path("/flaky"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        add_integration(&env, &server).await;
        let run = queued(
            &env,
            "    - {id: call, type: http.request, retry: {max_attempts: 3, backoff_seconds: 1}, \
             with: {integration: hook, path: /flaky}}\n",
            json!({}),
        )
        .await;

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        let done = runs::get(&env.pool, &run.id).await.unwrap().unwrap();
        assert_eq!(done.status, "failed");
        assert!(done.error.as_deref().unwrap().contains("call"));
        let step = &runs::steps(&env.pool, &run.id).await.unwrap()[0];
        assert_eq!((step.status.as_str(), step.attempts), ("failed", 3));
        assert_eq!(step.error.as_deref(), Some("HTTP 503"));
    }

    #[tokio::test]
    async fn a_step_that_recovers_stops_retrying() {
        let env = env().await;
        let server = MockServer::start().await;
        Mock::given(path("/flaky"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(path("/flaky"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        add_integration(&env, &server).await;
        let run = queued(
            &env,
            "    - {id: call, type: http.request, retry: {max_attempts: 3}, with: {integration: hook, path: /flaky}}\n",
            json!({}),
        )
        .await;

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        assert_eq!(
            runs::get(&env.pool, &run.id).await.unwrap().unwrap().status,
            "succeeded"
        );
        let step = &runs::steps(&env.pool, &run.id).await.unwrap()[0];
        assert_eq!((step.status.as_str(), step.attempts), ("succeeded", 2));
    }

    #[tokio::test]
    async fn non_retryable_failures_are_not_retried() {
        let env = env().await;
        let server = MockServer::start().await;
        Mock::given(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        add_integration(&env, &server).await;
        let run = queued(
            &env,
            "    - {id: call, type: http.request, retry: {max_attempts: 5}, with: {integration: hook, path: /missing}}\n",
            json!({}),
        )
        .await;

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        let step = &runs::steps(&env.pool, &run.id).await.unwrap()[0];
        assert_eq!((step.status.as_str(), step.attempts), ("failed", 1));
    }

    #[tokio::test]
    async fn a_failure_skips_dependents_but_independent_branches_finish() {
        let env = env().await;
        let run = queued(
            &env,
            "    - {id: bad, type: condition, with: {expression: 'nope.field'}}\n    \
             - {id: after, type: transform, needs: [bad], with: {value: 1}}\n    \
             - {id: side, type: transform, with: {value: 2}}\n",
            json!({}),
        )
        .await;

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        let done = runs::get(&env.pool, &run.id).await.unwrap().unwrap();
        assert_eq!(done.status, "failed");
        assert_eq!(
            statuses(&env, &run.id).await,
            pairs(&[
                ("bad", "failed"),
                ("after", "skipped"),
                ("side", "succeeded")
            ])
        );
    }

    #[tokio::test]
    async fn steps_interrupted_by_a_crash_are_replayed() {
        let env = env().await;
        let run = queued(
            &env,
            "    - {id: a, type: transform, with: {value: 1}}\n    - {id: b, type: transform, needs: [a], with: {value: 2}}\n",
            json!({}),
        )
        .await;
        runs::set_step(
            &env.pool,
            &run.id,
            "a",
            "succeeded",
            Some(&json!({ "value": 1 })),
            None,
            1,
        )
        .await
        .unwrap();
        runs::set_step(&env.pool, &run.id, "b", "running", None, None, 1)
            .await
            .unwrap();

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        assert_eq!(
            runs::get(&env.pool, &run.id).await.unwrap().unwrap().status,
            "succeeded"
        );
        assert_eq!(
            statuses(&env, &run.id).await,
            pairs(&[("a", "succeeded"), ("b", "succeeded")])
        );
    }

    #[tokio::test]
    async fn unimplemented_step_types_fail_the_run() {
        let env = env().await;
        let run = queued(&env, "    - {id: wait, type: delay}\n", json!({})).await;

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        let done = runs::get(&env.pool, &run.id).await.unwrap().unwrap();
        assert_eq!(done.status, "failed");
    }

    #[tokio::test]
    async fn processing_with_a_foreign_lease_does_not_finish_the_run() {
        let env = env().await;
        let run = queued(
            &env,
            "    - {id: a, type: transform, with: {value: 1}}\n",
            json!({}),
        )
        .await;
        let mut settings = env.settings.clone();
        settings.owner = "intrus".into();

        process_run(&env.pool, &run, &settings).await.unwrap();

        assert_eq!(
            runs::get(&env.pool, &run.id).await.unwrap().unwrap().status,
            "running"
        );
    }

    #[tokio::test]
    async fn a_cancel_request_stops_the_run_after_the_current_step() {
        let env = env().await;
        let run = queued(
            &env,
            "    - {id: a, type: transform, with: {value: 1}}\n    - {id: b, type: transform, needs: [a], with: {value: 2}}\n",
            json!({}),
        )
        .await;
        runs::set_step(
            &env.pool,
            &run.id,
            "a",
            "succeeded",
            Some(&json!({ "value": 1 })),
            None,
            1,
        )
        .await
        .unwrap();
        runs::request_cancel(&env.pool, &run.id).await.unwrap();

        process_run(&env.pool, &run, &env.settings).await.unwrap();

        assert_eq!(
            runs::get(&env.pool, &run.id).await.unwrap().unwrap().status,
            "cancelled"
        );
        assert_eq!(
            statuses(&env, &run.id).await,
            pairs(&[("a", "succeeded"), ("b", "cancelled")])
        );
    }
}
