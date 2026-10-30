use std::sync::Arc;

use yi_types::message::AgentMessage;
use yi_types::plan::doc::{AgentId, Delegation, TodoAddr, TodoLabel};
use yi_types::schedule::DeliveryMode;
use yi_types::url::Url;

use super::ops::{Delegate, Delta, PlanEngine, PlanOpError, Txn, agent_url};
use super::recovery::Liveness;
use super::spawn::{brief, child_name, kwargs_of};
use super::state::{KIND_SPAWN_INTENT, KIND_SPAWN_RESULT};
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

fn parse_url(rendered: String) -> Result<Url, String> {
    rendered
        .parse::<Url>()
        .map_err(|error| format!("{rendered}: {error}"))
}

pub(super) fn say(deliver: &DeliverFn, text: String) {
    deliver(
        AgentMessage::host_note("plan_relevance", text, yi_session::now_ms()),
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
        let artifacts = self.store.artifacts(id);
        let inlined = (super::spawn::reads_only(&delegation, &self.cwd))
            .then(|| super::spawn::schema(&artifacts, todo, self.output_resolve.as_deref()))
            .flatten();
        let told = super::brief::lines(&artifacts, &txn.records, id, todo, inlined.as_ref());
        if !told.is_empty() {
            delegation
                .extra
                .insert(super::brief::KEY.to_owned(), serde_json::json!(told));
        }
        if let Some(schema) = inlined {
            delegation
                .extra
                .insert(super::spawn::SCHEMA.to_owned(), schema);
        }
        let addr = TodoAddr {
            plan: id.clone(),
            todo: label.clone(),
        };
        let url = agent_url(&addr)?;
        if let Some((effect, intent)) = txn.state.intent_for(id, label) {
            if self.stranded(&intent.outcome) {
                return Err(PlanOpError::NeedsReconciliation {
                    label: label.clone(),
                    effect: effect.clone(),
                });
            }
            delta.spawned.push(url);
            return Ok(());
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
        let cwd = &self.host.options.cwd;
        let reader = super::spawn::reads_only(delegation, cwd);
        let kwargs = kwargs_of(&agent, delegation, reader)?;
        // Plan section 7.6: a brief whose own wall denies its context is refused here, with
        // the denial as evidence, not one fetch later inside a child that cannot do its job.
        let wall = self.host.wall_for(&kwargs)?;
        if let Some(denied) = delegation
            .context
            .iter()
            .find_map(|url| wall.check_url(url, cwd))
        {
            return Err(format!("the brief names context its wall denies. {denied}"));
        }
        self.host.spawn(brief(at, delegation, reader), kwargs)?;
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
pub(crate) mod tests {
    use super::*;
    use crate::plan::rig::*;
    use serde_json::Value;
    use yi_types::backoff::backoff;
    use yi_types::message::{StopReason, UserContent};
    use yi_types::plan::doc::{Check, GoalText};
    use yi_types::url::Durability;

    use crate::plan::ops::Op;
    use crate::scratch::Scratch;
    use crate::subagent::SubagentHostOptions;

    #[tokio::test]
    async fn a_ready_todo_dispatches_a_child_and_reap_names_its_transcript() -> TestResult {
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
        let child = format!("{}/cut-the-seam", out.plan.id.as_str());
        assert_eq!(
            texts(&rig.reports),
            [format!(
                "reaped {child} (completed): its transcript is history://{child}"
            )],
            "the reap names where the product lives, not the product"
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
        let said = texts(&rig.said);
        assert_eq!(
            said,
            [format!(
                "plan: failed \"write the patch\": the provider hung up; its last product is {last}"
            )],
            "the verdict names the last product's address"
        );
        assert!(texts(&rig.reports).is_empty(), "and no second reap line");
        Ok(())
    }

    /// Dies with the finish skipping the delegation's accept command (finish.rs `red_check`):
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
        let said = texts(&rig.said);
        assert_eq!(
            said,
            [format!(
                "plan: accepted \"cut the seam\" (agent://{id}/cut-the-seam); its product is {output}"
            )],
            "the verdict names its product's address"
        );
        let journal = rig.engine.store().journal(&id).read()?.records;
        let actors: Vec<(&str, &str)> = journal
            .iter()
            .filter(|record| ["submit", "done"].contains(&record.record.op.as_str()))
            .map(|record| (record.record.op.as_str(), record.record.actor.as_str()))
            .collect();
        assert_eq!(actors, [("submit", "engine"), ("done", "engine")]);
        assert!(
            texts(&rig.reports).is_empty(),
            "the done reaps the child and says so once, on the verdict"
        );
        let notices = rig.notices.lock().map_err(|_| "poisoned")?.clone();
        assert!(
            notices.iter().all(|notice| !notice.contains("[subagent")),
            "a child the engine took sends no second notice: {notices:?}"
        );
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
        rig.host.spawn("work".to_owned(), worker("quota"))?;
        let notice = wait_notice(&rig, "[subagent quota").await?;
        assert!(notice.contains("the quota parser lands"), "{notice}");
        assert!(
            texts(&rig.said).is_empty(),
            "no plan line for a child no todo runs"
        );
        Ok(())
    }

    async fn wait_notice(rig: &Rig, needle: &str) -> Result<String, Box<dyn std::error::Error>> {
        let mut pace = backoff(std::time::Duration::from_millis(25));
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
            tokio::time::sleep(pace()).await;
        }
        Err(format!("no notice with {needle:?}").into())
    }

    /// Incident: nine of twelve F0e "text, not JSON" refusals were a valid object inside a
    /// fenced block, and the parent had no road past them (#475).
    #[tokio::test]
    async fn a_fenced_json_answer_validates_against_a_schema() -> TestResult {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"outcome": {"type": "string"}},
            "required": ["outcome"],
        });
        let object = "```json\n{\"outcome\": \"the quota parser lands\"}\n```";
        // The #989 cases: a reader's fence after a non-ASCII paragraph, and a fence after
        // prose that is not JSON, which stays refused with its text attached.
        let cases: [(&'static str, Result<&str, &str>); 6] = [
            (object, Ok("the quota parser lands")),
            (
                "the prose quotes {\"outcome\": \"the quoted one\"} before [pkg/more.py#F0C2]\n\n```json\n{\"outcome\": \"the quota parser lands\"}\n```",
                Ok("the quota parser lands"),
            ),
            ("the quota parser lands", Err("text, not JSON")),
            (
                "Voil\u{e0} la r\u{e9}ponse:\n\n```json\n{\"outcome\": \"the quota parser lands\"}\n```\n",
                Ok("the quota parser lands"),
            ),
            (
                "Voil\u{e0} la r\u{e9}ponse:\n```\nnot json\n```\n",
                Err("text, not JSON"),
            ),
            ("```json\n{\"outcome\": 5}\n```", Err("result rejected")),
        ];
        for (answer, expected) in cases {
            let rig = rig(answer)?;
            rig.host.spawn("work".to_owned(), worker("q"))?;
            assert!(wait_done(&rig.host).await, "child never completed");
            match (rig.host.result("q", Some(&schema)), expected) {
                (Ok(reply), Ok(outcome)) => {
                    assert_eq!(reply["json"]["outcome"], Value::String(outcome.to_owned()));
                }
                (Err(refused), Err(reason)) => assert!(refused.contains(reason), "{refused}"),
                (reply, expected) => {
                    return Err(format!("{answer}: {reply:?}, expected {expected:?}").into());
                }
            }
        }
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
        stirred_until(&rig.host, || {
            let busy = rig.host.busy();
            let read = rig.engine.store().read(&out.plan.id)?;
            let running = matches!(
                read.todo(&label).map(|todo| &todo.state),
                Some(TodoState::Running { .. })
            );
            assert!(busy || !running, "idle while the todo is still running");
            Ok(!busy)
        })
        .await
    }

    /// Dies with the cursor dropped: a wait with none read epoch 0 and reported the finished
    /// child again, so a parent looping on `rlm.wait(300)` saw news that was not new. With
    /// nothing live, the second wait at that same state refuses and names it (M4).
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
        let again = registry
            .dispatch("rlm.wait", payload)
            .ok_or("rlm.wait")?
            .await;
        assert_eq!(
            again.err().as_deref(),
            Some("the family is settled: nothing is running; stop waiting")
        );
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
            provider: Arc::new(crate::provider::ProviderStream::new(None)),
            depth: 0,
            max_depth: 1,
            max_children: 8,
            parent_session_dir: root.join("children"),
            plans_dir: root.join(crate::plan::PLANS_DIR),
            family_live: crate::fetch::KernelServiceMap::new(),
            cwd: root.to_path_buf(),
            home: std::env::temp_dir(),
            lane_slots: 1,
            defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
            factory: Arc::new(|_build| Ok(faux_session(&["the child's answer"]))),
            notice,
            events,
            parent_messages: Arc::new(Vec::new),
            report: Arc::new(|_message, _| {}),
            attribute: Arc::new(|_usage| {}),
            store: Arc::new(|| None),
        }));
        host.spawn("answer once".to_owned(), worker("helper"))
            .map_err(|error| error.to_string())?;
        let mut woke = false;
        let mut pace = backoff(std::time::Duration::from_millis(25));
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
            tokio::time::sleep(pace()).await;
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
