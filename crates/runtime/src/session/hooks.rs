//! The session's handle vocabulary: every closure the runtime hands a subsystem so it can
//! steer, notice or read this session without holding it. The run loop stays out.

use std::sync::{Arc, PoisonError};

use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Usage};
use yi_types::model::{Effort, Model};
use yi_types::wire::SessionStats;

use super::run::{self, Queued};
use super::{
    AgentSession, ExtHook, Shared, Status, StillNews, attribute_to_shared, dispatch_ext, host_text,
    record_store_error, store_of,
};

pub(super) type LedgerFold = dyn FnMut(&str, &[Entry], &SessionStats) + Send;

pub(super) fn settings_of(shared: &Shared) -> (Model, Effort) {
    let model = shared
        .model
        .lock()
        .map(|model| model.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
    let effort = shared
        .effort
        .lock()
        .map(|effort| *effort)
        .unwrap_or_else(|poisoned| *poisoned.into_inner());
    (model, effort)
}

/// Incident: the kernel registry holds the heartbeat service, whose lane holds `shared`: no drop
/// broke that cycle, so an ended session's timer rewrote its ledger over the next holder's edits.
impl Drop for AgentSession {
    fn drop(&mut self) {
        if let Some(service) = self.heartbeat_service() {
            service.end();
        }
    }
}

impl AgentSession {
    /// Invariant: a resumed session's state is a fold of its ledger, never a count from zero;
    /// `fold` reads each store attached after this call (id, branch, totals) before a request.
    pub(crate) fn on_attach(
        &self,
        fold: impl FnMut(&str, &[Entry], &SessionStats) + Send + 'static,
    ) {
        let mut folds = self
            .on_attach
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        folds.push(Box::new(fold));
    }

    pub(super) fn restore_from_ledger(&self, entries: &[Entry]) {
        let mut effort = None;
        for entry in entries {
            match entry {
                Entry::ModelChange {
                    provider, model_id, ..
                } => {
                    let Some(model) = crate::provider::resolve_model(provider, model_id) else {
                        record_store_error(
                            &self.shared,
                            &yi_session::SessionError::InvalidPayload(format!(
                                "the ledger switched to {provider}/{model_id}, which the catalog no longer resolves"
                            )),
                        );
                        continue;
                    };
                    if let Ok(mut slot) = self.shared.model.lock() {
                        *slot = model;
                    }
                }
                Entry::ThinkingLevelChange { thinking_level, .. } => {
                    match thinking_level.parse::<Effort>() {
                        Ok(level) => effort = Some(level),
                        Err(_) => record_store_error(
                            &self.shared,
                            &yi_session::SessionError::InvalidPayload(format!(
                                "the ledger's thinking level {thinking_level} did not parse"
                            )),
                        ),
                    }
                }
                _ => {}
            }
        }
        let restored = self.model().clamp_effort(effort.unwrap_or(self.effort()));
        if let Ok(mut slot) = self.shared.effort.lock() {
            *slot = restored;
        }
        let (id, stats) = store_of(&self.shared).map_or_else(
            || (String::new(), SessionStats::zero()),
            |store| {
                let session = yi_session::lock_session(&store);
                (session.metadata().id.clone(), session.stats())
            },
        );
        let mut folds = self
            .on_attach
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for fold in folds.iter_mut() {
            fold(&id, entries, &stats);
        }
    }

    pub fn abort(&self) {
        self.shared.signal.fire();
    }

    /// The interrupt's epoch, read when a run is requested for [`Self::prompt_requested`].
    pub fn abort_epoch(&self) -> u64 {
        self.shared.signal.epoch()
    }

    pub async fn wait_idle(&self) {
        super::until(&self.shared.idle, || match self.status() {
            Status::Idle => std::ops::ControlFlow::Break(()),
            _ => std::ops::ControlFlow::Continue(None),
        })
        .await;
    }

    pub fn ext_hook(&self) -> ExtHook {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |event| dispatch_ext(&shared, &event))
    }

    /// Read per call: the engine attaches after the heartbeat service is wired.
    pub fn rules_handle(
        &self,
    ) -> Arc<dyn Fn() -> Option<Arc<crate::rules::RuleEngine>> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || shared.rules.lock().ok().and_then(|slot| slot.clone()))
    }

    pub fn store_handle(
        &self,
    ) -> std::sync::Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        std::sync::Arc::new(move || store_of(&shared))
    }

    /// The environment block as the host would render it now, for checks that count its facts.
    pub fn environment_handle(&self) -> Arc<dyn Fn() -> Option<String> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            let hook = shared
                .environment
                .lock()
                .ok()
                .and_then(|slot| slot.clone())?;
            hook()
        })
    }

    /// Incident: snapshotting these left `rlm.run` and `model.info` on the
    /// startup model once the TUI could switch.
    pub fn kernel_state_handle(&self) -> Arc<dyn Fn() -> Option<String> + Send + Sync> {
        let kernel = Arc::clone(&self.kernel);
        Arc::new(move || {
            kernel
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(|service| service.state()))
        })
    }

    pub fn settings_handle(&self) -> Arc<dyn Fn() -> (Model, Effort) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || settings_of(&shared))
    }

    /// A steer wakes an idle session through the one queue; a follow-up waits for a turn's answer.
    pub fn heartbeat_hook(
        &self,
    ) -> Arc<dyn Fn(AgentMessage, yi_types::schedule::DeliveryMode) + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |message, mode| match mode {
            yi_types::schedule::DeliveryMode::Steer => {
                run::enqueue(&parts, Queued::new(message, true, None));
            }
            yi_types::schedule::DeliveryMode::FollowUp => {
                run::follow(&parts, message, None);
            }
        })
    }

    pub fn deliver_hook(&self) -> Arc<dyn Fn(AgentMessage, bool) + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |message, wakes| {
            run::enqueue(&parts, Queued::new(message, wakes, None));
        })
    }

    /// Delivers each text `observe` returns as a shown notice of `custom_type`, never a wake:
    /// a notice must not buy another turn.
    pub fn show_notices(
        &self,
        custom_type: &'static str,
        mut observe: impl FnMut(&yi_types::event::AgentEvent) -> Option<String> + Send + 'static,
    ) {
        let mut events = self.subscribe();
        let deliver = self.deliver_hook();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if let Some(text) = observe(&event) {
                            let notice =
                                AgentMessage::host_note(custom_type, text, yi_session::now_ms());
                            deliver(notice, false);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    pub fn mail_hook(&self) -> Arc<dyn Fn() -> Vec<serde_json::Value> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            let taken = shared
                .steer
                .lock()
                .map(|mut queue| run::take_mail(&mut queue));
            crate::mail::received(store_of(&shared), taken.unwrap_or_default())
        })
    }

    /// Notified whenever a message joins the steer queue, for a wait on [`Self::mail_hook`].
    pub fn mail_arrived(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.shared.mail)
    }

    /// Incident: under a client's cell every finished child woke the idle root, a paid model
    /// turn for news the cell was about to read itself; while one runs, the notice only queues.
    pub fn wake_idle_hook(&self) -> Arc<dyn Fn(AgentMessage, Option<StillNews>) + Send + Sync> {
        let parts = self.parts();
        let kernel = Arc::clone(&self.kernel);
        Arc::new(move |message, news| {
            let cell = kernel
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .is_some_and(|service| service.user_cell_running());
            run::enqueue(&parts, Queued::new(message, !cell, news));
        })
    }

    /// Running ⇒ steer queue (next tool boundary); idle ⇒ follow-up queue. The
    /// advisor never wakes an idle primary.
    pub fn advisory_hook(&self) -> Arc<dyn Fn(AgentMessage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |message| {
            let Ok(status) = shared.status.lock() else {
                return;
            };
            if *status == Status::Running {
                if let Ok(mut queue) = shared.steer.lock() {
                    run::push(&mut queue, Queued::new(message, false, None));
                }
            } else if let Ok(mut queue) = shared.follow_up.lock() {
                queue.push(Queued::new(message, false, None));
            }
        })
    }

    pub fn history_handle(&self) -> Arc<dyn Fn() -> Vec<AgentMessage> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            shared
                .messages
                .lock()
                .map(|messages| messages.clone())
                .unwrap_or_default()
        })
    }

    /// Summed under the lock, not over a history clone; `None` when poisoned, never a fake `0.0`.
    /// The flag is true when a reply came back without usage, so the sum is a lower bound.
    pub fn cost_handle(&self) -> Arc<dyn Fn() -> Option<(f64, bool)> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            let messages = shared.messages.lock().ok()?;
            let usages = messages.iter().filter_map(|message| match message {
                AgentMessage::Assistant { usage, .. } => Some(usage),
                _ => None,
            });
            Some(usages.fold((0.0, false), |(cost, lower_bound), usage| {
                let spent = usage.cost.total.as_f64().unwrap_or(0.0);
                (cost + spent, lower_bound || usage.unknown)
            }))
        })
    }

    pub fn usage_handle(&self) -> Arc<dyn Fn() -> Option<Usage> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            shared
                .last_usage
                .lock()
                .ok()
                .and_then(|usage| usage.clone())
        })
    }

    pub fn activity_handle(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            shared
                .status
                .lock()
                .map(|status| *status == Status::Running)
                .unwrap_or(false)
        })
    }

    pub fn halt_hook(&self) -> Arc<dyn Fn(bool) -> bool + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |on| {
            let held = &parts.shared.held;
            let was = held.swap(on, std::sync::atomic::Ordering::SeqCst);
            if !on {
                if was {
                    run::kick(&parts);
                }
                return was;
            }
            let running = parts
                .shared
                .status
                .lock()
                .is_ok_and(|status| *status == Status::Running);
            parts.shared.signal.fire();
            running
        })
    }

    pub(crate) fn claude_hooks(
        &self,
        cwd: &std::path::Path,
    ) -> Option<Arc<crate::claude_hooks::Hooks>> {
        let broker = self.permission.lock().ok().and_then(|slot| slot.clone());
        match broker {
            Some(broker) => broker.claude_hooks(),
            None => self
                .claude_hooks
                .get_or_init(|| crate::claude_hooks::Hooks::load(cwd))
                .clone(),
        }
    }

    /// Invariant: an idle queue holds inboxed notices and held follow-ups waiting for another
    /// turn, so only a queue behind a running turn is pending work that defers a heartbeat.
    pub fn heartbeat_deliverer(&self) -> Arc<crate::schedule::DeliverFn> {
        let shared = Arc::clone(&self.shared);
        let compactor = self.compactor.clone();
        let cwd = self.tool_cwd.lock().ok().and_then(|slot| slot.clone());
        let hooks = self.claude_hooks(cwd.as_deref().unwrap_or_else(|| std::path::Path::new(".")));
        let (hook, quiet) = (self.heartbeat_hook(), self.deliver_hook());
        Arc::new(move |job, firing| {
            let is_streaming = shared
                .status
                .lock()
                .is_ok_and(|status| *status == Status::Running);
            let queued = shared.steer.lock().is_ok_and(|queue| !queue.is_empty())
                || shared.follow_up.lock().is_ok_and(|queue| !queue.is_empty());
            let activity = crate::schedule::SessionActivity {
                is_streaming,
                is_compacting: compactor.as_ref().is_some_and(|c| c.compacting()),
                has_pending_session_work: is_streaming && queued,
            };
            if crate::schedule::should_defer(job, &activity) {
                return Ok(crate::schedule::RunOutcome::Deferred);
            }
            let todos = shared
                .todos
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(Arc::clone))
                .ok_or("a tick creates a todo, and this session has no todo list")?;
            let fired =
                crate::schedule::clock::fire_with_hooks(&todos, job, firing, hooks.as_ref())?;
            let outcome = crate::schedule::clock::outcome(&fired);
            if let Some(wake) =
                crate::schedule::clock::wake_message(job, &fired, yi_session::now_ms())
            {
                let mode = job
                    .delivery_mode
                    .unwrap_or(crate::schedule::DEFAULT_HEARTBEAT_DELIVERY_MODE);
                match crate::post::rings(job, &wake) {
                    true => hook(wake, mode),
                    false => quiet(wake, false),
                }
            }
            Ok(outcome)
        })
    }

    /// Job reports (§4.3) start a turn on an idle session; false once retired, ending the loop.
    /// Each carries its `news`: a report already read by a wait on the job is dropped unread.
    pub fn job_report_hook(&self) -> Arc<super::run::JobReportFn> {
        let parts = self.parts();
        Arc::new(move |reports: Vec<(String, StillNews)>| {
            if parts
                .shared
                .retired
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return false;
            }
            for (text, news) in reports {
                run::follow(
                    &parts,
                    host_text(yi_types::message::HostSource::Job, &text),
                    Some(news),
                );
            }
            true
        })
    }

    /// Incident: the kernel pump's monitor task owns the tokio Child, so a dropped session
    /// leaks its IPython process. Every path that retires a session calls this.
    pub fn retire(&self) {
        self.shared
            .retired
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let Some(kernel) = self.kernel_service() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move { kernel.dispose().await });
        }
    }

    pub fn job_owner(&self) -> yi_tools::jobs::JobOwner {
        self.shared.job_owner
    }

    pub fn wait_hook(&self) -> Arc<crate::compaction::WaitFn> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |wait| {
            let _ = shared
                .events
                .send(yi_types::event::AgentEvent::Wait { wait });
        })
    }

    pub fn notice_hook(
        &self,
        source: yi_types::message::HostSource,
    ) -> Arc<dyn Fn(&str) + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |text: &str| {
            run::enqueue(&parts, Queued::new(host_text(source, text), false, None));
        })
    }

    /// The model snapshot is taken now: a later [`AgentSession::set_model`] leaves percent
    /// computed against the old window until re-wired.
    pub fn compact_status_handle(
        &self,
    ) -> Option<Arc<dyn Fn() -> crate::compaction::CompactStatus + Send + Sync>> {
        let compactor = self.compactor.clone()?;
        let shared = Arc::clone(&self.shared);
        let model = self.model();
        Some(Arc::new(move || match shared.messages.lock() {
            Ok(messages) => compactor.status(&messages, &model),
            Err(_) => compactor.status(&[], &model),
        }))
    }

    /// Usable after the session moves — the hook holds only shared state.
    pub fn attribution_handle(&self) -> Arc<dyn Fn(&Usage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |child: &Usage| attribute_to_shared(&shared, child))
    }

    /// A turn's usage settled after it ended is a `settled` main-lane row with its upstream;
    /// a child's also bills its parent, as its entries do.
    pub(crate) fn settle_hook(
        &self,
        parent: Option<Arc<crate::subagent::AttributeFn>>,
    ) -> yi_ai::settle::Settle {
        let shared = Arc::clone(&self.shared);
        yi_ai::settle::Settle(Arc::new(move |usage: Usage, upstream: Option<String>| {
            if let Some(store) = store_of(&shared) {
                let details = upstream.map(|name| serde_json::json!({"provider": name}));
                let booked = crate::spend::append_main_usage(
                    &store,
                    "settled".to_owned(),
                    usage.clone(),
                    details,
                );
                if let Err(error) = booked {
                    record_store_error(&shared, &error);
                }
            }
            if let Some(attribute) = &parent {
                attribute(&usage);
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use yi_types::message::Usage;
    use yi_types::model::Effort;
    use yi_types::record::LaneRecord;

    fn settle_test_config() -> crate::session::SessionConfig {
        crate::session::SessionConfig {
            system_prompt: String::new(),
            model: crate::plan::dispatch::tests::faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        }
    }

    fn store(id: &str) -> yi_session::SharedSession {
        Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
            yi_session::SessionMetadata {
                id: id.to_owned(),
                created_at: 0,
                parent_session_id: None,
                name: None,
            },
        )))
    }

    fn wiring(
        provider: &Arc<crate::provider::ProviderStream>,
        root: &std::path::Path,
        parent_link: Option<crate::ParentLink>,
    ) -> crate::RuntimeWiring {
        crate::RuntimeWiring {
            provider: Arc::clone(provider),
            system_prompt: String::new(),
            tool_execution: yi_loop::ExecutionMode::Sequential,
            cwd: root.join("cwd"),
            home: root.join("home"),
            lane_slots: 1,
            broker: None,
            tools: Arc::new(Vec::new),
            depth: u8::from(parent_link.is_some()),
            max_depth: 1,
            rlm_dir: root.join("rlm"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link,
            wall: crate::wall::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: crate::fetch::KernelServiceMap::new(),
        }
    }

    /// Dies with the hook unwired: a child's cut turn settled after it ended reached neither its
    /// own ledger nor the parent's bill.
    #[test]
    fn a_settled_usage_lands_on_the_ledger_and_bills_the_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = Arc::new(crate::provider::ProviderStream::new(None));
        let session = super::AgentSession::new(settle_test_config(), provider);
        let store = store("settled");
        session.attach_store(Arc::clone(&store))?;
        let billed = Arc::new(Mutex::new(Vec::new()));
        let bill = Arc::clone(&billed);
        let parent: Arc<crate::subagent::AttributeFn> = Arc::new(move |usage: &Usage| {
            if let Ok(mut seen) = bill.lock() {
                seen.push(usage.clone());
            }
        });
        let mut usage = Usage::zero();
        usage.output = 3089;
        (session.settle_hook(Some(parent)).0)(usage.clone(), Some("Z.AI".to_owned()));
        let records = yi_session::lock_session(&store).find_records(&Default::default())?;
        let booked = records.iter().find_map(|record| match record {
            LaneRecord::Usage {
                cause,
                usage,
                details,
                ..
            } if cause == "settled" => Some((usage.output, details.clone())),
            _ => None,
        });
        assert_eq!(
            booked,
            Some((3089, Some(serde_json::json!({"provider": "Z.AI"}))))
        );
        let seen = billed.lock().map_err(|_| "poisoned")?;
        assert_eq!(
            seen.iter().map(|usage| usage.output).collect::<Vec<_>>(),
            [3089]
        );
        Ok(())
    }

    /// A reader or worker child never reaches attach_runtime, so `new` itself must wire the
    /// hook: unwired, its cut OpenRouter turn settles inline and stalls the child.
    #[test]
    fn a_session_built_without_attach_runtime_still_carries_a_settle_hook() {
        let provider = Arc::new(crate::provider::ProviderStream::new(None));
        super::AgentSession::new(settle_test_config(), Arc::clone(&provider));
        assert!(provider.settle_wired_for_test().is_some());
    }

    /// Dies with the parent bill wired only in attach_runtime: a reader or worker child never
    /// reaches it, so its settled cost booked on its own ledger and the parent's total missed it.
    #[test]
    fn every_child_bills_its_settled_usage_to_its_parent() -> Result<(), Box<dyn std::error::Error>>
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let _reactor = runtime.enter();
        let dir = std::env::temp_dir().join(format!("yi-settle-rewire-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("cwd"))?;
        std::fs::create_dir_all(dir.join("home"))?;
        let root_stream = Arc::new(crate::provider::ProviderStream::new(None));
        let mut root = super::AgentSession::new(settle_test_config(), Arc::clone(&root_stream));
        let ledger = store("root");
        root.attach_store(Arc::clone(&ledger))?;
        let host = crate::attach_runtime(&mut root, wiring(&root_stream, &dir, None));
        let lease = yi_types::lease::Lease {
            holder: String::new(),
            parent: String::new(),
            deadline_ms: None,
            tokens: None,
            granted_at: 0,
            revoked: None,
        };
        for (output, role) in [(1, "root"), (2, "reader"), (3, "worker")] {
            let kwargs = serde_json::json!({ "role": role });
            let reader = crate::subagent::reader::parse(kwargs.as_object().ok_or("kwargs")?)?;
            let model = settle_test_config().model;
            let cast = (model, Effort::Low, crate::wall::Wall::default(), reader);
            let child = host.build(cast, role, &dir.join(role), None, &lease, None)?;
            let hook = child.provider().settle_wired_for_test().ok_or("no hook")?;
            let mut usage = Usage::zero();
            usage.output = output;
            (hook.0)(usage, None);
        }
        let records = yi_session::lock_session(&ledger).find_records(&Default::default())?;
        let mut billed: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                LaneRecord::Usage { cause, usage, .. }
                    if cause == yi_context::attribution::CHILD_USAGE_CAUSE =>
                {
                    Some(usage.output)
                }
                _ => None,
            })
            .collect();
        billed.sort_unstable();
        let _cleanup = std::fs::remove_dir_all(&dir);
        assert_eq!(
            billed,
            [1, 2, 3],
            "a child's settled usage missed the parent"
        );
        Ok(())
    }

    /// Dies with the hook overwritten: an auto-review session built on the root's stream took
    /// the slot, and with no store of its own it dropped the root's settled cost.
    #[test]
    fn a_side_session_on_the_root_stream_keeps_the_root_hook()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = Arc::new(crate::provider::ProviderStream::new(None));
        let root = super::AgentSession::new(settle_test_config(), Arc::clone(&provider));
        let store = store("root");
        root.attach_store(Arc::clone(&store))?;
        let _reviewer = super::AgentSession::new(settle_test_config(), Arc::clone(&provider));
        let hook = provider.settle_wired_for_test().ok_or("no hook wired")?;
        let mut usage = Usage::zero();
        usage.output = 3089;
        (hook.0)(usage, None);
        let records = yi_session::lock_session(&store).find_records(&Default::default())?;
        let settled = records.iter().any(|record| {
            matches!(record, LaneRecord::Usage { cause, usage, .. }
                if cause == "settled" && usage.output == 3089)
        });
        assert!(settled, "the root's settled usage was dropped");
        Ok(())
    }
}
