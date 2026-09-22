use std::sync::Arc;

use serde_json::Value;
use yi_types::plan::contract::{Decider, ItemVerdict, Outcome as ContractOutcome, Verdict};
use yi_types::plan::doc::{PlanId, PlanState, Todo, TodoAddr, TodoLabel, TodoState};
use yi_types::plan::ledger::AttemptId;
use yi_types::plan::op::Choice;
use yi_types::subagent::{ChildExit, ChildResult, FailClass};
use yi_types::url::Url;

use super::acceptance::{Phase, is_worktree, phase_of};
use super::ops::{Actor, Op, OpRequest, PlanEngine, PlanOpError, agent_url};
use super::state::{SUBMITTED_KEY, reduce, root_of};
use crate::goal::DeliverFn;
use crate::subagent::{FinishFn, SubagentHost, last_assistant_text};

const CAUSE_CHARS: usize = 500;

struct Held {
    plan: PlanId,
    label: TodoLabel,
    attempt: AttemptId,
    phase: Option<Phase>,
    early: Option<Url>,
    json: bool,
}

fn needs_json(todo: &Todo) -> bool {
    let declared = todo
        .delegation
        .as_ref()
        .is_some_and(|delegation| delegation.output.is_some());
    let schema_item = todo.contract.as_ref().is_some_and(|contract| {
        contract
            .items
            .iter()
            .any(|item| matches!(item.decider, Decider::Schema { .. }))
    });
    declared || schema_item
}

fn clip(text: String) -> String {
    if text.chars().count() <= CAUSE_CHARS {
        return text;
    }
    text.chars().take(CAUSE_CHARS).collect()
}

fn failed_items(verdict: &Verdict) -> String {
    let items: Vec<String> = verdict
        .items
        .iter()
        .filter_map(|line| match &line.verdict {
            ItemVerdict::Fail { detail } => Some(format!("{}: {detail}", line.id)),
            _ => None,
        })
        .collect();
    if items.is_empty() {
        return "the verdict was fail".to_owned();
    }
    clip(items.join("; "))
}

impl PlanEngine {
    /// The todo `agent` runs in an active plan; `None` means a child `rlm.run` spawned.
    fn locate(&self, agent: &str) -> Option<Held> {
        let roots = match agent.split_once('/') {
            Some((plan, _)) => vec![root_of(&PlanId::new(plan).ok()?).ok()?],
            None => self.store.roots().ok()?,
        };
        for root in roots {
            let Ok(reading) = self.store.journal(&root).read() else {
                continue;
            };
            let Ok(state) = reduce(&reading.records) else {
                continue;
            };
            for (id, plan) in &state.plans {
                if plan.state != PlanState::Active {
                    continue;
                }
                let Some(todo) = plan.todos.iter().find(|todo| {
                    todo.delegation.is_some()
                        && matches!(&todo.state, TodoState::Running { by } if by.as_str() == agent)
                }) else {
                    continue;
                };
                return Some(Held {
                    plan: id.clone(),
                    label: todo.label.clone(),
                    attempt: todo.attempt,
                    phase: is_worktree(todo)
                        .then(|| phase_of(&reading.records, id, &todo.label, todo.attempt)),
                    early: todo
                        .extra
                        .get(SUBMITTED_KEY)
                        .and_then(Value::as_str)
                        .and_then(|url| url.parse().ok()),
                    json: needs_json(todo),
                });
            }
        }
        None
    }

    fn step(&self, plan: &PlanId, op: Op) -> Result<(), PlanOpError> {
        self.apply(OpRequest {
            plan: Some(plan.clone()),
            actor: Actor::Engine,
            op,
            request_id: None,
            expected_revision: None,
        })
        .map(drop)
    }

    fn store_product(&self, held: &Held, text: &str) -> Result<Url, PlanOpError> {
        let unstored = |reason: String| PlanOpError::Verification {
            label: held.label.clone(),
            reason,
        };
        let value = held
            .json
            .then(|| crate::schema::extract(text).ok())
            .flatten();
        let (bytes, media_type) = match value {
            Some(value) => (serde_json::to_string(&value)?, "application/json"),
            None => (text.to_owned(), "text/plain"),
        };
        let stored = self
            .store
            .artifacts(&held.plan)
            .put(bytes.as_bytes(), media_type, &self.store.nonce())
            .map_err(|error| unstored(error.to_string()))?;
        format!("plan://{}/artifacts/{}", held.plan, stored.digest.hex())
            .parse()
            .map_err(|error| unstored(format!("{error}")))
    }

    fn accept_finish(&self, held: &Held, text: &str) -> Result<(), PlanOpError> {
        let unsubmitted = match held.phase {
            Some(phase) => phase == Phase::Unsubmitted,
            None => held.early.is_none(),
        };
        let output = if unsubmitted {
            let url = self.store_product(held, text)?;
            self.step(
                &held.plan,
                Op::Submit {
                    label: held.label.clone(),
                    attempt: held.attempt,
                    output: url.clone(),
                },
            )?;
            Some(url)
        } else {
            held.early.clone()
        };
        self.step(
            &held.plan,
            Op::Done {
                label: held.label.clone(),
                output: output.filter(|_| held.phase.is_none()),
            },
        )
    }

    fn fail_finish(&self, held: &Held, cause: String) -> Result<(), PlanOpError> {
        self.step(
            &held.plan,
            Op::Fail {
                label: held.label.clone(),
                cause,
                disposition: held.phase.map(|_| Choice::Retained),
            },
        )
    }

    fn left_notice(&self, held: &Held, refusal: &PlanOpError) -> Option<String> {
        if matches!(
            refusal,
            PlanOpError::IllegalStep { .. } | PlanOpError::NotRunningBy { .. }
        ) {
            return None;
        }
        let label = held.label.as_str();
        let plan = self.store.read(&held.plan).ok();
        Some(
            match plan.as_ref().and_then(|plan| plan.todo(&held.label)) {
                Some(Todo {
                    state: TodoState::Blocked { note, .. },
                    ..
                }) => format!("plan: {label:?} waits on you: {note}"),
                _ => format!("plan: not accepted {label:?}, still running: {refusal}"),
            },
        )
    }

    pub fn finish_child(
        &self,
        agent: &str,
        exit: ChildExit,
        error: Option<String>,
        product: Option<String>,
    ) -> Option<String> {
        let held = self.locate(agent)?;
        let label = held.label.as_str();
        if exit != ChildExit::Completed {
            let cause =
                clip(error.unwrap_or_else(|| crate::family::read_exit(Some(exit)).verb.to_owned()));
            return Some(match self.fail_finish(&held, cause.clone()) {
                Ok(()) => format!("plan: failed {label:?}: {cause}"),
                Err(refused) => {
                    format!("plan: {label:?} ended ({cause}); its fail was refused: {refused}")
                }
            });
        }
        let refusal = match self.accept_finish(&held, product.as_deref().unwrap_or_default()) {
            Ok(()) => {
                let addr = TodoAddr {
                    plan: held.plan.clone(),
                    todo: held.label.clone(),
                };
                let url = agent_url(&addr).map_or_else(|_| agent.to_owned(), |url| url.to_string());
                return Some(format!("plan: accepted {label:?} ({url})"));
            }
            Err(refusal) => refusal,
        };
        let PlanOpError::Refused { verdict, .. } = &refusal else {
            return self.left_notice(&held, &refusal);
        };
        if verdict.outcome != ContractOutcome::Fail {
            return self.left_notice(&held, &refusal);
        }
        let items = failed_items(verdict);
        Some(
            match self.fail_finish(&held, format!("contract refused: {items}")) {
                Ok(()) => format!("plan: refused {label:?}: {items}"),
                Err(error) => {
                    format!("plan: refused {label:?}: {items}; its fail was refused: {error}")
                }
            },
        )
    }
}

impl SubagentHost {
    pub fn set_finished(&self, hook: Arc<FinishFn>) {
        if let Ok(mut slot) = self.finished.lock() {
            *slot = Some(hook);
        }
    }

    pub(crate) fn finish_taken(&self, name: &str, exit: ChildExit, error: Option<String>) -> bool {
        let hook = self.finished.lock().ok().and_then(|slot| slot.clone());
        hook.is_some_and(|hook| hook(name.to_owned(), exit, error))
    }

    fn held_back(&self, name: &str) -> Option<String> {
        let check = {
            let children = self.children.lock().ok()?;
            let key = Self::key_of(&children, name).ok()?;
            children.get(&key)?.check.clone()?
        };
        if let Err(evidence) = crate::goal::run_check(&check, crate::goal::DEFAULT_CHECK_TIMEOUT_MS)
        {
            return Some(format!("its check is red: {evidence}"));
        }
        let answer = self.answer_of(name)?;
        let result = serde_json::from_str::<ChildResult>(answer.trim()).ok()?;
        self.route_discoveries(name, &result.discoveries)
            .err()
            .map(|error| format!("its discoveries were held back: {error}"))
    }

    fn answer_of(&self, name: &str) -> Option<String> {
        let children = self.children.lock().ok()?;
        let key = Self::key_of(&children, name).ok()?;
        last_assistant_text(&children.get(&key)?.session.messages())
    }
}

/// Invariant: the hook holds the host and engine weakly, since the engine holds the host.
pub fn install(host: &Arc<SubagentHost>, engine: &Arc<PlanEngine>, deliver: DeliverFn) {
    let (weak_host, weak_engine) = (Arc::downgrade(host), Arc::downgrade(engine));
    host.set_finished(Arc::new(move |agent, exit, error| {
        let (Some(host), Some(engine)) = (weak_host.upgrade(), weak_engine.upgrade()) else {
            return false;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        // Invariant: a plan child is named `<plan>/<todo>`, so any other exit reads no journal.
        if !agent.contains('/') || engine.locate(&agent).is_none() {
            return false;
        }
        let deliver = Arc::clone(&deliver);
        drop(runtime.spawn_blocking(move || {
            let held = (exit == ChildExit::Completed)
                .then(|| host.held_back(&agent))
                .flatten();
            let (exit, error) = match held {
                Some(reason) => (
                    ChildExit::Failed {
                        class: FailClass::RedCheck,
                    },
                    Some(reason),
                ),
                None => (exit, error),
            };
            let product = host.answer_of(&agent);
            if let Some(line) = engine.finish_child(&agent, exit, error, product) {
                super::dispatch::say(&deliver, line);
            }
        }));
        true
    }));
}
