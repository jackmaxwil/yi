use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::message::{AgentMessage, UserContent};
use yi_types::plan::doc::{AgentId, Check, Delegation, Isolation, TodoAddr, TodoLabel};
use yi_types::schedule::DeliveryMode;
use yi_types::url::Url;

use super::ops::{Delegate, Delta, PlanEngine, PlanOpError, Txn, agent_url};
use super::recovery::Liveness;
use super::state::{self, KIND_SPAWN_INTENT, KIND_SPAWN_RESULT};
use super::store::draft;
use crate::fetch::FetchLog;
use crate::goal::DeliverFn;
use crate::subagent::SubagentHost;
use yi_types::plan::doc::{DocError, PlanId, SPAWN_CAP, TodoState};
use yi_types::plan::ledger::EffectId;

/// [`super::ops::Delegate`] over the B-series host and the owner's Steer queue.
pub struct SessionDelegate {
    host: Arc<SubagentHost>,
    deliver: DeliverFn,
    pins: Arc<FetchLog>,
}

impl SessionDelegate {
    pub fn new(host: Arc<SubagentHost>, deliver: DeliverFn, pins: Arc<FetchLog>) -> Self {
        Self {
            host,
            deliver,
            pins,
        }
    }
}

/// A child is addressed by the todo it executes — its name is the todo
/// address's URL path, so the live and reaped trace addresses need no map.
fn child_name(at: &TodoAddr) -> Result<AgentId, String> {
    let address = at.to_url().map_err(|error| error.to_string())?;
    AgentId::new(address.path()).map_err(|error| error.to_string())
}

fn accept_line(accept: &Check) -> String {
    match accept {
        Check::Command(command) => format!("Acceptance: exit 0 of `{command}`"),
        Check::Stated(stated) => format!("Acceptance: {stated}"),
        Check::Other(other) => format!("Acceptance: {other}"),
    }
}

/// Delegation fields the closed spawn kwargs cannot carry ride the prompt.
fn brief(at: &TodoAddr, delegation: &Delegation) -> String {
    let mut lines = vec![format!(
        "Execute todo {:?} of plan {}.",
        at.todo.as_str(),
        at.plan
    )];
    if let Some(role) = &delegation.spec.role {
        lines.push(format!("Role: {role}"));
    }
    lines.push(accept_line(&delegation.accept));
    if let Some(output) = &delegation.output {
        lines.push(format!(
            "Answer with JSON matching the schema at {}",
            output.schema
        ));
    }
    if !delegation.context.is_empty() {
        lines.push("Context:".to_owned());
        for url in &delegation.context {
            lines.push(format!("- {url}"));
        }
    }
    if !delegation.spec.tools.is_empty() {
        lines.push(format!(
            "Prefer tools: {}",
            delegation.spec.tools.join(", ")
        ));
    }
    if let Some(budget) = &delegation.spec.budget {
        lines.push(format!("Token budget (advisory): {}", budget.0));
    }
    if let Some(note) = &delegation.note {
        lines.push(note.as_str().to_owned());
    }
    lines.join("\n")
}

fn kwargs_of(agent: &AgentId, delegation: &Delegation) -> Result<Map<String, Value>, String> {
    let mut kwargs = Map::new();
    kwargs.insert("name".to_owned(), Value::String(agent.as_str().to_owned()));
    if let Some(model) = &delegation.spec.model {
        kwargs.insert("model".to_owned(), Value::String(model.clone()));
    }
    if let Some(effort) = delegation.spec.effort {
        kwargs.insert("thinking".to_owned(), Value::String(effort.to_string()));
    }
    match &delegation.spec.isolation {
        None | Some(Isolation::None) => {}
        Some(Isolation::Worktree) => {
            kwargs.insert("isolation".to_owned(), Value::String("worktree".to_owned()));
        }
        Some(Isolation::Other(tag)) => {
            return Err(format!(
                "delegation isolation {tag:?} has no spawn mapping; use \"none\" or \"worktree\""
            ));
        }
    }
    if let Check::Command(command) = &delegation.accept {
        kwargs.insert("check".to_owned(), Value::String(command.clone()));
    }
    if let Some(policy) = &delegation.spec.parent_close {
        kwargs.insert("parent_close".to_owned(), serde_json::json!(policy));
    }
    // Plan section 7.6: the spec's wall rides the spawn kwargs the host already reads, so a
    // plan-dispatched reader is walled at the same cooperative seams as an `rlm.run` child.
    if let Some(wall) = &delegation.spec.wall {
        for (key, list) in [
            ("deny_write", &wall.deny_write),
            ("deny_read", &wall.deny_read),
            ("deny_url", &wall.deny_url),
        ] {
            if !list.is_empty() {
                kwargs.insert(key.to_owned(), serde_json::json!(list));
            }
        }
    }
    Ok(kwargs)
}

fn parse_url(rendered: String) -> Result<Url, String> {
    rendered
        .parse::<Url>()
        .map_err(|error| format!("{rendered}: {error}"))
}

impl SessionDelegate {
    fn say(&self, text: String) {
        (self.deliver)(
            AgentMessage::Custom {
                custom_type: "plan_relevance".to_owned(),
                content: UserContent::Text(text),
                display: true,
                details: None,
                timestamp: yi_session::now_ms(),
            },
            DeliveryMode::Steer,
        );
    }
}

/// The host holds a child from spawn to reap: not held means no live process, and a held
/// child that finished still has a result the owner harvests, so it needs no reconciliation.
impl Liveness for SessionDelegate {
    fn alive(&self, agent: &AgentId) -> Option<bool> {
        Some(self.host.holds(agent.as_str()))
    }
}

/// The spawn half of `start` (plan section 5.3): the intent and its result are two records
/// around the one effect, so a crash between them is reconciled, never re-spawned blind.
impl PlanEngine {
    pub(super) fn ensure_spawned(
        &self,
        txn: &mut Txn,
        id: &PlanId,
        root: &PlanId,
        label: &TodoLabel,
        delta: &mut Delta,
    ) -> Result<(), PlanOpError> {
        let plan = txn.state.plan(id)?;
        let Some(todo) = plan.todo(label) else {
            return Ok(());
        };
        let Some(delegation) = todo.delegation.clone() else {
            return Ok(());
        };
        if !matches!(todo.state, TodoState::Pending) {
            return Ok(());
        }
        let addr = TodoAddr {
            plan: id.clone(),
            todo: label.clone(),
        };
        let url = agent_url(&addr)?;
        if let Some((effect, intent)) = txn.state.intent_for(id, label) {
            return match &intent.outcome {
                state::IntentOutcome::Spawned { .. } => {
                    delta.spawned.push(url);
                    Ok(())
                }
                state::IntentOutcome::Pending => Err(PlanOpError::NeedsReconciliation {
                    label: label.clone(),
                    effect: effect.clone(),
                }),
            };
        }
        let spent = txn.state.plan(root)?.spawns();
        if spent >= SPAWN_CAP {
            return Err(PlanOpError::SpawnCeilingExhausted {
                spent,
                cap: SPAWN_CAP,
            });
        }
        let attempt = todo.attempt;
        // Invariant: a journal outlives the pid, so the effect id carries the clock too.
        let effect =
            EffectId::new(format!("e-{}", self.store.request_nonce())).map_err(|error| {
                PlanOpError::Doc(DocError::AgentIdWhitespace {
                    id: error.to_string(),
                })
            })?;
        let todos = u32::try_from(plan.todos.len()).unwrap_or(u32::MAX);
        let mut intent = draft(
            id,
            KIND_SPAWN_INTENT,
            txn.actor.clone(),
            self.store.now_ms(),
            todos,
            serde_json::json!({"label": label, "attempt": attempt, "effect_id": effect}),
            txn.derived("intent")?,
            txn.expected,
            Some(attempt),
        );
        intent.record.todo = Some(label.clone());
        self.commit(txn, intent)?;
        let spawned = self.delegate.spawn(&addr, &delegation);
        let args = match &spawned {
            Ok(agent) => serde_json::json!({"effect_id": effect, "agent": agent}),
            Err(reason) => serde_json::json!({"effect_id": effect, "error": reason}),
        };
        let mut result = draft(
            id,
            KIND_SPAWN_RESULT,
            txn.actor.clone(),
            self.store.now_ms(),
            todos,
            args,
            txn.derived("result")?,
            txn.expected,
            Some(attempt),
        );
        result.record.todo = Some(label.clone());
        self.commit(txn, result)?;
        match spawned {
            Ok(_) => {
                delta.spawned.push(url);
                Ok(())
            }
            Err(reason) => {
                self.store.checkpoint_family(&txn.state)?;
                Err(PlanOpError::SpawnFailed { at: addr, reason })
            }
        }
    }
}

impl Delegate for SessionDelegate {
    fn candidate(&self, agent: &AgentId) -> Result<Option<crate::lane::settle::Held>, String> {
        if !self.host.holds(agent.as_str()) {
            return Ok(None);
        }
        self.host.candidate_of(agent.as_str())
    }

    fn mark(&self, agent: &AgentId, choice: yi_types::plan::op::Choice) -> Result<(), String> {
        if !self.host.holds(agent.as_str()) {
            return Ok(());
        }
        self.host.mark_disposed(agent.as_str(), choice)
    }

    fn spawn(&self, at: &TodoAddr, delegation: &Delegation) -> Result<AgentId, String> {
        let agent = child_name(at)?;
        self.host
            .spawn(brief(at, delegation), kwargs_of(&agent, delegation)?)?;
        // Its worktree goes through `submit` or a journaled disposition: the kernel's merge
        // and discard are refused for a child the engine dispatched.
        self.host.mark_managed(agent.as_str())?;
        Ok(agent)
    }

    fn reap(&self, agent: &AgentId, supplied: &[Url]) -> Result<Option<Url>, String> {
        if !self.host.holds(agent.as_str()) {
            return Ok(None);
        }
        // Measured before the reap frees the record, and only when something was
        // handed over: with nothing supplied there is no supply to be wrong about.
        let measured = (!supplied.is_empty())
            .then(|| self.host.transcript(agent.as_str()))
            .flatten()
            .map(|session| crate::fetch::relevance_of(&session, supplied));
        let harvest = self.host.reap(agent.as_str())?;
        let live = parse_url(format!("agent://{agent}"))?;
        let trace = parse_url(format!("history://{agent}"))?;
        self.pins
            .register_pin(&live, trace.clone())
            .map_err(|error| error.to_string())?;
        if let Some(measured) = measured {
            self.say(format!(
                "context relevance for {agent}: {} of {} supplied URLs were read{}",
                measured.referenced,
                measured.supplied,
                if measured.unused.is_empty() {
                    String::new()
                } else {
                    format!("; unread: {}", measured.unused.join(", "))
                }
            ));
        }
        Ok(harvest.produced.then_some(trace))
    }

    fn follow_up(&self, dispatched: &[TodoLabel], held: usize) {
        let mut text = String::from("plan dispatch: ");
        if dispatched.is_empty() {
            text.push_str("no todo became dispatchable");
        } else {
            let labels: Vec<&str> = dispatched.iter().map(TodoLabel::as_str).collect();
            text.push_str(&format!("ready to start: {}", labels.join(", ")));
        }
        if held > 0 {
            text.push_str(&format!(
                " — {held} ready todo(s) held behind the dispatch width"
            ));
        }
        (self.deliver)(
            AgentMessage::Custom {
                custom_type: "plan_dispatch".to_owned(),
                content: UserContent::Text(text),
                display: true,
                details: None,
                timestamp: yi_session::now_ms(),
            },
            DeliveryMode::Steer,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;
    use std::sync::Mutex;
    use yi_loop::ExecutionMode;
    use yi_types::message::StopReason;
    use yi_types::model::{Model, ModelCost};
    use yi_types::plan::doc::{GoalText, SpawnSpec, TodoState};
    use yi_types::url::Durability;

    use crate::plan::ops::{Actor, Op, OpRequest, PlanEngine, TodoSpec};
    use crate::plan::store::PlanStore;
    use crate::scratch::Scratch;
    use crate::session::{AgentSession, SessionConfig};
    use crate::subagent::{ChildBuild, ChildStatus, SubagentHostOptions};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn faux_model() -> Model {
        let zero = || serde_json::Number::from(0u64);
        Model {
            id: "faux-1".to_owned(),
            name: "Faux".to_owned(),
            api: "faux".to_owned(),
            provider: "faux".to_owned(),
            base_url: "http://localhost:0".to_owned(),
            reasoning: false,
            input: vec!["text".to_owned()],
            cost: ModelCost {
                input: zero(),
                output: zero(),
                cache_read: zero(),
                cache_write: zero(),
                tiers: None,
            },
            context_window: 128_000,
            max_tokens: 16_384,
            compat: None,
            thinking_level_map: None,
            headers: None,
        }
    }

    fn faux_session(replies: &[&str]) -> AgentSession {
        let provider = Arc::new(crate::provider::ProviderStream::new(None, None));
        for reply in replies {
            provider.queue_faux(vec![yi_ai::faux::faux_assistant_message(
                vec![yi_ai::faux::faux_text(reply)],
                StopReason::Stop,
            )]);
        }
        AgentSession::new(
            SessionConfig {
                system_prompt: "sys".to_owned(),
                model: faux_model(),
                thinking_level: None,
                tool_execution: ExecutionMode::Sequential,
            },
            provider,
        )
    }

    struct Rig {
        engine: PlanEngine,
        host: Arc<SubagentHost>,
        pins: Arc<FetchLog>,
        reports: Arc<Mutex<Vec<AgentMessage>>>,
        delivered: Arc<Mutex<Vec<AgentMessage>>>,
        cwd: Scratch,
    }

    fn rig(child_answer: &'static str) -> Result<Rig, Box<dyn std::error::Error>> {
        let root = Scratch::new("yi-dispatch-rig")?;
        let (events, _keep) = tokio::sync::broadcast::channel(64);
        let reports: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let report_sink = Arc::clone(&reports);
        let host = Arc::new(SubagentHost::new(SubagentHostOptions {
            depth: 0,
            max_depth: 1,
            max_children: 8,
            parent_session_dir: root.join("children"),
            cwd: root.to_path_buf(),
            home: std::env::temp_dir(),
            lane_slots: 1,
            defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
            factory: Arc::new(move |build: ChildBuild<'_>| {
                let _ = build;
                Ok(faux_session(&[child_answer]))
            }),
            notice: Arc::new(|_text: &str| {}),
            events,
            parent_messages: Arc::new(Vec::new),
            report: Arc::new(move |message| {
                if let Ok(mut sink) = report_sink.lock() {
                    sink.push(message);
                }
            }),
            attribute: Arc::new(|_usage| {}),
            store: Arc::new(|| None),
            plans_dir: root.join(crate::plan::PLANS_DIR),
            family_live: Arc::new(|| 0),
        }));
        let delivered: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let deliver_sink = Arc::clone(&delivered);
        let pins = Arc::new(FetchLog::new());
        let delegate = Arc::new(SessionDelegate::new(
            Arc::clone(&host),
            Arc::new(move |message, _mode| {
                if let Ok(mut sink) = deliver_sink.lock() {
                    sink.push(message);
                }
            }),
            Arc::clone(&pins),
        ));
        let engine = PlanEngine::new(PlanStore::open(root.join("plans"))?, delegate)
            .with_width(NonZeroUsize::new(2).ok_or("width")?);
        Ok(Rig {
            engine,
            host,
            pins,
            reports,
            delivered,
            cwd: root,
        })
    }

    fn owner(op: Op) -> OpRequest {
        OpRequest {
            plan: None,
            actor: Actor::Owner,
            op,
            request_id: None,
            expected_revision: None,
        }
    }

    fn delegated(label: &str) -> Result<TodoSpec, Box<dyn std::error::Error>> {
        Ok(TodoSpec {
            label: TodoLabel::new(label)?,
            after: Vec::new(),
            delegation: Some(Delegation {
                spec: SpawnSpec {
                    role: Some("worker".to_owned()),
                    model: None,
                    effort: None,
                    tools: Vec::new(),
                    isolation: None,
                    budget: None,
                    wall: None,
                    parent_close: None,
                    extra: Map::new(),
                },
                accept: Check::Command("true".to_owned()),
                output: None,
                context: Vec::new(),
                note: None,
                extra: Map::new(),
            }),
            contract: None,
            children: Vec::new(),
        })
    }

    async fn wait_done(host: &Arc<SubagentHost>) -> bool {
        for _ in 0..400 {
            let done = host
                .children_view()
                .iter()
                .any(|child| child.update.status == ChildStatus::Completed);
            if done {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        false
    }

    fn texts(messages: &Arc<Mutex<Vec<AgentMessage>>>) -> Vec<String> {
        messages
            .lock()
            .map(|sink| {
                sink.iter()
                    .filter_map(|message| match message {
                        AgentMessage::Custom {
                            content: UserContent::Text(text),
                            ..
                        } => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn a_ready_todo_dispatches_a_child_and_reap_promotes_its_product() -> TestResult {
        let rig = rig("the seam is cut and holds")?;
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        assert_eq!(out.dispatched, vec![TodoLabel::new("cut the seam")?]);
        let follow = texts(&rig.delivered);
        assert!(
            follow.iter().any(|text| text.contains("cut the seam")),
            "the follow-up names the dispatchable slice: {follow:?}"
        );
        let out = rig.engine.apply(owner(Op::Start {
            label: TodoLabel::new("cut the seam")?,
        }))?;
        assert_eq!(out.spawned.len(), 1);
        assert!(wait_done(&rig.host).await, "child never completed");
        let out = rig.engine.apply(owner(Op::Done {
            label: TodoLabel::new("cut the seam")?,
            output: Some("local://seam.md".parse()?),
        }))?;
        assert_eq!(out.reaped.len(), 1);
        let promoted = texts(&rig.reports);
        assert!(
            promoted
                .iter()
                .any(|text| text.contains("reaped_child") && text.contains("seam is cut")),
            "the product is promoted into the owner's transcript at reap: {promoted:?}"
        );
        let agent = format!("agent://{}/cut-the-seam", out.plan.id.as_str()).parse::<Url>()?;
        let pin = rig.pins.pin_of(&agent).ok_or("no pin registered at reap")?;
        assert_eq!(pin.durability(), Durability::Durable);
        assert!(pin.to_string().starts_with("history://"), "{pin}");
        // Incident: the real seam looked a reaped child up by name under the
        // sessions root, where no child transcript lives; only a stub desk passed.
        let workspace = Scratch::new("yi-dispatch-resolver")?;
        let resolver =
            crate::fetch::Resolver::new(workspace.to_path_buf(), crate::wall::Wall::default())
                .with_log(Arc::clone(&rig.pins))
                .with_transcripts(Arc::new(crate::fetch::SessionTranscripts::new(
                    Arc::clone(&rig.host),
                    None,
                    &rig.cwd,
                )));
        let fetched = resolver.fetch(&agent)?;
        assert!(
            fetched.served_by.starts_with("reap-pin history://"),
            "agent:// must resolve through the shared log into its pin: {}",
            fetched.served_by
        );
        assert!(
            fetched.text.contains("seam is cut"),
            "the reaped child's transcript answers for its pin: {}",
            fetched.text
        );
        assert!(
            rig.host.children_view().is_empty(),
            "the reaped child's slot is disposed"
        );
        Ok(())
    }

    #[tokio::test]
    async fn reaping_a_child_the_host_no_longer_holds_is_a_no_op() -> TestResult {
        let rig = rig("done and gone")?;
        rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        rig.engine.apply(owner(Op::Start {
            label: TodoLabel::new("cut the seam")?,
        }))?;
        assert!(wait_done(&rig.host).await, "child never completed");
        let delegate = SessionDelegate::new(
            Arc::clone(&rig.host),
            Arc::new(|_message, _mode| {}),
            Arc::clone(&rig.pins),
        );
        let agent = AgentId::new(
            rig.engine
                .apply(owner(Op::View { full: false }))?
                .plan
                .id
                .child(&TodoLabel::new("cut the seam")?)?
                .as_str()
                .replace('.', "/"),
        )?;
        assert!(
            delegate.reap(&agent, &[])?.is_some(),
            "the first reap harvests"
        );
        assert!(
            delegate.reap(&agent, &[])?.is_none(),
            "a second reap of a child the host no longer holds must answer, not refuse"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_childs_last_product_survives_the_reap() -> TestResult {
        let rig = rig("half a patch, then it went sideways")?;
        rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("land the patch")?,
            todos: vec![delegated("write the patch")?],
        }))?;
        rig.engine.apply(owner(Op::Start {
            label: TodoLabel::new("write the patch")?,
        }))?;
        assert!(wait_done(&rig.host).await, "child never completed");
        let out = rig.engine.apply(owner(Op::Fail {
            disposition: None,
            label: TodoLabel::new("write the patch")?,
            cause: "the probe disagreed".to_owned(),
        }))?;
        let todo = out
            .plan
            .todo(&TodoLabel::new("write the patch")?)
            .ok_or("todo missing")?;
        let TodoState::Failed { cause, last } = &todo.state else {
            return Err(format!("expected Failed, got {:?}", todo.state).into());
        };
        assert_eq!(cause, "the probe disagreed");
        let last = last
            .clone()
            .ok_or("a child that produced work carries last")?;
        assert_eq!(last.durability(), Durability::Durable);
        assert_eq!(
            last.to_string(),
            format!("history://{}/write-the-patch", out.plan.id.as_str())
        );
        let promoted = texts(&rig.reports);
        assert!(
            promoted.iter().any(|text| text.contains("half a patch")),
            "the failure path still promotes the last product: {promoted:?}"
        );
        Ok(())
    }

    /// Dies with the wall block in `kwargs_of`: drop it and a plan-dispatched reader spawns
    /// with the parent's whole capability set (plan section 7.6).
    #[test]
    fn kwargs_carry_the_wall() -> TestResult {
        let mut delegation = delegated("read the docs")?.delegation.ok_or("delegated")?;
        delegation.spec.wall = Some(yi_types::plan::doc::WallSpec {
            deny_write: vec![".".to_owned()],
            deny_read: vec!["secrets/".to_owned()],
            deny_url: vec!["history://main".to_owned()],
        });
        let kwargs = kwargs_of(&AgentId::new("reader-1")?, &delegation)?;
        assert_eq!(kwargs["deny_write"], serde_json::json!(["."]));
        assert_eq!(kwargs["deny_read"], serde_json::json!(["secrets/"]));
        assert_eq!(kwargs["deny_url"], serde_json::json!(["history://main"]));
        let wall = crate::wall::Wall::from_kwargs(&kwargs, std::path::Path::new("/tmp"))?;
        assert!(
            !wall.is_empty(),
            "the host reads the same keys it is handed"
        );
        delegation.spec.wall = None;
        let bare = kwargs_of(&AgentId::new("reader-2")?, &delegation)?;
        assert!(!bare.contains_key("deny_write") && !bare.contains_key("deny_url"));
        Ok(())
    }

    #[tokio::test]
    async fn the_follow_up_wakes_an_idle_owner_and_names_the_held_count() -> TestResult {
        let owner_session = faux_session(&["picking up the dispatched todo"]);
        let deliver = owner_session.heartbeat_hook();
        let root = Scratch::new("yi-dispatch-idle")?;
        let (events, _keep) = tokio::sync::broadcast::channel(16);
        let host = Arc::new(SubagentHost::new(SubagentHostOptions {
            depth: 0,
            max_depth: 1,
            max_children: 8,
            parent_session_dir: root.join("children"),
            plans_dir: root.join(crate::plan::PLANS_DIR),
            family_live: Arc::new(|| 0),
            cwd: root.to_path_buf(),
            home: std::env::temp_dir(),
            lane_slots: 1,
            defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
            factory: Arc::new(|_build| Err("no child in this test".to_owned())),
            notice: Arc::new(|_text: &str| {}),
            events,
            parent_messages: Arc::new(Vec::new),
            report: Arc::new(|_message| {}),
            attribute: Arc::new(|_usage| {}),
            store: Arc::new(|| None),
        }));
        let delegate = SessionDelegate::new(host, deliver, Arc::new(FetchLog::new()));
        delegate.follow_up(&[TodoLabel::new("cut the seam")?], 3);
        let mut woke = false;
        for _ in 0..400 {
            let replied = owner_session
                .messages()
                .iter()
                .any(|message| matches!(message, AgentMessage::Assistant { .. }));
            if replied {
                woke = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            woke,
            "an idle owner must be woken by the follow-up, not polled"
        );
        let carried = owner_session.messages().iter().any(|message| {
            matches!(
                message,
                AgentMessage::Custom { content: UserContent::Text(text), .. }
                    if text.contains("cut the seam")
                        && text.contains("3 ready todo(s) held behind the dispatch width")
            )
        });
        assert!(carried, "the nudge names the slice and the held count");
        Ok(())
    }

    /// Guards `wiring::lifecycle_notice`: restore `session.notice_hook()` there and the
    /// child's finish only queues a steer for a turn nobody starts.
    #[tokio::test]
    async fn a_childs_finish_wakes_an_idle_owner() -> TestResult {
        let owner_session = faux_session(&["reading the child's answer"]);
        let notice = crate::wiring::lifecycle_notice(&owner_session);
        let root = Scratch::new("yi-dispatch-child-wake")?;
        let (events, _keep) = tokio::sync::broadcast::channel(16);
        let host = Arc::new(SubagentHost::new(SubagentHostOptions {
            depth: 0,
            max_depth: 1,
            max_children: 8,
            parent_session_dir: root.join("children"),
            plans_dir: root.join(crate::plan::PLANS_DIR),
            family_live: Arc::new(|| 0),
            cwd: root.to_path_buf(),
            home: std::env::temp_dir(),
            lane_slots: 1,
            defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
            factory: Arc::new(|_build| Ok(faux_session(&["the child's answer"]))),
            notice,
            events,
            parent_messages: Arc::new(Vec::new),
            report: Arc::new(|_message| {}),
            attribute: Arc::new(|_usage| {}),
            store: Arc::new(|| None),
        }));
        let mut kwargs = serde_json::Map::new();
        kwargs.insert(
            "name".to_owned(),
            serde_json::Value::String("helper".to_owned()),
        );
        host.spawn("answer once".to_owned(), kwargs)
            .map_err(|error| error.to_string())?;
        let mut woke = false;
        for _ in 0..400 {
            let notified = owner_session.messages().iter().any(|message| {
                matches!(
                    message,
                    AgentMessage::User { content: UserContent::Text(text), .. }
                        if text.contains("[subagent helper")
                )
            });
            let replied = owner_session
                .messages()
                .iter()
                .any(|message| matches!(message, AgentMessage::Assistant { .. }));
            if notified && replied {
                woke = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            woke,
            "an idle owner must be woken by its child's finish: {:?}",
            owner_session.messages()
        );
        Ok(())
    }

    /// The session's liveness is the host's hold: a spawned child is alive until it is reaped,
    /// finished or not, and a name the host never held has no live process.
    #[tokio::test]
    async fn a_held_child_is_alive_and_an_unknown_one_is_not() -> TestResult {
        let rig = rig("the seam holds")?;
        rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        let out = rig.engine.apply(owner(Op::Start {
            label: TodoLabel::new("cut the seam")?,
        }))?;
        let spawned = out.spawned.first().ok_or("nothing spawned")?;
        let child = AgentId::new(spawned.path())?;
        let liveness = SessionDelegate::new(
            Arc::clone(&rig.host),
            Arc::new(|_message, _mode| {}),
            Arc::clone(&rig.pins),
        );
        assert_eq!(liveness.alive(&child), Some(true));
        assert_eq!(liveness.alive(&AgentId::new("never-spawned")?), Some(false));
        assert!(wait_done(&rig.host).await, "child never completed");
        assert_eq!(
            liveness.alive(&child),
            Some(true),
            "a finished child the owner has not harvested is still held"
        );
        rig.engine.apply(owner(Op::Done {
            label: TodoLabel::new("cut the seam")?,
            output: Some("local://seam.md".parse()?),
        }))?;
        assert_eq!(
            liveness.alive(&child),
            Some(false),
            "a reaped child is gone"
        );
        Ok(())
    }
}
