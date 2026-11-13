pub mod http;

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::expr;
use crate::netguard::Policy;

#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    Succeeded(Value),
    Failed { error: String, retryable: bool },
}

impl StepOutcome {
    pub fn fatal(error: impl Into<String>) -> Self {
        Self::Failed {
            error: error.into(),
            retryable: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Integration {
    pub base_url: String,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Services {
    policy: Policy,
    integrations: BTreeMap<String, Integration>,
    client: Option<reqwest::Client>,
}

impl Services {
    pub fn new(policy: Policy, integrations: BTreeMap<String, Integration>) -> Self {
        let client = http::build_client(&policy)
            .inspect_err(|err| tracing::error!(%err, "client HTTP indisponible"))
            .ok();
        Self {
            policy,
            integrations,
            client,
        }
    }
}

impl Default for Services {
    fn default() -> Self {
        Self::new(Policy::default(), BTreeMap::new())
    }
}

pub async fn execute(kind: &str, with: &Value, vars: &Value, services: &Services) -> StepOutcome {
    match kind {
        "condition" => condition(with, vars),
        "transform" => transform(with, vars),
        "http.request" => http::execute(with, vars, services).await,
        other => StepOutcome::fatal(format!("type d'étape non implémenté: {other}")),
    }
}

pub const MAX_DELAY_SECS: u64 = 30 * 24 * 60 * 60;

pub fn delay_seconds(with: &Value, vars: &Value) -> Result<u64, String> {
    let raw = with.get("seconds").ok_or("`with.seconds` est requis")?;
    let rendered = expr::render(raw, vars).map_err(|err| err.to_string())?;
    let seconds = match &rendered {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
    .ok_or("`with.seconds` doit être un entier positif")?;
    if (1..=MAX_DELAY_SECS).contains(&seconds) {
        Ok(seconds)
    } else {
        Err(format!(
            "`with.seconds` doit être compris entre 1 et {MAX_DELAY_SECS}"
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub message: Option<String>,
    pub timeout_seconds: Option<u64>,
}

pub fn approval_request(with: &Value, vars: &Value) -> Result<ApprovalRequest, String> {
    let message = match with.get("message") {
        None | Some(Value::Null) => None,
        Some(raw) => match expr::render(raw, vars).map_err(|err| err.to_string())? {
            Value::String(text) => Some(text),
            other => Some(other.to_string()),
        },
    };
    let timeout_seconds = match with.get("timeout_seconds") {
        None | Some(Value::Null) => None,
        Some(_) => Some(
            delay_seconds(&json!({ "seconds": with["timeout_seconds"] }), vars)
                .map_err(|err| err.replace("with.seconds", "with.timeout_seconds"))?,
        ),
    };
    Ok(ApprovalRequest {
        message,
        timeout_seconds,
    })
}

fn condition(with: &Value, vars: &Value) -> StepOutcome {
    let Some(expression) = with.get("expression").and_then(Value::as_str) else {
        return StepOutcome::fatal("`with.expression` est requis et doit être une chaîne");
    };
    match expr::evaluate_bool(expression, vars) {
        Ok(result) => StepOutcome::Succeeded(json!({ "result": result })),
        Err(err) => StepOutcome::fatal(err.to_string()),
    }
}

fn transform(with: &Value, vars: &Value) -> StepOutcome {
    let Some(value) = with.get("value") else {
        return StepOutcome::fatal("`with.value` est requis");
    };
    match expr::render(value, vars) {
        Ok(rendered) => StepOutcome::Succeeded(json!({ "value": rendered })),
        Err(err) => StepOutcome::fatal(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn vars() -> Value {
        json!({ "event": { "payload": { "service": "api", "count": 3 } }, "steps": {} })
    }

    async fn run(kind: &str, with: Value) -> StepOutcome {
        execute(kind, &with, &vars(), &Services::default()).await
    }

    fn failed(outcome: &StepOutcome) -> (&str, bool) {
        match outcome {
            StepOutcome::Failed { error, retryable } => (error.as_str(), *retryable),
            StepOutcome::Succeeded(_) => panic!("succès inattendu"),
        }
    }

    #[tokio::test]
    async fn condition_outputs_its_boolean_result() {
        let yes = run(
            "condition",
            json!({ "expression": "event.payload.count == 3" }),
        )
        .await;
        assert_eq!(yes, StepOutcome::Succeeded(json!({ "result": true })));
        let no = run(
            "condition",
            json!({ "expression": "event.payload.count > 3" }),
        )
        .await;
        assert_eq!(no, StepOutcome::Succeeded(json!({ "result": false })));
    }

    #[tokio::test]
    async fn condition_failures_are_not_retryable() {
        for with in [
            json!({}),
            json!({ "expression": 12 }),
            json!({ "expression": "event.payload.count" }),
            json!({ "expression": "nope.field" }),
            json!({ "expression": "event.payload." }),
        ] {
            let outcome = run("condition", with.clone()).await;
            assert!(!failed(&outcome).1, "{with} ne doit pas être rejouable");
        }
    }

    #[tokio::test]
    async fn transform_renders_templates_into_value() {
        let outcome = run(
            "transform",
            json!({ "value": { "service": "${{ event.payload.service }}", "n": "${{ event.payload.count }}" } }),
        )
        .await;
        assert_eq!(
            outcome,
            StepOutcome::Succeeded(json!({ "value": { "service": "api", "n": 3 } }))
        );
    }

    #[tokio::test]
    async fn transform_requires_a_value_and_valid_templates() {
        let missing = run("transform", json!({})).await;
        assert!(!failed(&missing).1);
        let broken = run("transform", json!({ "value": "${{ nope.field }}" })).await;
        assert!(!failed(&broken).1);
    }

    #[tokio::test]
    async fn unimplemented_types_fail_without_retry() {
        for kind in [
            "delay",
            "approval",
            "notification",
            "agent.run",
            "script.run",
            "integration.x.y",
            "bogus",
        ] {
            let outcome = run(kind, json!({})).await;
            let (error, retryable) = failed(&outcome);
            assert!(error.contains(kind));
            assert!(!retryable);
        }
    }

    #[test]
    fn delay_seconds_reads_integers_and_templates() {
        let vars = json!({ "event": { "payload": { "n": 90 } } });
        assert_eq!(delay_seconds(&json!({ "seconds": 5 }), &vars), Ok(5));
        assert_eq!(delay_seconds(&json!({ "seconds": "7" }), &vars), Ok(7));
        assert_eq!(
            delay_seconds(&json!({ "seconds": "${{ event.payload.n }}" }), &vars),
            Ok(90)
        );
    }

    #[test]
    fn delay_seconds_rejects_missing_invalid_and_out_of_range_values() {
        let vars = json!({});
        for with in [
            json!({}),
            json!({ "seconds": 0 }),
            json!({ "seconds": -3 }),
            json!({ "seconds": 1.5 }),
            json!({ "seconds": "abc" }),
            json!({ "seconds": true }),
            json!({ "seconds": MAX_DELAY_SECS + 1 }),
        ] {
            assert!(delay_seconds(&with, &vars).is_err(), "{with}");
        }
        assert_eq!(
            delay_seconds(&json!({ "seconds": MAX_DELAY_SECS }), &vars),
            Ok(MAX_DELAY_SECS)
        );
    }

    #[test]
    fn approval_request_reads_message_and_optional_timeout() {
        let vars = json!({ "event": { "payload": { "service": "api" } } });
        let request = approval_request(
            &json!({ "message": "Déployer ${{ event.payload.service }} ?", "timeout_seconds": 600 }),
            &vars,
        )
        .unwrap();
        assert_eq!(request.message.as_deref(), Some("Déployer api ?"));
        assert_eq!(request.timeout_seconds, Some(600));
        let bare = approval_request(&json!({}), &vars).unwrap();
        assert_eq!((bare.message, bare.timeout_seconds), (None, None));
    }

    #[test]
    fn approval_request_rejects_a_bad_timeout() {
        let error = approval_request(&json!({ "timeout_seconds": 0 }), &json!({})).unwrap_err();
        assert!(error.contains("timeout_seconds"), "{error}");
    }
}
