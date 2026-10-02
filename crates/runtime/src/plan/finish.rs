use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use yi_types::plan::contract::{Decider, ItemId, ItemVerdict, Outcome as ContractOutcome, Verdict};
use yi_types::plan::doc::{PlanId, PlanState, Todo, TodoAddr, TodoLabel, TodoState};
use yi_types::plan::ledger::{AttemptId, RequestId};
use yi_types::plan::op::Choice;
use yi_types::subagent::{ChildExit, ChildResult, FailClass};
use yi_types::url::Url;

use super::acceptance::{Phase, is_worktree, phase_of};
use super::ops::{Actor, ENGINE_AGENT, Op, OpRequest, Outcome, PlanEngine, PlanOpError, agent_url};
use super::state::{KIND_LEFT, SUBMITTED_KEY, reduce, root_of};
use crate::goal::DeliverFn;
use crate::subagent::{FinishFn, SubagentHost};

const CAUSE_CHARS: usize = 500;

struct Held {
    plan: PlanId,
    label: TodoLabel,
    attempt: AttemptId,
    phase: Option<Phase>,
    early: Option<Url>,
    json: bool,
    verified: bool,
    schemas: Vec<ItemId>,
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
    fn names_plan(&self, agent: &str) -> bool {
        let root = agent
            .split_once('/')
            .and_then(|(plan, _)| PlanId::new(plan).ok());
        let root = root.and_then(|plan| root_of(&plan).ok());
        root.is_some_and(|root| self.store.journal_path(&root).is_file())
    }

    /// The todo `agent` runs in an active plan; `None` means a child `rlm.run` spawned.
    fn locate(&self, agent: &str) -> Option<Held> {
        let roots = match agent.split_once('/') {
            Some((plan, _)) => vec![root_of(&PlanId::new(plan).ok()?).ok()?],
            None => self.roots().ok()?,
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
                    verified: is_worktree(todo) || todo.contract.is_some(),
                    schemas: todo
                        .contract
                        .iter()
                        .flat_map(|contract| &contract.items)
                        .filter(|item| matches!(item.decider, Decider::Schema { .. }))
                        .map(|item| item.id.clone())
                        .collect(),
                });
            }
        }
        None
    }

    fn step(&self, plan: &PlanId, op: Op) -> Result<Outcome, PlanOpError> {
        self.apply(OpRequest {
            plan: Some(plan.clone()),
            actor: Actor::Engine,
            op,
            request_id: None,
            expected_revision: None,
        })
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
        .map(drop)
    }

    /// Incident: in all eight G5 trials the owner re-ran the check the engine had just passed.
    fn accepted_evidence(&self, held: &Held) -> String {
        let reading = root_of(&held.plan).map(|root| self.store.journal(&root).read());
        let records = match reading {
            Ok(Ok(reading)) => reading.records,
            _ => Vec::new(),
        };
        let verdict = records
            .iter()
            .rev()
            .filter(|record| record.record.plan == held.plan)
            .filter(|record| record.record.todo.as_ref() == Some(&held.label))
            .find_map(|record| serde_json::from_value::<Verdict>(record.verdict.clone()?).ok())
            .filter(|verdict| verdict.outcome == ContractOutcome::Pass);
        let Some(verdict) = verdict else {
            return String::new();
        };
        let (passed, short): (Vec<_>, Vec<_>) = verdict
            .items
            .iter()
            .partition(|line| line.verdict == ItemVerdict::Pass);
        let lines: String = passed
            .iter()
            .map(|line| match &line.evidence {
                Some(said) => format!("\n- {} pass: {said}", line.id),
                None => format!("\n- {} pass", line.id),
            })
            .collect();
        let short: Vec<String> = short.iter().map(|line| line.id.to_string()).collect();
        let short = match short.as_slice() {
            [] => String::new(),
            ids => format!("\nNot passed, and not covered by this: {}.", ids.join(", ")),
        };
        format!(
            ". The engine ran these checks and they passed, so do not run them again:{lines}{short}"
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
        .map(drop)
    }

    fn fail_and_retry(&self, held: &Held, said: String, cause: String, retry: bool) -> String {
        if let Err(error) = self.fail_finish(held, cause) {
            self.leave(held, &error, &format!("its fail was refused: {error}"));
            return format!("{said}; its fail was refused: {error}");
        }
        if !retry || self.retried(held) {
            return said;
        }
        let again = Op::Retry {
            label: held.label.clone(),
            delegation: None,
        };
        let unstarted = format!("the engine could not start {:?}", held.label.as_str());
        match self.step(&held.plan, again) {
            Ok(out) => match out.notices.iter().find(|line| line.starts_with(&unstarted)) {
                Some(notice) => format!("{said}; the engine retried it, and {notice}"),
                None => format!("{said}; the engine retried it once with that in the brief"),
            },
            Err(error) => format!("{said}; the engine's retry was refused: {error}"),
        }
    }

    fn retried(&self, held: &Held) -> bool {
        let Some(reading) = root_of(&held.plan)
            .ok()
            .and_then(|root| self.store.journal(&root).read().ok())
        else {
            return true;
        };
        reading.records.iter().any(|record| {
            record.record.op == "retry"
                && record.record.actor == ENGINE_AGENT
                && record.record.plan == held.plan
                && record.record.todo.as_ref() == Some(&held.label)
        })
    }

    fn leave(&self, held: &Held, refusal: &PlanOpError, detail: &str) -> bool {
        if matches!(
            refusal,
            PlanOpError::IllegalStep { .. } | PlanOpError::NotRunningBy { .. }
        ) {
            return false;
        }
        let journaled = || -> Option<()> {
            let request = RequestId::new(format!("left-{}", self.store.request_nonce())).ok()?;
            let _lease = self.lease_waiting().ok()?;
            let mut txn = self
                .begin(
                    &root_of(&held.plan).ok()?,
                    &Actor::Engine,
                    request,
                    None,
                    false,
                )
                .ok()?;
            let args = json!({"label": held.label, "detail": detail});
            let who = (&held.label, held.attempt);
            self.record(&mut txn, &held.plan, who, KIND_LEFT, args, None)
                .ok()?;
            self.store.checkpoint_family(&txn.state).ok().map(drop)
        };
        let _the_notice_still_names_it = journaled();
        true
    }

    fn left_notice(&self, held: &Held, refusal: &PlanOpError) -> String {
        let detail = match refusal {
            PlanOpError::Refused { verdict, .. } => format!("its verdict was {}", verdict.outcome),
            other => other.to_string(),
        };
        let label = held.label.as_str();
        if !self.leave(held, refusal, &detail) {
            return format!("plan: {label:?} ended, and another op moved it first: {refusal}");
        }
        let plan = self.store.read(&held.plan).ok();
        match plan.as_ref().and_then(|plan| plan.todo(&held.label)) {
            Some(Todo {
                state: TodoState::Blocked { note, .. },
                ..
            }) => format!("plan: {label:?} waits on you: {note}"),
            _ => format!("plan: not accepted {label:?}, still running: {refusal}"),
        }
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
                    self.leave(&held, &refused, &format!("its fail was refused: {refused}"));
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
                let evidence = self.accepted_evidence(&held);
                return Some(format!("plan: accepted {label:?} ({url}){evidence}"));
            }
            Err(refusal) => refusal,
        };
        if let PlanOpError::MergeFailed { paths, .. } = &refusal {
            let cause = format!(
                "its candidate conflicts with the parent at {}: the next attempt starts from the parent's current tree",
                paths.join(", ")
            );
            let said = format!("plan: failed {label:?}: {cause}");
            return Some(self.fail_and_retry(&held, said, cause, true));
        }
        let PlanOpError::Refused { verdict, .. } = &refusal else {
            return Some(self.left_notice(&held, &refusal));
        };
        if verdict.outcome != ContractOutcome::Fail {
            return Some(self.left_notice(&held, &refusal));
        }
        let items = failed_items(verdict);
        let unparsed = verdict.items.iter().any(|line| {
            matches!(line.verdict, ItemVerdict::Fail { .. }) && held.schemas.contains(&line.id)
        });
        let said = format!("plan: refused {label:?}: {items}");
        Some(self.fail_and_retry(&held, said, format!("contract refused: {items}"), unparsed))
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

    fn held_back(&self, name: &str, verified: bool) -> Option<String> {
        let check = {
            let children = self.children.lock().ok()?;
            let key = Self::key_of(&children, name).ok()?;
            children.get(&key)?.check.clone()?
        };
        let timeout = crate::goal::DEFAULT_CHECK_TIMEOUT_MS;
        let cwd = self
            .cwd_of(name)
            .unwrap_or_else(|| self.options.cwd.clone());
        let red = (!verified)
            .then(|| crate::goal::run_check(&check, &cwd, timeout).err())
            .flatten();
        if let Some(evidence) = red {
            return Some(format!("its check is red: {evidence}"));
        }
        let answer = Self::json_answer(&self.answer_of(name)?)?;
        let result = serde_json::from_value::<ChildResult>(answer).ok()?;
        self.route_discoveries(name, &result.discoveries)
            .err()
            .map(|error| format!("its discoveries were held back: {error}"))
    }

    fn settled(&self, name: &str) {
        if let Ok(mut children) = self.children.lock()
            && let Ok(key) = Self::key_of(&children, name)
        {
            children.touch(&key, crate::family::Cause::Settled);
        }
    }

    pub fn busy(&self) -> bool {
        let live = self
            .children
            .lock()
            .map(|children| children.values().any(|child| child.exit.is_none()));
        live.unwrap_or(false) || self.settling.load(Ordering::SeqCst) > 0
    }

    /// A finish settling, or any child still moving or asking.
    pub fn holds_owner(&self) -> bool {
        self.settling.load(Ordering::SeqCst) > 0
            || self
                .states()
                .iter()
                .any(|view| holds(view, self.in_tool(&view.name)))
    }

    fn in_tool(&self, name: &str) -> bool {
        self.children.lock().is_ok_and(|children| {
            children.values().any(|record| {
                record.session_name == name
                    && record.activity == yi_types::subagent::ChildActivity::Executing
                    && record.session.status() == crate::session::Status::Running
            })
        })
    }

    /// Held, a reap of `name` leaves its block here to ride the verdict; released, it comes back.
    fn hold_harvest(&self, name: &str, hold: bool) -> Option<String> {
        let mut held = self.harvests.lock().ok()?;
        if hold {
            held.insert(name.to_owned(), None);
            return None;
        }
        held.remove(name).flatten()
    }

    fn answer_of(&self, name: &str) -> Option<String> {
        let children = self.children.lock().ok()?;
        let key = Self::key_of(&children, name).ok()?;
        crate::subagent::answer_text(&children.get(&key)?.session.messages())
    }
}

pub(crate) struct Settling(Arc<SubagentHost>);

impl Settling {
    pub(crate) fn hold(host: &Arc<SubagentHost>) -> Self {
        host.settling.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(host))
    }
}

impl Drop for Settling {
    /// The last release is when `busy` can fall, so a family wait reads it without a timer.
    fn drop(&mut self) {
        self.0.settling.fetch_sub(1, Ordering::SeqCst);
        if let Ok(children) = self.0.children.lock() {
            children.stirred.notify_waiters();
        }
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
        if !engine.names_plan(&agent) {
            return false;
        }
        let deliver = Arc::clone(&deliver);
        let settling = Settling::hold(&host);
        drop(runtime.spawn_blocking(move || {
            // Invariant: a start commits Running after its spawn, so an early end waits out its lease.
            let held = engine.locate(&agent).or_else(|| {
                drop(engine.lease_waiting().ok()?);
                engine.locate(&agent)
            });
            let verb = crate::family::read_exit(Some(exit)).verb;
            let unheld = format!("plan: {agent:?} {verb}, and no todo runs by it");
            let Some(held) = held else {
                super::dispatch::say(&deliver, unheld);
                return;
            };
            let held = (exit == ChildExit::Completed)
                .then(|| host.held_back(&agent, held.verified))
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
            host.hold_harvest(&agent, true);
            let line = engine.finish_child(&agent, exit, error, product);
            let said = line.unwrap_or(unheld);
            match host.hold_harvest(&agent, false) {
                Some(block) => super::dispatch::say(&deliver, format!("{said}\n{block}")),
                None => super::dispatch::say(&deliver, said),
            }
            host.settled(&agent);
            drop(settling);
        }));
        true
    }));
}

/// Invariant: a stuck child holds nothing unless only the idle clock stalled it mid tool call.
fn holds(view: &crate::family::MemberView, in_tool: bool) -> bool {
    use crate::family::MemberState;
    let idle = view
        .note
        .as_deref()
        .is_some_and(|note| note.starts_with("idle "));
    match view.state {
        MemberState::Running | MemberState::Queued | MemberState::NeedsYou => true,
        MemberState::Stuck => idle && in_tool,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::dispatch::tests::{
        delegated, hooked, owner, reply, settled, stirred_until, texts,
    };
    use super::super::ops::Op;
    use super::super::ops::PlanEngine;
    use super::install;
    use yi_types::plan::doc::{Check, GoalText, PlanState, TodoState};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Dies with the accept command run in the process's own cwd: the marker is only in the
    /// checkout the child worked in, so the todo fails "its check is red".
    #[tokio::test]
    async fn an_accept_command_runs_in_the_childs_checkout() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut")])?;
        std::fs::write(rig.cwd.join("seam.marker"), "cut")?;
        let mut spec = delegated("cut the seam")?;
        if let Some(delegation) = spec.delegation.as_mut() {
            delegation.accept = Check::Command("test -f seam.marker".to_owned());
        }
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![spec],
        }))?;
        let todo = settled(&rig, &out.plan.id, "cut the seam").await?;
        assert!(
            matches!(todo.state, TodoState::Done { .. }),
            "{:?}",
            todo.state
        );
        Ok(())
    }

    /// Dies with the accept command run beside a contract: the contract passes, the red accept
    /// command fails the todo first, and the verifier never decides.
    #[tokio::test]
    async fn a_contracted_delegations_accept_command_is_not_run() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut")])?;
        let args = serde_json::json!({"op": "init", "goal": "ship the widget", "todos": [{
            "label": "cut the seam",
            "delegation": {"spec": {"role": "worker"}, "accept": {"command": "false"}},
            "contract": {"class": "inline", "items": [
                {"id": "ok", "critical": true, "weight": 1, "decider": {"cmd": "true"}}
            ]},
        }]});
        let args = args.as_object().cloned().ok_or("args")?;
        let (request, blobs) =
            super::super::tool::declared(&super::super::ops::Actor::Owner, &args)?;
        let out = rig.engine.apply_with(request, &blobs)?;
        let todo = settled(&rig, &out.plan.id, "cut the seam").await?;
        assert!(
            matches!(todo.state, TodoState::Done { .. }),
            "{:?} {:?}",
            todo.state,
            texts(&rig.said)
        );
        Ok(())
    }

    /// Dies with a bare `plan: accepted` (G5: 8 of 8 owners re-ran it), or an unnamed cut, or
    /// a failed item the threshold let through listed among the checks not to run again.
    #[tokio::test]
    async fn an_accepted_notice_quotes_each_check_the_engine_passed() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut")])?;
        let long = "head -c 1000 /dev/zero | tr '\\0' x";
        let args = serde_json::json!({"op": "init", "goal": "ship the widget", "todos": [{
            "label": "cut the seam",
            "delegation": {"spec": {"role": "worker"}, "accept": {"command": "true"}},
            "contract": {"class": "inline", "threshold": 600, "items": [
                {"id": "tests", "critical": true, "weight": 1, "decider": {"cmd": "echo 12 passed"}},
                {"id": "long", "critical": true, "weight": 1, "decider": {"cmd": long}},
                {"id": "lint", "critical": false, "weight": 1, "decider": {"cmd": "echo lint broke; exit 1"}}
            ]},
        }]});
        let args = args.as_object().cloned().ok_or("args")?;
        let (request, blobs) =
            super::super::tool::declared(&super::super::ops::Actor::Owner, &args)?;
        let out = rig.engine.apply_with(request, &blobs)?;
        let todo = settled(&rig, &out.plan.id, "cut the seam").await?;
        assert!(
            matches!(todo.state, TodoState::Done { .. }),
            "{:?}",
            todo.state
        );
        let said = texts(&rig.said).join("\n");
        assert!(said.contains("do not run them again"), "{said}");
        assert!(
            said.contains("- tests pass: `echo 12 passed` exit 0\n12 passed"),
            "{said}"
        );
        assert!(said.contains("[… last 300 of 1000 chars"), "{said}");
        assert!(!said.contains("lint broke"), "{said}");
        assert!(said.contains("not covered by this: lint."), "{said}");
        Ok(())
    }

    struct Own(super::super::store::PlanStore);

    impl super::super::output::OutputResolve for Own {
        fn resolve(&self, url: &yi_types::url::Url) -> Result<Option<String>, String> {
            let text = url.to_string();
            let (plan, hex) = text
                .strip_prefix("plan://")
                .and_then(|rest| rest.split_once("/artifacts/"))
                .ok_or("not a plan artifact")?;
            let digest = yi_types::plan::canonical::Digest::parse(&format!("sha256:{hex}"))
                .map_err(|error| error.to_string())?;
            let plan = yi_types::plan::doc::PlanId::new(plan).map_err(|error| error.to_string())?;
            let bytes = self
                .0
                .artifacts(&plan)
                .get(&digest)
                .map_err(|error| error.to_string())?;
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        }
    }

    /// Dies with a schema refusal failing for good: ten final-confirmation readers answered prose.
    #[tokio::test]
    async fn a_prose_answer_to_a_schema_is_retried_once_with_the_schema_and_the_error() -> TestResult
    {
        let mut rig = hooked(vec![reply("I read every file; the findings are above.")])?;
        let store = super::super::store::PlanStore::open(rig.cwd.join("read"))?;
        let delegate = super::super::dispatch::SessionDelegate::new(
            std::sync::Arc::clone(&rig.host),
            std::sync::Arc::new(|_message, _mode| {}),
            std::sync::Arc::new(crate::fetch::FetchLog::new()),
        );
        rig.engine = std::sync::Arc::new(
            PlanEngine::new(store.clone(), std::sync::Arc::new(delegate))
                .with_cwd(rig.cwd.to_path_buf())
                .with_output_resolve(std::sync::Arc::new(Own(store))),
        );
        let said = std::sync::Arc::clone(&rig.said);
        install(
            &rig.host,
            &rig.engine,
            std::sync::Arc::new(move |message, _mode| {
                if let Ok(mut said) = said.lock() {
                    said.push(message);
                }
            }),
        );
        let schema = serde_json::json!({"type": "object", "required": ["findings"]});
        let id = yi_types::plan::doc::PlanId::slug("read the archive")?;
        let stored = rig.engine.store().artifacts(&id).put(
            serde_json::to_string(&schema)?.as_bytes(),
            "application/schema+json",
            &rig.engine.store().nonce(),
        )?;
        let item = serde_json::json!({"id": "schema1", "critical": true, "weight": 1,
            "decider": {"schema": {"schema": stored}}});
        let args = serde_json::json!({"op": "init", "goal": "read the archive", "todos": [{
            "label": "read board",
            "delegation": {"spec": {"role": "reader"}, "accept": {"stated": "the findings"}},
            "contract": {"class": "reader", "items": [item]},
        }]});
        let args = args.as_object().cloned().ok_or("args")?;
        let (request, blobs) =
            super::super::tool::declared(&super::super::ops::Actor::Owner, &args)?;
        rig.engine.apply_with(request, &blobs)?;
        let label = yi_types::plan::doc::TodoLabel::new("read board")?;
        let mut last = None;
        stirred_until(&rig.host, || {
            let todo = rig.engine.store().read(&id)?.todo(&label).cloned();
            if let Some(todo) = todo.filter(|todo| todo.attempt.get() == 2)
                && let TodoState::Failed { last: trace, .. } = &todo.state
            {
                last = trace.clone();
                return Ok(true);
            }
            Ok(false)
        })
        .await?;
        let said = texts(&rig.said);
        assert!(
            said.iter().any(|line| line.contains("retried it once")),
            "{said:?}"
        );
        let now = rig.engine.store().read(&id)?.todo(&label).cloned();
        let agent = last.ok_or(format!("the second attempt never failed: {now:?} {said:?}"))?;
        let kept = rig
            .host
            .kept_transcript(agent.to_string().trim_start_matches("history://"))
            .ok_or("no kept transcript")?;
        let kept = yi_session::lock_session(&kept);
        for told in [
            "Answer with only a JSON value",
            "Schema: {",
            "previous attempt failed: contract refused: schema1: answer contains no JSON value",
        ] {
            assert!(!kept.grep(told, 1).is_empty(), "the brief lacks {told:?}");
        }
        Ok(())
    }

    /// Dies with the hook's lease wait on the child's runtime thread: an early end parks it 10 s.
    #[tokio::test]
    async fn an_early_ends_lease_wait_leaves_the_runtime_running() -> TestResult {
        let rig = hooked(vec![reply("done")])?;
        let mut inline = delegated("write the notes")?;
        inline.delegation = None;
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![inline],
        }))?;
        let lease = rig.engine.store().lease()?;
        let mut kwargs = serde_json::Map::new();
        let name = format!("{}/stray", out.plan.id);
        kwargs.insert("name".to_owned(), serde_json::Value::String(name));
        kwargs.insert("role".to_owned(), serde_json::Value::from("root"));
        rig.host.spawn("work".to_owned(), kwargs)?;
        let started = std::time::Instant::now();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let parked = started.elapsed();
        drop(lease);
        assert!(parked < std::time::Duration::from_secs(2), "{parked:?}");
        Ok(())
    }

    /// Dies with open plans read alone: the engine closed the plan and `done` read "no plan is open".
    #[tokio::test]
    async fn an_owner_done_after_the_engine_closed_the_plan_names_the_acceptance() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut")])?;
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        settled(&rig, &out.plan.id, "cut the seam").await?;
        let label = yi_types::plan::doc::TodoLabel::new("cut the seam")?;
        stirred_until(&rig.host, || {
            Ok(rig.engine.store().read(&out.plan.id)?.state != PlanState::Active)
        })
        .await?;
        let said = rig.engine.apply(owner(Op::Done {
            label,
            output: None,
        }))?;
        assert!(
            said.notices[0].starts_with("nothing to do: the engine accepts delegated todos")
                && said.notices[0].contains("accepted"),
            "{:?}",
            said.notices
        );
        Ok(())
    }

    struct Once(std::sync::atomic::AtomicBool);

    impl super::super::ops::Delegate for Once {
        fn spawn(
            &self,
            _at: &yi_types::plan::doc::TodoAddr,
            _delegation: &yi_types::plan::doc::Delegation,
        ) -> Result<yi_types::plan::doc::AgentId, String> {
            if self.0.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return Err("RLM child limit reached".to_owned());
            }
            yi_types::plan::doc::AgentId::new("child-0").map_err(|error| error.to_string())
        }

        fn reap(
            &self,
            _agent: &yi_types::plan::doc::AgentId,
            _supplied: &[yi_types::url::Url],
        ) -> Result<Option<yi_types::url::Url>, String> {
            Ok(None)
        }
    }

    /// Dies with the retry's outcome dropped: a refused start still read "retried it once".
    #[test]
    fn a_retry_the_engine_could_not_start_says_so() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-finish-unstarted")?;
        let store = super::super::store::PlanStore::open(dir.to_path_buf())?;
        let once = std::sync::Arc::new(Once(std::sync::atomic::AtomicBool::new(false)));
        let engine = PlanEngine::new(store, once);
        engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        let held = engine.locate("child-0").ok_or("child-0 runs nothing")?;
        let said = engine.fail_and_retry(&held, "plan: failed".to_owned(), "x".to_owned(), true);
        assert!(!said.contains("retried it once"), "{said}");
        assert!(
            said.contains("could not start \"cut the seam\"") && said.contains("limit reached"),
            "{said}"
        );
        Ok(())
    }

    /// Dies with the answer parsed as bare JSON: a fenced result's discoveries are dropped.
    #[tokio::test]
    async fn a_fenced_answer_still_routes_its_discoveries() -> TestResult {
        let answer = "```json\n{\"value\": 1, \"discoveries\": [{\"text\": \"the ledger drifts\", \"fingerprint\": \"f1\"}]}\n```";
        let rig = hooked(vec![reply(answer)])?;
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        settled(&rig, &out.plan.id, "cut the seam").await?;
        let reports = texts(&rig.reports);
        assert!(
            reports
                .iter()
                .any(|text| text.contains("deferred discovery")
                    && text.contains("the ledger drifts")),
            "{reports:?}"
        );
        Ok(())
    }

    /// Dies with the settle moving nothing: a finish the engine leaves running bumped no epoch,
    /// so a wait from the child's end slept out its budget while the plan had already moved.
    #[tokio::test]
    async fn a_finish_the_engine_left_running_moves_the_family() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut")])?;
        let mut spec = delegated("cut the seam")?;
        if let Some(delegation) = spec.delegation.as_mut() {
            delegation.accept = Check::Stated("it holds".to_owned());
        }
        rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![spec],
        }))?;
        let lease = rig.engine.lease_waiting()?;
        let completed = crate::subagent::ChildStatus::Completed;
        let epoch = || rig.host.children.lock().map(|children| children.epoch);
        let mut ended = None;
        stirred_until(&rig.host, || {
            let done = rig
                .host
                .children_view()
                .iter()
                .any(|child| child.update.status == completed);
            if done {
                ended = Some(epoch().map_err(|_| "poisoned")?);
            }
            Ok(done)
        })
        .await?;
        drop(lease);
        let ended = ended.ok_or("the child never ended")?;
        stirred_until(&rig.host, || {
            Ok(!texts(&rig.said).is_empty() && !rig.host.busy())
        })
        .await?;
        let said = texts(&rig.said);
        assert!(
            said.iter().any(|line| line.contains("not accepted")),
            "{said:?}"
        );
        assert!(epoch().map_err(|_| "poisoned")? > ended, "{said:?}");
        Ok(())
    }

    async fn ended(host: &std::sync::Arc<crate::SubagentHost>) -> bool {
        let finished = || {
            let states = host.states();
            Ok(states.iter().any(|view| view.state.as_str() == "finished"))
        };
        stirred_until(host, finished).await.is_ok()
    }

    /// Dies with `refold` reading only a live session: a lagged woken run never concluded.
    #[tokio::test]
    async fn a_lagged_watch_concludes_a_woken_run_it_never_saw_start() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut"), reply("cut again")])?;
        let named = serde_json::Map::from_iter([("name".to_owned(), "kid".into())]);
        rig.host.spawn("cut".to_owned(), named)?;
        assert!(ended(&rig.host).await, "child never completed");
        let key = {
            let mut children = rig.host.children.lock().map_err(|_| "poisoned")?;
            let (key, record) = children.iter_mut().next().ok_or("no record")?;
            record.session.deliver(crate::session::task("again"), true);
            record.step(crate::subagent::Step::Resumed);
            key.clone()
        };
        if rig.host.refold(&key) {
            rig.host.resume(&key);
        }
        assert!(ended(&rig.host).await, "the woken run was never concluded");
        Ok(())
    }

    /// Dies with a late `AgentStart` claiming a concluded run again: a second ending, one run.
    #[tokio::test]
    async fn a_late_start_of_a_concluded_run_concludes_nothing() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut")])?;
        let named = serde_json::Map::from_iter([("name".to_owned(), "kid".into())]);
        rig.host.spawn("cut".to_owned(), named)?;
        assert!(ended(&rig.host).await, "child never completed");
        let endings = || rig.notices.lock().map(|sink| sink.len()).unwrap_or(0);
        let before = endings();
        let key = rig
            .host
            .children_view()
            .pop()
            .ok_or("no child")?
            .update
            .id
            .0;
        rig.host.resume(&key);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(endings(), before, "a second ending for one run");
        assert!(ended(&rig.host).await, "the concluded run reads live again");
        Ok(())
    }

    /// Dies with every wait counted: an empty family's wait exempted a repeat forever, and a
    /// settled family's at-once reply exempted a model looping on `rlm.wait` while it settled.
    #[tokio::test]
    async fn only_a_wait_on_a_live_family_counts_for_the_breaker() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut")])?;
        let counted = || rig.host.waits.load(std::sync::atomic::Ordering::SeqCst);
        drop(rig.host.wait(1_000, None).await);
        assert_eq!(counted(), 0, "nothing lives to wait on");
        rig.host.spawn("cut".to_owned(), serde_json::Map::new())?;
        drop(rig.host.wait(1_000, None).await);
        assert_eq!(counted(), 1, "a live child is waited on");
        let _settling = super::Settling::hold(&rig.host);
        assert!(ended(&rig.host).await, "child never completed");
        let settled = rig.host.wait(1_000, None).await?;
        assert_eq!(settled["state"], "settled", "{settled:?}");
        assert_eq!(counted(), 1, "an at-once settled reply is no wait");
        Ok(())
    }

    #[test]
    fn a_child_idle_inside_a_running_tool_still_holds_the_owner() {
        let view = |note: &str| crate::family::MemberView {
            name: "builder".to_owned(),
            state: crate::family::MemberState::Stuck,
            note: Some(note.to_owned()),
            tools: 1,
            tokens: 0,
            idle_s: 400,
            worktree: None,
        };
        assert!(super::holds(&view("idle 400s"), true));
        assert!(!super::holds(&view("idle 400s"), false));
        assert!(!super::holds(&view("repeat_break"), true));
    }

    /// Dies with the ask ending the child's run, or read as outside its tool call.
    #[tokio::test]
    async fn an_asking_child_is_not_submitted() -> TestResult {
        let ask = serde_json::Map::from_iter([("question".to_owned(), "which region?".into())]);
        let rig = hooked(vec![yi_ai::faux::faux_assistant_message(
            vec![yi_ai::faux::faux_tool_call("ask-1", "ask_user", ask)],
            yi_types::message::StopReason::ToolUse,
        )])?;
        rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        let mut asked = None;
        for _ in 0..400 {
            asked = texts(&rig.reports)
                .into_iter()
                .find(|text| text.contains("which region?"));
            if asked.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let asked = asked.ok_or("the parent never got the question")?;
        assert!(asked.contains("reply_to="), "{asked}");
        let plan = rig.engine.apply(owner(Op::View { full: false }))?.plan;
        let todo = plan
            .todo(&yi_types::plan::doc::TodoLabel::new("cut the seam")?)
            .ok_or("todo missing")?;
        assert!(
            matches!(todo.state, TodoState::Running { .. }),
            "{:?}",
            todo.state
        );
        assert!(texts(&rig.said).is_empty(), "nothing was submitted");
        let name = rig
            .host
            .children_view()
            .pop()
            .ok_or("no child")?
            .update
            .name;
        assert!(rig.host.in_tool(&name), "blocked inside ask_user");
        rig.host.interrupt(&name)?;
        for _ in 0..400 {
            if !rig.host.in_tool(&name) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(!rig.host.in_tool(&name), "a stopped run is in no tool");
        Ok(())
    }

    /// Dies with the chase turn harvested: a plan child answered its brief, a request to it
    /// was still open, and the steered turn's "Still nothing to add." became the product.
    #[tokio::test]
    async fn a_chased_plan_child_keeps_its_briefs_answer() -> TestResult {
        let ask = serde_json::Map::from_iter([("question".to_owned(), "which region?".into())]);
        let rig = hooked(vec![
            yi_ai::faux::faux_assistant_message(
                vec![yi_ai::faux::faux_tool_call("ask-1", "ask_user", ask)],
                yi_types::message::StopReason::ToolUse,
            ),
            reply("the seam is cut and holds"),
            reply("Still nothing to add."),
            reply("Nothing more."),
        ])?;
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        let (host, mut child, mut question) = (std::sync::Arc::clone(&rig.host), None, None);
        for _ in 0..400 {
            child = host.children_view().pop().map(|view| view.update.name);
            question = host
                .open_requests()
                .into_iter()
                .find(|open| open.2 == "parent");
            if question.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let (child, (id, ..)) = (child.ok_or("no child")?, question.ok_or("no question")?);
        let asking = (std::sync::Arc::clone(&host), child.clone());
        let request = tokio::spawn(async move {
            asking
                .0
                .request("parent", &asking.1, "total?", 20_000)
                .await
        });
        while !host.open_requests().iter().any(|open| open.2 == child) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let answer = serde_json::json!({"target": child, "message": "eu", "reply_to": id});
        host.send("parent", answer.as_object().ok_or("answer")?)?;
        let todo = settled(&rig, &out.plan.id, "cut the seam").await?;
        let TodoState::Done {
            output: Some(output),
            ..
        } = &todo.state
        else {
            return Err(format!("expected Done with a product, got {:?}", todo.state).into());
        };
        let digest = output.to_string();
        let digest = digest.rsplit('/').next().ok_or("no digest")?;
        let digest = yi_types::plan::canonical::Digest::parse(digest)?;
        let stored = rig.engine.store().artifacts(&out.plan.id).get(&digest)?;
        assert_eq!(
            stored, b"the seam is cut and holds",
            "the brief's answer is the product"
        );
        let reply = request.await??;
        assert_eq!(
            reply["envelope"]["body"], "Still nothing to add.",
            "{reply:?}"
        );
        Ok(())
    }
}
