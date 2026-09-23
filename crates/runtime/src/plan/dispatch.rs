use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::message::{AgentMessage, UserContent};
use yi_types::plan::canonical::DIGEST_PREFIX;
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
        lines.push(format!(
            "Token budget (reserved, not enforced): {}",
            budget.0
        ));
    }
    if let Some(note) = &delegation.note {
        lines.push(note.as_str().to_owned());
    }
    let whole = delegation.extra.get(super::declare::NOTE_REF);
    if let Some(hex) =
        whole.and_then(|note| note.get("digest")?.as_str()?.strip_prefix(DIGEST_PREFIX))
    {
        lines.push(format!(
            "The whole note: read plan://{}/artifacts/{hex}",
            at.plan
        ));
    }
    if let Some(Value::Array(told)) = delegation.extra.get(super::brief::KEY) {
        lines.extend(told.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    lines.push(
        "When the work is done, end your turn with your answer; the engine takes it as your work."
            .to_owned(),
    );
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
    // Plan section 7.4: a lease is drawn at every spawn road, so the spec's budget is the
    // engine's ask against the parent's own, refused with both numbers rather than clamped.
    if let Some(budget) = &delegation.spec.budget {
        kwargs.insert("tokens".to_owned(), Value::from(budget.0));
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

pub(super) fn say(deliver: &DeliverFn, text: String) {
    deliver(
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
        let Some(mut delegation) = todo.delegation.clone() else {
            return Ok(());
        };
        if !matches!(todo.state, TodoState::Pending) {
            return Ok(());
        }
        let told = super::brief::lines(&self.store.artifacts(id), &txn.records, id, todo);
        if !told.is_empty() {
            delegation
                .extra
                .insert(super::brief::KEY.to_owned(), serde_json::json!(told));
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
    fn finishes(&self) -> bool {
        self.host.finished.lock().is_ok_and(|slot| slot.is_some())
    }

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
        let kwargs = kwargs_of(&agent, delegation)?;
        // Plan section 7.6: a brief whose own wall denies its context is refused here, with
        // the denial as evidence, not one fetch later inside a child that cannot do its job.
        let wall = self.host.wall_for(&kwargs)?;
        let cwd = &self.host.options.cwd;
        if let Some(denied) = delegation
            .context
            .iter()
            .find_map(|url| wall.check_url(url, cwd))
        {
            return Err(format!("the brief names context its wall denies. {denied}"));
        }
        self.host.spawn(brief(at, delegation), kwargs)?;
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
            say(
                &self.deliver,
                format!(
                    "context relevance for {agent}: {} of {} supplied URLs were read{}",
                    measured.referenced,
                    measured.supplied,
                    if measured.unused.is_empty() {
                        String::new()
                    } else {
                        format!("; unread: {}", measured.unused.join(", "))
                    }
                ),
            );
        }
        Ok(harvest.produced.then_some(trace))
    }
}

#[cfg(test)]
pub(super) mod tests {
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

    pub(in crate::plan) fn reply(text: &str) -> AgentMessage {
        yi_ai::faux::faux_assistant_message(vec![yi_ai::faux::faux_text(text)], StopReason::Stop)
    }

    fn scripted(script: Vec<AgentMessage>) -> AgentSession {
        let provider = Arc::new(crate::provider::ProviderStream::new(None, None));
        provider.queue_faux(script);
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

    fn faux_session(replies: &[&str]) -> AgentSession {
        scripted(replies.iter().map(|text| reply(text)).collect())
    }

    pub(in crate::plan) type Sink<T> = Arc<Mutex<Vec<T>>>;

    fn sink<T: Send + 'static>() -> (Sink<T>, Arc<dyn Fn(T) + Send + Sync>) {
        let sink: Sink<T> = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&sink);
        let push = Arc::new(move |item: T| {
            if let Ok(mut items) = writer.lock() {
                items.push(item);
            }
        });
        (sink, push)
    }

    pub(in crate::plan) struct Rig {
        pub(in crate::plan) engine: Arc<PlanEngine>,
        pub(in crate::plan) host: Arc<SubagentHost>,
        pins: Arc<FetchLog>,
        pub(in crate::plan) reports: Sink<AgentMessage>,
        /// The host's own lifecycle notices, which a child the engine took never sends.
        notices: Sink<String>,
        /// The engine's lines to the owner.
        pub(in crate::plan) said: Sink<AgentMessage>,
        pub(in crate::plan) cwd: Scratch,
    }

    fn rig(child_answer: &'static str) -> Result<Rig, Box<dyn std::error::Error>> {
        rig_of(vec![reply(child_answer)], false)
    }

    /// The root session's rig: the engine takes each finished plan child (D225).
    pub(in crate::plan) fn hooked(
        script: Vec<AgentMessage>,
    ) -> Result<Rig, Box<dyn std::error::Error>> {
        rig_of(script, true)
    }

    fn rig_of(script: Vec<AgentMessage>, finish: bool) -> Result<Rig, Box<dyn std::error::Error>> {
        let root = Scratch::new("yi-dispatch-rig")?;
        let (events, _keep) = tokio::sync::broadcast::channel(64);
        let (reports, report) = sink();
        let (notices, notice) = sink::<String>();
        let (said, say) = sink();
        let asks = script.iter().any(|message| {
            crate::family::pending_question(std::slice::from_ref(message)).is_some()
        });
        let cwd = root.to_path_buf();
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
                let mut child = scripted(script.clone());
                if asks {
                    let ask: Arc<dyn yi_tools::Tool> =
                        Arc::new(crate::auto_review::AskUserTool::new(None));
                    child.use_tools(vec![ask], cwd.clone(), None);
                }
                Ok(child)
            }),
            notice: Arc::new(move |text: &str| notice(text.to_owned())),
            events,
            parent_messages: Arc::new(Vec::new),
            report: Arc::new(move |message| report(message)),
            attribute: Arc::new(|_usage| {}),
            store: Arc::new(|| None),
            plans_dir: root.join(crate::plan::PLANS_DIR),
            family_live: Arc::new(|| 0),
        }));
        let pins = Arc::new(FetchLog::new());
        let delegate = Arc::new(SessionDelegate::new(
            Arc::clone(&host),
            Arc::new(|_message, _mode| {}),
            Arc::clone(&pins),
        ));
        let engine = Arc::new(
            PlanEngine::new(PlanStore::open(root.join("plans"))?, delegate)
                .with_width(NonZeroUsize::new(2).ok_or("width")?)
                .with_cwd(root.to_path_buf()),
        );
        if finish {
            crate::plan::finish::install(
                &host,
                &engine,
                Arc::new(move |message, _mode| say(message)),
            );
        }
        Ok(Rig {
            engine,
            host,
            pins,
            reports,
            notices,
            said,
            cwd: root,
        })
    }

    pub(in crate::plan) fn owner(op: Op) -> OpRequest {
        OpRequest {
            plan: None,
            actor: Actor::Owner,
            op,
            request_id: None,
            expected_revision: None,
        }
    }

    pub(in crate::plan) fn delegated(label: &str) -> Result<TodoSpec, Box<dyn std::error::Error>> {
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

    pub(in crate::plan) fn texts(messages: &Arc<Mutex<Vec<AgentMessage>>>) -> Vec<String> {
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
        assert_eq!(
            out.spawned.len(),
            1,
            "the engine starts it with no owner op"
        );
        let noted = rig.engine.apply(owner(Op::Start {
            label: TodoLabel::new("cut the seam")?,
        }))?;
        assert!(
            noted
                .notices
                .iter()
                .any(|line| line.starts_with("nothing to do: the engine starts")),
            "the owner has nothing left to start: {:?}",
            noted.notices
        );
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

    /// The todo once the engine has stepped it and told the owner, polled from the store.
    pub(in crate::plan) async fn settled(
        rig: &Rig,
        plan: &PlanId,
        label: &str,
    ) -> Result<yi_types::plan::doc::Todo, Box<dyn std::error::Error>> {
        let label = TodoLabel::new(label)?;
        for _ in 0..400 {
            let read = rig.engine.store().read(plan)?;
            let todo = read.todo(&label).ok_or("todo missing")?;
            if !texts(&rig.said).is_empty() && !matches!(todo.state, TodoState::Running { .. }) {
                return Ok(todo.clone());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        Err(format!("{label:?} never left running").into())
    }

    fn failing(text: &str, error: &str) -> AgentMessage {
        let mut message = yi_ai::faux::faux_assistant_message(
            vec![yi_ai::faux::faux_text(text)],
            StopReason::Error,
        );
        if let AgentMessage::Assistant { error_message, .. } = &mut message {
            *error_message = Some(error.to_owned());
        }
        message
    }

    /// Dies with the `Failed` arm of `conclude` skipping the finish hook: the todo stays
    /// running under a child that ended, and the owner is never told.
    #[tokio::test]
    async fn a_failed_childs_last_product_survives_the_reap() -> TestResult {
        let rig = hooked(vec![failing(
            "half a patch, then it went sideways",
            "the provider hung up",
        )])?;
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("land the patch")?,
            todos: vec![delegated("write the patch")?],
        }))?;
        let todo = settled(&rig, &out.plan.id, "write the patch").await?;
        let TodoState::Failed { cause, last } = &todo.state else {
            return Err(format!("expected Failed, got {:?}", todo.state).into());
        };
        assert_eq!(
            cause, "the provider hung up",
            "the engine fails it with the child's error"
        );
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
        assert_eq!(
            texts(&rig.said),
            ["plan: failed \"write the patch\": the provider hung up"]
        );
        Ok(())
    }

    /// Dies with the finish skipping the delegation's accept command (finish.rs `held_back`):
    /// the engine's `done` completes an uncontracted todo whose check is red.
    #[tokio::test]
    async fn a_red_accept_command_fails_the_finish() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut")])?;
        let mut spec = delegated("cut the seam")?;
        if let Some(delegation) = spec.delegation.as_mut() {
            delegation.accept = Check::Command("false".to_owned());
        }
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![spec],
        }))?;
        let todo = settled(&rig, &out.plan.id, "cut the seam").await?;
        let TodoState::Failed { cause, .. } = &todo.state else {
            return Err(format!("expected Failed, got {:?}", todo.state).into());
        };
        assert!(cause.starts_with("its check is red"), "{cause}");
        Ok(())
    }

    /// Dies with the `(Completed, None)` arm of `conclude` skipping the finish hook: the child
    /// ends, its todo stays running, and only an owner `done` could complete it.
    #[tokio::test]
    async fn an_inline_childs_finish_is_accepted_without_an_owner_op() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut and holds")])?;
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        let id = out.plan.id.clone();
        let todo = settled(&rig, &id, "cut the seam").await?;
        let TodoState::Done {
            output: Some(output),
            ..
        } = &todo.state
        else {
            return Err(format!("expected Done with a product, got {:?}", todo.state).into());
        };
        let digest = output
            .to_string()
            .strip_prefix(&format!("plan://{id}/artifacts/"))
            .map(yi_types::plan::canonical::Digest::parse)
            .ok_or_else(|| format!("{output} is not in the plan's store"))??;
        let stored = rig.engine.store().artifacts(&id).get(&digest)?;
        assert_eq!(
            stored, b"the seam is cut and holds",
            "the product is the child's answer"
        );
        assert_eq!(
            texts(&rig.said),
            [format!(
                "plan: accepted \"cut the seam\" (agent://{id}/cut-the-seam)"
            )]
        );
        let journal = rig.engine.store().journal(&id).read()?.records;
        let actors: Vec<(&str, &str)> = journal
            .iter()
            .filter(|record| ["submit", "done"].contains(&record.record.op.as_str()))
            .map(|record| (record.record.op.as_str(), record.record.actor.as_str()))
            .collect();
        assert_eq!(actors, [("submit", "engine"), ("done", "engine")]);
        assert!(
            texts(&rig.reports)
                .iter()
                .any(|text| text.contains("seam is cut")),
            "the done reaps the child and promotes its transcript"
        );
        let notices = rig.notices.lock().map_err(|_| "poisoned")?.clone();
        assert!(
            notices.iter().all(|notice| !notice.contains("[subagent")),
            "a child the engine took sends no second notice: {notices:?}"
        );
        Ok(())
    }

    /// Dies with the hook consulted on the asking arm (D165): a child waiting on its parent
    /// would be submitted and accepted mid-question.
    #[tokio::test]
    async fn an_asking_child_is_not_submitted() -> TestResult {
        let mut ask = Map::new();
        ask.insert(
            "question".to_owned(),
            Value::String("which region?".to_owned()),
        );
        let rig = hooked(vec![yi_ai::faux::faux_assistant_message(
            vec![yi_ai::faux::faux_tool_call("ask-1", "ask_user", ask)],
            StopReason::ToolUse,
        )])?;
        rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        let asked = wait_notice(&rig, "asks: which region?").await?;
        assert!(asked.contains("rlm.send"), "{asked}");
        let plan = rig.engine.apply(owner(Op::View { full: false }))?.plan;
        let todo = plan
            .todo(&TodoLabel::new("cut the seam")?)
            .ok_or("todo missing")?;
        assert!(
            matches!(todo.state, TodoState::Running { .. }),
            "{:?}",
            todo.state
        );
        assert!(texts(&rig.said).is_empty(), "nothing was submitted");
        Ok(())
    }

    /// Dies with `locate` answering for any agent: an `rlm.run` child would lose the finish
    /// notice that is its only report.
    #[tokio::test]
    async fn an_rlm_run_child_is_not_a_plan_finish() -> TestResult {
        let rig = hooked(vec![reply("the quota parser lands")])?;
        rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: Vec::new(),
        }))?;
        rig.host.spawn(
            "work".to_owned(),
            Map::from_iter([("name".to_owned(), Value::String("quota".to_owned()))]),
        )?;
        let notice = wait_notice(&rig, "[subagent quota").await?;
        assert!(notice.contains("the quota parser lands"), "{notice}");
        assert!(
            texts(&rig.said).is_empty(),
            "no plan line for a child no todo runs"
        );
        Ok(())
    }

    async fn wait_notice(rig: &Rig, needle: &str) -> Result<String, Box<dyn std::error::Error>> {
        for _ in 0..400 {
            let found = rig
                .notices
                .lock()
                .map_err(|_| "poisoned")?
                .iter()
                .find(|notice| notice.contains(needle))
                .cloned();
            if let Some(notice) = found {
                return Ok(notice);
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        Err(format!("no notice with {needle:?}").into())
    }

    /// Dies with the wall block in `kwargs_of`: drop it and a plan-dispatched reader spawns
    /// with the parent's whole capability set (plan section 7.6).
    /// Incident: nine of twelve F0e "text, not JSON" refusals were a valid object inside a
    /// fenced block, and the parent had no road past them (#475).
    #[tokio::test]
    async fn a_fenced_json_answer_validates_against_a_schema() -> TestResult {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"outcome": {"type": "string"}},
            "required": ["outcome"],
        });
        let fenced = rig("```json\n{\"outcome\": \"the quota parser lands\"}\n```")?;
        fenced.host.spawn(
            "work".to_owned(),
            Map::from_iter([("name".to_owned(), Value::String("quota".to_owned()))]),
        )?;
        assert!(wait_done(&fenced.host).await, "child never completed");
        let reply = fenced.host.result("quota", Some(&schema))?;
        assert_eq!(
            reply["json"]["outcome"],
            Value::String("the quota parser lands".to_owned())
        );

        // Prose is still refused, with its text attached: there is nothing to read as JSON.
        let prose = rig("the quota parser lands")?;
        prose.host.spawn(
            "work".to_owned(),
            Map::from_iter([("name".to_owned(), Value::String("prose".to_owned()))]),
        )?;
        assert!(wait_done(&prose.host).await, "child never completed");
        let refused = prose
            .host
            .result("prose", Some(&schema))
            .err()
            .ok_or("prose was admitted as JSON")?;
        assert!(refused.contains("text, not JSON"), "{refused}");
        Ok(())
    }

    /// The engine submits a child's finish (D225): a brief that named `submit` would have the
    /// child race the engine for the one record.
    #[test]
    fn a_brief_never_names_submit() -> TestResult {
        let at = TodoAddr {
            plan: PlanId::new("ship-the-thing")?,
            todo: TodoLabel::new("gateway")?,
        };
        let mut delegation = delegated("gateway")?.delegation.ok_or("delegated")?;
        for isolation in [None, Some(Isolation::None), Some(Isolation::Worktree)] {
            delegation.spec.isolation = isolation;
            let brief = brief(&at, &delegation);
            assert!(brief.contains("Execute todo \"gateway\""), "{brief}");
            assert!(!brief.contains("submit"), "{brief}");
            assert!(
                brief.ends_with("the engine takes it as your work."),
                "{brief}"
            );
        }
        Ok(())
    }

    /// Dies with the brief dropping the stored note: the child reads the head alone.
    #[test]
    fn an_over_cap_note_is_linked_from_the_brief() -> TestResult {
        let at = TodoAddr {
            plan: PlanId::new("ship-the-thing")?,
            todo: TodoLabel::new("quota")?,
        };
        let mut delegation = delegated("quota")?.delegation.ok_or("delegated")?;
        let hex = "ab".repeat(32);
        delegation.extra.insert(
            super::super::declare::NOTE_REF.to_owned(),
            serde_json::json!({"digest": format!("sha256:{hex}"), "media_type": "text/markdown", "length": 2000}),
        );
        let brief = brief(&at, &delegation);
        assert!(
            brief.contains(&format!("read plan://ship-the-thing/artifacts/{hex}")),
            "{brief}"
        );
        Ok(())
    }

    /// Dies with `busy` reading only the children's exits: the child has ended while the engine
    /// still settles its finish, and `yi ask` would end the run before the acceptance lands.
    #[tokio::test]
    async fn the_host_stays_busy_until_the_finish_settles() -> TestResult {
        let rig = hooked(vec![reply("the seam is cut and holds")])?;
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        let label = TodoLabel::new("cut the seam")?;
        for _ in 0..400 {
            let busy = rig.host.busy();
            let read = rig.engine.store().read(&out.plan.id)?;
            let running = matches!(
                read.todo(&label).map(|todo| &todo.state),
                Some(TodoState::Running { .. })
            );
            assert!(busy || !running, "idle while the todo is still running");
            if !busy {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        Err("the host never went idle".into())
    }

    /// Dies with the cursor dropped: a wait with none read epoch 0 and returned at once, so a
    /// parent looping on `rlm.wait(300)` spun and the repeat breaker ended its session.
    #[tokio::test]
    async fn a_wait_with_no_cursor_blocks_until_something_moves() -> TestResult {
        use yi_kernel::client::HostHandlers;
        let rig = rig("the seam is cut")?;
        let mut registry = crate::kernel::HostRegistry::default();
        rig.host.register(&mut registry);
        rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
        }))?;
        assert!(wait_done(&rig.host).await, "child never completed");
        let payload = serde_json::json!({"timeout_ms": 1000});
        let payload = payload.as_object().cloned().ok_or("payload")?;
        let started = std::time::Instant::now();
        registry
            .dispatch("rlm.wait", payload.clone())
            .ok_or("rlm.wait")?
            .await?;
        assert!(
            started.elapsed().as_millis() < 900,
            "the first wait sees the family now"
        );
        let started = std::time::Instant::now();
        registry
            .dispatch("rlm.wait", payload)
            .ok_or("rlm.wait")?
            .await?;
        assert!(
            started.elapsed().as_millis() >= 900,
            "nothing moved, so it waited"
        );
        Ok(())
    }

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

    /// Dies with the budget block in `kwargs_of`: leave it out of the kwargs and the engine's
    /// own spawn road mints tokens the parent never held, whatever `rlm.run` is refused.
    #[test]
    fn kwargs_draw_the_specs_budget_as_the_childs_lease() -> TestResult {
        let mut delegation = delegated("read the docs")?.delegation.ok_or("delegated")?;
        delegation.spec.budget = Some(yi_types::plan::doc::TokenBudget(4_000));
        let kwargs = kwargs_of(&AgentId::new("reader-1")?, &delegation)?;
        assert_eq!(kwargs["tokens"], serde_json::json!(4_000));
        let ask = crate::lease::Ask::from_kwargs(&kwargs)?;
        assert_eq!(
            ask.tokens,
            Some(4_000),
            "the host reads the key it is handed"
        );
        delegation.spec.budget = None;
        assert!(!kwargs_of(&AgentId::new("reader-2")?, &delegation)?.contains_key("tokens"));
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
        let out = rig.engine.apply(owner(Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos: vec![delegated("cut the seam")?],
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
