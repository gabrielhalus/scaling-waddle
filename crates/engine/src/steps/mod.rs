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
    unavailable: BTreeMap<String, String>,
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
            unavailable: BTreeMap::new(),
            client,
        }
    }

    pub fn mark_unavailable(mut self, name: impl Into<String>, reason: impl Into<String>) -> Self {
        self.unavailable.insert(name.into(), reason.into());
        self
    }

    pub(crate) fn unavailable_reason(&self, name: &str) -> Option<&str> {
        self.unavailable.get(name).map(String::as_str)
    }
}

impl Default for Services {
    fn default() -> Self {
        Self::new(Policy::default(), BTreeMap::new())
    }
}

// Remplace `${secret:NOM}` ; une référence inconnue est une erreur, jamais un texte laissé en clair.
pub fn resolve_secret_refs(
    text: &str,
    secrets: &BTreeMap<String, String>,
) -> Result<String, String> {
    const OPEN: &str = "${secret:";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let end = after.find('}').ok_or("référence de secret non fermée")?;
        let name = &after[..end];
        let value = secrets
            .get(name)
            .ok_or_else(|| format!("secret inconnu: {name}"))?;
        out.push_str(value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

pub async fn execute(kind: &str, with: &Value, vars: &Value, services: &Services) -> StepOutcome {
    match kind {
        "condition" => condition(with, vars),
        "transform" => transform(with, vars),
        "http.request" => http::execute(with, vars, services).await,
        "notification" => notification(with, vars, services).await,
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

async fn notification(with: &Value, vars: &Value, services: &Services) -> StepOutcome {
    let Some(message) = with.get("message").filter(|message| message.is_string()) else {
        return StepOutcome::fatal("`with.message` est requis et doit être une chaîne");
    };
    let mut request = with.as_object().cloned().unwrap_or_default();
    request.remove("message");
    request
        .entry("method")
        .or_insert_with(|| Value::String("POST".into()));
    request
        .entry("body")
        .or_insert_with(|| json!({ "message": message }));
    http::execute(&Value::Object(request), vars, services).await
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

    mod notification {
        use std::collections::BTreeMap;

        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        use super::*;
        use crate::netguard::Policy;

        fn services(server: &MockServer) -> Services {
            let integration = Integration {
                base_url: server.uri(),
                headers: BTreeMap::new(),
            };
            Services::new(
                Policy::parse_allowed("127.0.0.0/8").unwrap(),
                BTreeMap::from([("chat".to_owned(), integration)]),
            )
        }

        #[tokio::test]
        async fn posts_the_rendered_message_as_json_by_default() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/hooks/x"))
                .and(body_json(json!({ "message": "Déployé api" })))
                .respond_with(ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;
            let vars = json!({ "event": { "payload": { "service": "api" } } });
            let outcome = execute(
                "notification",
                &json!({ "integration": "chat", "path": "/hooks/x", "message": "Déployé ${{ event.payload.service }}" }),
                &vars,
                &services(&server),
            )
            .await;
            assert!(matches!(outcome, StepOutcome::Succeeded(_)), "{outcome:?}");
        }

        #[tokio::test]
        async fn a_custom_body_replaces_the_default_shape() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(body_json(json!({ "text": "salut" })))
                .respond_with(ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;
            let outcome = execute(
                "notification",
                &json!({ "integration": "chat", "message": "ignoré", "body": { "text": "salut" } }),
                &json!({}),
                &services(&server),
            )
            .await;
            assert!(matches!(outcome, StepOutcome::Succeeded(_)), "{outcome:?}");
        }

        #[tokio::test]
        async fn a_message_is_required() {
            let outcome = execute(
                "notification",
                &json!({ "integration": "chat" }),
                &json!({}),
                &Services::default(),
            )
            .await;
            assert!(matches!(
                outcome,
                StepOutcome::Failed {
                    retryable: false,
                    ..
                }
            ));
        }

        #[tokio::test]
        async fn delivery_errors_follow_the_http_retry_rules() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(503))
                .mount(&server)
                .await;
            let outcome = execute(
                "notification",
                &json!({ "integration": "chat", "message": "x" }),
                &json!({}),
                &services(&server),
            )
            .await;
            assert!(
                matches!(
                    outcome,
                    StepOutcome::Failed {
                        retryable: true,
                        ..
                    }
                ),
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn secret_refs_are_replaced_in_place() {
        let secrets = BTreeMap::from([("gh".to_owned(), "tok".to_owned())]);
        assert_eq!(
            resolve_secret_refs("Bearer ${secret:gh}", &secrets).unwrap(),
            "Bearer tok"
        );
        assert_eq!(
            resolve_secret_refs("sans référence", &secrets).unwrap(),
            "sans référence"
        );
        assert_eq!(
            resolve_secret_refs("${secret:gh}-${secret:gh}", &secrets).unwrap(),
            "tok-tok"
        );
    }

    #[test]
    fn unknown_or_unterminated_secret_refs_are_errors() {
        let secrets = BTreeMap::new();
        assert_eq!(
            resolve_secret_refs("${secret:nope}", &secrets),
            Err("secret inconnu: nope".into())
        );
        assert!(resolve_secret_refs("${secret:oups", &secrets).is_err());
    }

    #[test]
    fn secret_values_are_not_reinterpreted() {
        let secrets = BTreeMap::from([("a".to_owned(), "${secret:b}".to_owned())]);
        assert_eq!(
            resolve_secret_refs("${secret:a}", &secrets).unwrap(),
            "${secret:b}"
        );
    }
}
