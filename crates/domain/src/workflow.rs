use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const API_VERSION: &str = "scaling-waddle.dev/v1";
pub const KIND: &str = "Workflow";

const STEP_TYPES: &[&str] = &[
    "http.request",
    "condition",
    "transform",
    "delay",
    "notification",
    "approval",
    "agent.run",
    "script.run",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: Spec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
    pub project: String,
}

use crate::cron;

fn valid_events(config: &Value) -> bool {
    match config.get("events") {
        None | Some(Value::Null) => true,
        Some(Value::Array(items)) => {
            (1..=50).contains(&items.len())
                && items.iter().all(|item| {
                    item.as_str()
                        .is_some_and(|text| !text.trim().is_empty() && text.len() <= 80)
                })
        }
        Some(_) => false,
    }
}

// Motif exact, ou préfixe terminé par `.*` (`gitlab.*`).
pub fn event_matches(patterns: &[String], kind: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| match pattern.strip_suffix(".*") {
            Some(prefix) => kind
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.')),
            None => pattern == kind,
        })
}

const MAX_CONCURRENCY: u32 = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub trigger: Trigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default)]
    pub with: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    #[serde(default = "default_backoff_seconds")]
    pub backoff_seconds: u64,
}

fn default_backoff_seconds() -> u64 {
    1
}

impl Step {
    pub fn max_attempts(&self) -> u32 {
        self.retry.map_or(1, |retry| retry.max_attempts)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("apiVersion non supportée: {0}")]
    UnsupportedApiVersion(String),
    #[error("kind non supporté: {0}")]
    UnsupportedKind(String),
    #[error("le nom du workflow est vide")]
    EmptyName,
    #[error("le workflow n'a aucune étape")]
    NoSteps,
    #[error("identifiant d'étape invalide: {0}")]
    InvalidStepId(String),
    #[error("identifiant d'étape en double: {0}")]
    DuplicateStepId(String),
    #[error("politique de retry invalide pour {0} (1 à 10 tentatives, délai de 1 à 3600 s)")]
    InvalidRetry(String),
    #[error("concurrence invalide (1 à 100 runs simultanés)")]
    InvalidConcurrency,
    #[error("type de déclencheur non supporté: {0}")]
    UnsupportedTrigger(String),
    #[error(
        "déclencheur cron invalide: `config.schedule` doit être une expression à 5 champs valide"
    )]
    InvalidSchedule,
    #[error(
        "`config.events` doit être une liste de 1 à 50 motifs (texte non vide, 80 caractères max)"
    )]
    InvalidEvents,
    #[error("type d'étape inconnu pour {step}: {step_type}")]
    UnknownStepType { step: String, step_type: String },
    #[error("l'étape {step} dépend d'une étape inconnue: {needs}")]
    UnknownDependency { step: String, needs: String },
    #[error("l'étape {0} dépend d'elle-même")]
    SelfDependency(String),
    #[error("cycle entre les étapes: {0:?}")]
    Cycle(Vec<String>),
    #[error("l'étape {step} référence {reference} sans l'avoir en ancêtre")]
    UndeclaredReference { step: String, reference: String },
}

impl Workflow {
    pub fn from_yaml(text: &str) -> Result<Self, serde_yaml_ng::Error> {
        serde_yaml_ng::from_str(text)
    }

    pub fn to_yaml(&self) -> Result<String, serde_yaml_ng::Error> {
        serde_yaml_ng::to_string(self)
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn hash(&self) -> String {
        let canonical = serde_json::to_vec(self).unwrap_or_default();
        hex::encode(Sha256::digest(canonical))
    }

    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();
        if self.api_version != API_VERSION {
            errors.push(ValidationError::UnsupportedApiVersion(
                self.api_version.clone(),
            ));
        }
        if self.kind != KIND {
            errors.push(ValidationError::UnsupportedKind(self.kind.clone()));
        }
        if self.metadata.name.trim().is_empty() {
            errors.push(ValidationError::EmptyName);
        }
        if self.spec.steps.is_empty() {
            errors.push(ValidationError::NoSteps);
        }
        match self.spec.trigger.kind.as_str() {
            "manual" | "webhook" | "gitlab" | "uptime-kuma" => {}
            "cron" => {
                let valid = self
                    .spec
                    .trigger
                    .config
                    .get("schedule")
                    .and_then(Value::as_str)
                    .is_some_and(|schedule| cron::validate(schedule).is_ok());
                if !valid {
                    errors.push(ValidationError::InvalidSchedule);
                }
            }
            other => errors.push(ValidationError::UnsupportedTrigger(other.to_owned())),
        }
        if !valid_events(&self.spec.trigger.config) {
            errors.push(ValidationError::InvalidEvents);
        }
        if self
            .spec
            .concurrency
            .is_some_and(|limit| !(1..=MAX_CONCURRENCY).contains(&limit))
        {
            errors.push(ValidationError::InvalidConcurrency);
        }

        let ids = self.check_steps(&mut errors);
        self.check_dependencies(&ids, &mut errors);
        if errors.is_empty() {
            self.check_references(&mut errors);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn check_steps(&self, errors: &mut Vec<ValidationError>) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        for step in &self.spec.steps {
            if !is_valid_id(&step.id) {
                errors.push(ValidationError::InvalidStepId(step.id.clone()));
            } else if !ids.insert(step.id.clone()) {
                errors.push(ValidationError::DuplicateStepId(step.id.clone()));
            }
            if step.retry.is_some_and(|retry| {
                !(1..=10).contains(&retry.max_attempts)
                    || !(1..=3600).contains(&retry.backoff_seconds)
            }) {
                errors.push(ValidationError::InvalidRetry(step.id.clone()));
            }
            if !is_known_type(&step.kind) {
                errors.push(ValidationError::UnknownStepType {
                    step: step.id.clone(),
                    step_type: step.kind.clone(),
                });
            }
        }
        ids
    }

    fn check_dependencies(&self, ids: &BTreeSet<String>, errors: &mut Vec<ValidationError>) {
        let mut graph_ok = true;
        for step in &self.spec.steps {
            for needs in &step.needs {
                if *needs == step.id {
                    errors.push(ValidationError::SelfDependency(step.id.clone()));
                    graph_ok = false;
                } else if !ids.contains(needs) {
                    errors.push(ValidationError::UnknownDependency {
                        step: step.id.clone(),
                        needs: needs.clone(),
                    });
                    graph_ok = false;
                }
            }
        }
        if let Some(cycle) = graph_ok.then(|| self.find_cycle()).flatten() {
            errors.push(ValidationError::Cycle(cycle));
        }
    }

    // Retire les étapes sans dépendant non résolu (Kahn) ; le reste forme les cycles.
    fn find_cycle(&self) -> Option<Vec<String>> {
        let mut remaining: BTreeMap<&str, BTreeSet<&str>> = self
            .spec
            .steps
            .iter()
            .map(|s| (s.id.as_str(), s.needs.iter().map(String::as_str).collect()))
            .collect();
        loop {
            let ready: Vec<&str> = remaining
                .iter()
                .filter(|(_, needs)| needs.is_empty())
                .map(|(id, _)| *id)
                .collect();
            if ready.is_empty() {
                break;
            }
            for id in ready {
                remaining.remove(id);
                for needs in remaining.values_mut() {
                    needs.remove(id);
                }
            }
        }
        if remaining.is_empty() {
            return None;
        }
        // Ne garde que les étapes réellement sur un cycle, pas celles qui en dépendent seulement.
        let on_cycle: Vec<String> = remaining
            .keys()
            .filter(|id| reaches(&remaining, id, id))
            .map(|id| (*id).to_owned())
            .collect();
        Some(on_cycle)
    }

    fn check_references(&self, errors: &mut Vec<ValidationError>) {
        let direct: BTreeMap<&str, Vec<&str>> = self
            .spec
            .steps
            .iter()
            .map(|s| (s.id.as_str(), s.needs.iter().map(String::as_str).collect()))
            .collect();
        for step in &self.spec.steps {
            let ancestors = ancestors_of(&direct, &step.id);
            let mut references = BTreeSet::new();
            if let Some(condition) = &step.condition {
                collect_references(condition, &mut references);
            }
            collect_value_references(&step.with, &mut references);
            for reference in references {
                if !ancestors.contains(reference.as_str()) {
                    errors.push(ValidationError::UndeclaredReference {
                        step: step.id.clone(),
                        reference,
                    });
                }
            }
        }
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn is_known_type(kind: &str) -> bool {
    STEP_TYPES.contains(&kind)
        || kind
            .strip_prefix("integration.")
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('.') && !rest.ends_with('.'))
}

fn reaches(graph: &BTreeMap<&str, BTreeSet<&str>>, from: &str, target: &str) -> bool {
    let mut stack: Vec<&str> = graph
        .get(from)
        .map(|n| n.iter().copied().collect())
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if seen.insert(node) {
            stack.extend(graph.get(node).into_iter().flatten().copied());
        }
    }
    false
}

fn ancestors_of<'a>(direct: &BTreeMap<&'a str, Vec<&'a str>>, id: &str) -> BTreeSet<&'a str> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&str> = direct.get(id).cloned().unwrap_or_default();
    while let Some(node) = stack.pop() {
        if seen.insert(node) {
            stack.extend(direct.get(node).into_iter().flatten().copied());
        }
    }
    seen
}

fn collect_value_references(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => collect_references(text, out),
        Value::Array(items) => items.iter().for_each(|v| collect_value_references(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect_value_references(v, out)),
        _ => {}
    }
}

// Cherche `steps.<id>` dans une expression ou un gabarit.
fn collect_references(text: &str, out: &mut BTreeSet<String>) {
    let mut rest = text;
    while let Some(at) = rest.find("steps.") {
        let boundary =
            at == 0 || !rest[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.');
        let after = &rest[at + "steps.".len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(after.len());
        if boundary && end > 0 {
            out.insert(after[..end].to_owned());
        }
        rest = &after[end..];
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const VALID: &str = r#"
apiVersion: scaling-waddle.dev/v1
kind: Workflow
metadata:
  name: notify-on-deploy
  project: platform
spec:
  trigger:
    type: webhook
    config:
      auth: signed-secret
  steps:
    - id: check-status
      type: condition
      with:
        expression: 'event.payload.status == "success"'
    - id: notify
      type: http.request
      needs: [check-status]
      if: 'steps.check-status.result == true'
      with:
        integration: team-webhook
        body:
          service: '${{ event.payload.service }}'
"#;

    fn parse_yaml(text: &str) -> Workflow {
        Workflow::from_yaml(text).unwrap()
    }

    fn with_steps(steps: &str) -> String {
        format!(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata:\n  name: w\n  project: p\n\
             spec:\n  trigger:\n    type: manual\n  steps:\n{steps}"
        )
    }

    #[test]
    fn valid_definition_parses_and_validates() {
        let workflow = parse_yaml(VALID);
        assert_eq!(workflow.metadata.name, "notify-on-deploy");
        assert_eq!(workflow.spec.steps.len(), 2);
        assert_eq!(workflow.spec.steps[1].needs, vec!["check-status"]);
        assert!(workflow.validate().is_ok());
    }

    #[test]
    fn json_and_yaml_describe_the_same_workflow() {
        let from_yaml = parse_yaml(VALID);
        let json = serde_json::to_string(&from_yaml).unwrap();
        assert_eq!(Workflow::from_json(&json).unwrap(), from_yaml);
    }

    #[test]
    fn unknown_fields_are_rejected_at_parse_time() {
        let text = VALID.replace("kind: Workflow", "kind: Workflow\nsurprise: 1");
        assert!(Workflow::from_yaml(&text).is_err());
    }

    #[test]
    fn wrong_api_version_or_kind_is_rejected() {
        let mut workflow = parse_yaml(VALID);
        workflow.api_version = "v2".into();
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::UnsupportedApiVersion("v2".into())])
        );
        let mut workflow = parse_yaml(VALID);
        workflow.kind = "Job".into();
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::UnsupportedKind("Job".into())])
        );
    }

    #[test]
    fn empty_name_and_no_steps_are_rejected() {
        let mut workflow = parse_yaml(VALID);
        workflow.metadata.name = "  ".into();
        workflow.spec.steps.clear();
        let errors = workflow.validate().unwrap_err();
        assert!(errors.contains(&ValidationError::EmptyName));
        assert!(errors.contains(&ValidationError::NoSteps));
    }

    #[test]
    fn duplicate_step_ids_are_rejected() {
        let workflow = parse_yaml(&with_steps(
            "    - {id: a, type: delay}\n    - {id: a, type: delay}\n",
        ));
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::DuplicateStepId("a".into())])
        );
    }

    #[test]
    fn invalid_step_ids_are_rejected() {
        let workflow = parse_yaml(&with_steps("    - {id: 'Bad Id', type: delay}\n"));
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::InvalidStepId("Bad Id".into())])
        );
    }

    #[test]
    fn unknown_step_type_is_rejected_but_integration_prefix_is_allowed() {
        let workflow = parse_yaml(&with_steps(
            "    - {id: a, type: teleport}\n    - {id: b, type: integration.gitlab.issue}\n",
        ));
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::UnknownStepType {
                step: "a".into(),
                step_type: "teleport".into()
            }])
        );
    }

    #[test]
    fn unknown_dependency_is_rejected() {
        let workflow = parse_yaml(&with_steps("    - {id: a, type: delay, needs: [ghost]}\n"));
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::UnknownDependency {
                step: "a".into(),
                needs: "ghost".into()
            }])
        );
    }

    #[test]
    fn self_dependency_is_rejected() {
        let workflow = parse_yaml(&with_steps("    - {id: a, type: delay, needs: [a]}\n"));
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::SelfDependency("a".into())])
        );
    }

    #[test]
    fn cycles_are_rejected_and_named() {
        let workflow = parse_yaml(&with_steps(
            "    - {id: a, type: delay, needs: [c]}\n    - {id: b, type: delay, needs: [a]}\n    \
             - {id: c, type: delay, needs: [b]}\n    - {id: d, type: delay}\n",
        ));
        let errors = workflow.validate().unwrap_err();
        assert_eq!(
            errors,
            vec![ValidationError::Cycle(vec![
                "a".into(),
                "b".into(),
                "c".into()
            ])]
        );
    }

    #[test]
    fn diamond_dependencies_are_valid() {
        let workflow = parse_yaml(&with_steps(
            "    - {id: a, type: delay}\n    - {id: b, type: delay, needs: [a]}\n    \
             - {id: c, type: delay, needs: [a]}\n    - {id: d, type: delay, needs: [b, c]}\n",
        ));
        assert!(workflow.validate().is_ok());
    }

    #[test]
    fn expression_may_only_reference_ancestor_steps() {
        let workflow = parse_yaml(&with_steps(
            "    - {id: a, type: delay}\n    - {id: b, type: condition}\n    \
             - {id: c, type: delay, needs: [b], if: 'steps.a.result == true'}\n",
        ));
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::UndeclaredReference {
                step: "c".into(),
                reference: "a".into()
            }])
        );
    }

    #[test]
    fn transitive_ancestors_and_with_templates_are_checked() {
        let workflow = parse_yaml(&with_steps(
            "    - {id: a, type: condition}\n    - {id: b, type: delay, needs: [a]}\n    \
             - {id: c, type: http.request, needs: [b], with: {body: '${{ steps.a.output }}'}}\n    \
             - {id: d, type: http.request, with: {body: ['${{ steps.a.output }}']}}\n",
        ));
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::UndeclaredReference {
                step: "d".into(),
                reference: "a".into()
            }])
        );
    }

    #[test]
    fn reference_to_unknown_step_is_rejected() {
        let workflow = parse_yaml(&with_steps(
            "    - {id: a, type: delay, if: 'steps.ghost.result == true'}\n",
        ));
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::UndeclaredReference {
                step: "a".into(),
                reference: "ghost".into()
            }])
        );
    }

    #[test]
    fn retry_policy_parses_with_defaults() {
        let workflow = parse_yaml(&with_steps(
            "    - {id: a, type: delay, retry: {max_attempts: 3}}\n    \
             - {id: b, type: delay, retry: {max_attempts: 2, backoff_seconds: 5}}\n    - {id: c, type: delay}\n",
        ));
        assert!(workflow.validate().is_ok());
        let a = workflow.spec.steps[0].retry.unwrap();
        assert_eq!((a.max_attempts, a.backoff_seconds), (3, 1));
        let b = workflow.spec.steps[1].retry.unwrap();
        assert_eq!((b.max_attempts, b.backoff_seconds), (2, 5));
        assert!(workflow.spec.steps[2].retry.is_none());
        assert_eq!(workflow.spec.steps[2].max_attempts(), 1);
        assert_eq!(workflow.spec.steps[0].max_attempts(), 3);
    }

    #[test]
    fn retry_bounds_are_enforced() {
        for retry in [
            "{max_attempts: 0}",
            "{max_attempts: 11}",
            "{max_attempts: 2, backoff_seconds: 0}",
            "{max_attempts: 2, backoff_seconds: 4000}",
        ] {
            let workflow = parse_yaml(&with_steps(&format!(
                "    - {{id: a, type: delay, retry: {retry}}}\n"
            )));
            assert_eq!(
                workflow.validate(),
                Err(vec![ValidationError::InvalidRetry("a".into())]),
                "{retry}"
            );
        }
    }

    #[test]
    fn hash_is_stable_and_changes_with_content() {
        let a = parse_yaml(VALID);
        let b = parse_yaml(VALID);
        assert_eq!(a.hash(), b.hash());
        let mut c = parse_yaml(VALID);
        c.metadata.name = "other".into();
        assert_ne!(a.hash(), c.hash());
        assert_eq!(a.hash().len(), 64);
    }

    #[test]
    fn concurrency_must_be_between_1_and_100() {
        for (limit, ok) in [(1, true), (100, true), (0, false), (101, false)] {
            let mut workflow = parse_yaml(VALID);
            workflow.spec.concurrency = Some(limit);
            assert_eq!(workflow.validate().is_ok(), ok, "{limit}");
        }
    }

    #[test]
    fn concurrency_is_parsed_and_omitted_from_the_hash_when_absent() {
        let plain = parse_yaml(VALID);
        assert!(
            !serde_json::to_string(&plain)
                .unwrap()
                .contains("concurrency")
        );
        let mut limited = parse_yaml(VALID);
        limited.spec.concurrency = Some(2);
        assert_ne!(plain.hash(), limited.hash());
        let yaml = VALID.replace("  trigger:", "  concurrency: 2\n  trigger:");
        assert_eq!(parse_yaml(&yaml).spec.concurrency, Some(2));
    }

    #[test]
    fn trigger_types_are_restricted() {
        let mut workflow = parse_yaml(VALID);
        workflow.spec.trigger.kind = "telepathy".into();
        assert_eq!(
            workflow.validate(),
            Err(vec![ValidationError::UnsupportedTrigger(
                "telepathy".into()
            )])
        );
    }

    #[test]
    fn a_cron_trigger_needs_a_valid_schedule() {
        let mut workflow = parse_yaml(VALID);
        workflow.spec.trigger.kind = "cron".into();
        for config in [
            json!(null),
            json!({}),
            json!({ "schedule": 5 }),
            json!({ "schedule": "oups" }),
        ] {
            workflow.spec.trigger.config = config;
            assert_eq!(
                workflow.validate(),
                Err(vec![ValidationError::InvalidSchedule])
            );
        }
        workflow.spec.trigger.config = json!({ "schedule": "*/10 * * * *" });
        assert!(workflow.validate().is_ok());
    }

    #[test]
    fn yaml_export_roundtrips_to_the_same_workflow_and_hash() {
        let original = parse_yaml(VALID);
        let reparsed = parse_yaml(&original.to_yaml().unwrap());
        assert_eq!(original, reparsed);
        assert_eq!(original.hash(), reparsed.hash());
    }

    #[test]
    fn json_export_roundtrips_too() {
        let original = parse_yaml(VALID);
        let text = serde_json::to_string(&original).unwrap();
        assert_eq!(Workflow::from_json(&text).unwrap(), original);
    }

    #[test]
    fn connector_triggers_are_accepted() {
        for kind in ["gitlab", "uptime-kuma"] {
            let mut workflow = parse_yaml(VALID);
            workflow.spec.trigger.kind = kind.into();
            assert!(workflow.validate().is_ok(), "{kind}");
        }
    }

    #[test]
    fn trigger_events_must_be_a_bounded_list_of_patterns() {
        let mut workflow = parse_yaml(VALID);
        for (config, ok) in [
            (json!({ "events": ["gitlab.push"] }), true),
            (json!({ "events": null }), true),
            (json!({}), true),
            (json!({ "events": [] }), false),
            (json!({ "events": "gitlab.push" }), false),
            (json!({ "events": [""] }), false),
            (json!({ "events": [3] }), false),
            (json!({ "events": ["x".repeat(81)] }), false),
            (json!({ "events": vec!["a"; 51] }), false),
        ] {
            workflow.spec.trigger.config = config.clone();
            assert_eq!(workflow.validate().is_ok(), ok, "{config}");
        }
    }

    #[test]
    fn event_patterns_match_exactly_or_by_dotted_prefix() {
        let patterns = |list: &[&str]| list.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
        assert!(event_matches(&patterns(&["gitlab.push"]), "gitlab.push"));
        assert!(!event_matches(&patterns(&["gitlab.push"]), "gitlab.pushed"));
        assert!(event_matches(
            &patterns(&["gitlab.*"]),
            "gitlab.merge_request"
        ));
        assert!(!event_matches(&patterns(&["gitlab.*"]), "gitlab"));
        assert!(!event_matches(&patterns(&["gitlab.*"]), "gitlabx.push"));
        assert!(event_matches(
            &patterns(&["a.b", "uptime-kuma.down"]),
            "uptime-kuma.down"
        ));
        assert!(!event_matches(&patterns(&[]), "gitlab.push"));
    }
}
