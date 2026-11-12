use std::collections::BTreeMap;

use serde_json::Value;
use sw_domain::workflow::Workflow;

use crate::expr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Waiting,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

impl StepState {
    pub fn parse(status: &str) -> Option<Self> {
        Some(match status {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    fn is_final(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Skipped | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Start(String),
    Skip(String, String),
    Fail(String, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct PlannedStep {
    pub id: String,
    pub needs: Vec<String>,
    pub condition: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub steps: Vec<PlannedStep>,
}

impl Plan {
    pub fn from_workflow(workflow: &Workflow) -> Self {
        Self {
            steps: workflow
                .spec
                .steps
                .iter()
                .map(|step| PlannedStep {
                    id: step.id.clone(),
                    needs: step.needs.clone(),
                    condition: step.condition.clone(),
                })
                .collect(),
        }
    }
}

// Jointure ALL : une étape part quand toutes ses dépendances ont réussi ; sinon elle est sautée.
pub fn decide(plan: &Plan, states: &BTreeMap<String, StepState>, vars: &Value) -> Vec<Action> {
    let state_of = |id: &str| states.get(id).copied().unwrap_or(StepState::Pending);
    let mut actions = Vec::new();
    for step in &plan.steps {
        if state_of(&step.id) != StepState::Pending {
            continue;
        }
        let needs: Vec<StepState> = step.needs.iter().map(|id| state_of(id)).collect();
        if !needs.iter().all(|state| state.is_final()) {
            continue;
        }
        if needs.iter().any(|state| *state != StepState::Succeeded) {
            actions.push(Action::Skip(
                step.id.clone(),
                "dépendance non réussie".into(),
            ));
            continue;
        }
        match step
            .condition
            .as_deref()
            .map(|condition| expr::evaluate_bool(condition, vars))
        {
            None | Some(Ok(true)) => actions.push(Action::Start(step.id.clone())),
            Some(Ok(false)) => {
                actions.push(Action::Skip(step.id.clone(), "condition fausse".into()))
            }
            Some(Err(err)) => actions.push(Action::Fail(step.id.clone(), err.to_string())),
        }
    }
    actions
}

pub fn outcome(states: &BTreeMap<String, StepState>) -> Option<RunOutcome> {
    if !states.values().all(|state| state.is_final()) {
        return None;
    }
    Some(
        if states.values().any(|state| *state == StepState::Failed) {
            RunOutcome::Failed
        } else if states.values().any(|state| *state == StepState::Cancelled) {
            RunOutcome::Cancelled
        } else {
            RunOutcome::Succeeded
        },
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use sw_domain::workflow::Workflow;

    use super::*;

    fn plan(steps: &str) -> Plan {
        let workflow = Workflow::from_yaml(&format!(
            "apiVersion: scaling-waddle.dev/v1\nkind: Workflow\nmetadata: {{name: w, project: p}}\n\
             spec:\n  trigger: {{type: manual}}\n  steps:\n{steps}"
        ))
        .unwrap();
        Plan::from_workflow(&workflow)
    }

    fn states(pairs: &[(&str, StepState)]) -> BTreeMap<String, StepState> {
        pairs
            .iter()
            .map(|(id, state)| ((*id).to_owned(), *state))
            .collect()
    }

    fn vars() -> serde_json::Value {
        json!({ "event": { "payload": { "ok": true } }, "steps": {} })
    }

    use StepState::{Failed, Pending, Running, Skipped, Succeeded};

    #[test]
    fn roots_start_immediately_and_dependents_wait() {
        let plan = plan("    - {id: a, type: delay}\n    - {id: b, type: delay, needs: [a]}\n");
        let actions = decide(&plan, &states(&[("a", Pending), ("b", Pending)]), &vars());
        assert_eq!(actions, vec![Action::Start("a".into())]);
    }

    #[test]
    fn independent_roots_start_together() {
        let plan = plan("    - {id: a, type: delay}\n    - {id: b, type: delay}\n");
        let actions = decide(&plan, &states(&[("a", Pending), ("b", Pending)]), &vars());
        assert_eq!(
            actions,
            vec![Action::Start("a".into()), Action::Start("b".into())]
        );
    }

    #[test]
    fn join_waits_for_every_dependency() {
        let plan = plan(
            "    - {id: a, type: delay}\n    - {id: b, type: delay}\n    - {id: c, type: delay, needs: [a, b]}\n",
        );
        let waiting = states(&[("a", Succeeded), ("b", Running), ("c", Pending)]);
        assert!(decide(&plan, &waiting, &vars()).is_empty());
        let ready = states(&[("a", Succeeded), ("b", Succeeded), ("c", Pending)]);
        assert_eq!(
            decide(&plan, &ready, &vars()),
            vec![Action::Start("c".into())]
        );
    }

    #[test]
    fn running_and_finished_steps_are_never_restarted() {
        let plan = plan("    - {id: a, type: delay}\n");
        assert!(decide(&plan, &states(&[("a", Running)]), &vars()).is_empty());
        assert!(decide(&plan, &states(&[("a", Succeeded)]), &vars()).is_empty());
    }

    #[test]
    fn false_condition_skips_the_step() {
        let plan = plan("    - {id: a, type: delay, if: 'event.payload.ok == false'}\n");
        let actions = decide(&plan, &states(&[("a", Pending)]), &vars());
        assert_eq!(
            actions,
            vec![Action::Skip("a".into(), "condition fausse".into())]
        );
    }

    #[test]
    fn true_condition_starts_the_step() {
        let plan = plan("    - {id: a, type: delay, if: 'event.payload.ok'}\n");
        let actions = decide(&plan, &states(&[("a", Pending)]), &vars());
        assert_eq!(actions, vec![Action::Start("a".into())]);
    }

    #[test]
    fn condition_can_read_a_finished_dependency() {
        let plan = plan(
            "    - {id: check, type: condition}\n    - {id: b, type: delay, needs: [check], \
             if: 'steps.check.result == true'}\n",
        );
        let mut vars = vars();
        vars["steps"] = json!({ "check": { "result": true } });
        let actions = decide(
            &plan,
            &states(&[("check", Succeeded), ("b", Pending)]),
            &vars,
        );
        assert_eq!(actions, vec![Action::Start("b".into())]);
    }

    #[test]
    fn condition_error_fails_the_step() {
        let plan = plan("    - {id: a, type: delay, if: 'nope.field == 1'}\n");
        let actions = decide(&plan, &states(&[("a", Pending)]), &vars());
        assert!(matches!(actions.as_slice(), [Action::Fail(id, _)] if id == "a"));
    }

    #[test]
    fn failed_or_skipped_dependency_skips_dependents() {
        let plan = plan("    - {id: a, type: delay}\n    - {id: b, type: delay, needs: [a]}\n");
        let failed = decide(&plan, &states(&[("a", Failed), ("b", Pending)]), &vars());
        assert_eq!(
            failed,
            vec![Action::Skip("b".into(), "dépendance non réussie".into())]
        );
        let skipped = decide(&plan, &states(&[("a", Skipped), ("b", Pending)]), &vars());
        assert_eq!(
            skipped,
            vec![Action::Skip("b".into(), "dépendance non réussie".into())]
        );
    }

    #[test]
    fn independent_branch_keeps_running_after_a_failure() {
        let plan = plan(
            "    - {id: a, type: delay}\n    - {id: b, type: delay, needs: [a]}\n    - {id: c, type: delay}\n",
        );
        let actions = decide(
            &plan,
            &states(&[("a", Failed), ("b", Pending), ("c", Pending)]),
            &vars(),
        );
        assert_eq!(
            actions,
            vec![
                Action::Skip("b".into(), "dépendance non réussie".into()),
                Action::Start("c".into())
            ]
        );
    }

    #[test]
    fn outcome_is_none_while_work_remains() {
        assert_eq!(outcome(&states(&[("a", Succeeded), ("b", Pending)])), None);
        assert_eq!(outcome(&states(&[("a", Running)])), None);
    }

    #[test]
    fn outcome_summarises_the_finished_run() {
        assert_eq!(
            outcome(&states(&[("a", Succeeded), ("b", Skipped)])),
            Some(RunOutcome::Succeeded)
        );
        assert_eq!(
            outcome(&states(&[("a", Succeeded), ("b", Failed)])),
            Some(RunOutcome::Failed)
        );
        assert_eq!(
            outcome(&states(&[("a", StepState::Cancelled), ("b", Succeeded)])),
            Some(RunOutcome::Cancelled)
        );
        assert_eq!(
            outcome(&states(&[("a", StepState::Cancelled), ("b", Failed)])),
            Some(RunOutcome::Failed)
        );
    }

    #[test]
    fn waiting_step_keeps_the_run_open() {
        assert_eq!(outcome(&states(&[("a", StepState::Waiting)])), None);
    }
}
