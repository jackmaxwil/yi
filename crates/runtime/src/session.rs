use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use yi_loop::interrupt::InterruptSignal;
use yi_loop::{ExecutionMode, LoopConfig, LoopContext, run_loop};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Usage, UserContent};
use yi_types::model::Model;

use crate::provider::ProviderStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Running,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionError {
    Busy,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("a run is already in progress"),
        }
    }
}

impl std::error::Error for SessionError {}

pub struct SessionConfig {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: Option<String>,
    pub tool_execution: ExecutionMode,
}

struct Shared {
    messages: Mutex<Vec<AgentMessage>>,
    steer: Mutex<Vec<AgentMessage>>,
    follow_up: Mutex<Vec<AgentMessage>>,
    status: Mutex<Status>,
    last_usage: Mutex<Option<Usage>>,
    store: Mutex<Option<yi_session::SharedSession>>,
    store_error: Mutex<Option<String>>,
    events: broadcast::Sender<AgentEvent>,
    idle: tokio::sync::Notify,
    signal: InterruptSignal,
    on_turn_start: Mutex<Option<Arc<TurnHook>>>,
    on_turn_end: Mutex<Option<Arc<TurnHook>>>,
}

/// The start hook captures the tree the turn is about to change, the end hook
/// what it left behind.
pub type TurnHook = dyn Fn() + Send + Sync;

fn persist_message(shared: &Shared, message: &AgentMessage) {
    let store = shared
        .store
        .lock()
        .map(|handle| handle.clone())
        .unwrap_or_default();
    if let Some(store) = store
        && let Err(error) = yi_session::lock_session(&store).append_message("main", message.clone())
        && let Ok(mut slot) = shared.store_error.lock()
    {
        *slot = Some(error.to_string());
    }
}

#[derive(Clone)]
struct RunParts {
    shared: Arc<Shared>,
    provider: Arc<ProviderStream>,
    system_prompt: String,
    model: Model,
    tool_execution: ExecutionMode,
    tools: Vec<Arc<dyn yi_loop::AgentTool>>,
    compactor: Option<Arc<crate::compaction::Compactor>>,
    on_compacted: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub struct AgentSession {
    config: SessionConfig,
    model: Mutex<Model>,
    provider: Arc<ProviderStream>,
    shared: Arc<Shared>,
    tools: Vec<Arc<dyn yi_loop::AgentTool>>,
    compactor: Option<Arc<crate::compaction::Compactor>>,
    on_compacted: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    schedule: Mutex<Option<ScheduleParts>>,
    advisor: Mutex<Option<Arc<crate::advisor::AdvisorRuntime>>>,
    permission: Mutex<Option<Arc<crate::permission::PermissionBroker>>>,
    goal: Mutex<Option<Arc<crate::goal::GoalService>>>,
    plan: Mutex<Option<Arc<crate::plan::PlanService>>>,
    rules: Mutex<Option<Arc<crate::rules::RuleEngine>>>,
}

type ScheduleParts = (
    Arc<crate::schedule::JobStore>,
    Arc<crate::schedule::HeartbeatService>,
    crate::schedule::Scheduler,
);

impl AgentSession {
    pub fn new(config: SessionConfig, provider: Arc<ProviderStream>) -> Self {
        provider.set_thinking_level(config.thinking_level.clone());
        let (events, _) = broadcast::channel(1024);
        Self {
            model: Mutex::new(config.model.clone()),
            config,
            provider,
            shared: Arc::new(Shared {
                messages: Mutex::new(Vec::new()),
                steer: Mutex::new(Vec::new()),
                follow_up: Mutex::new(Vec::new()),
                status: Mutex::new(Status::Idle),
                last_usage: Mutex::new(None),
                store: Mutex::new(None),
                store_error: Mutex::new(None),
                events,
                idle: tokio::sync::Notify::new(),
                signal: InterruptSignal::default(),
                on_turn_start: Mutex::new(None),
                on_turn_end: Mutex::new(None),
            }),
            tools: Vec::new(),
            compactor: None,
            on_compacted: Mutex::new(None),
            schedule: Mutex::new(None),
            advisor: Mutex::new(None),
            permission: Mutex::new(None),
            goal: Mutex::new(None),
            plan: Mutex::new(None),
            rules: Mutex::new(None),
        }
    }

    pub fn set_turn_start_hook(&self, hook: Arc<TurnHook>) {
        if let Ok(mut slot) = self.shared.on_turn_start.lock() {
            *slot = Some(hook);
        }
    }

    pub fn set_turn_end_hook(&self, hook: Arc<TurnHook>) {
        if let Ok(mut slot) = self.shared.on_turn_end.lock() {
            *slot = Some(hook);
        }
    }

    pub fn set_advisor(&self, advisor: Arc<crate::advisor::AdvisorRuntime>) {
        if let Ok(mut slot) = self.advisor.lock() {
            *slot = Some(advisor);
        }
    }

    pub fn permission_broker(&self) -> Option<Arc<crate::permission::PermissionBroker>> {
        self.permission.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn advisor(&self) -> Option<Arc<crate::advisor::AdvisorRuntime>> {
        self.advisor.lock().ok().and_then(|slot| slot.clone())
    }

    /// Dropping the session stops the timer.
    pub fn set_schedule(
        &self,
        store: Arc<crate::schedule::JobStore>,
        heartbeats: Arc<crate::schedule::HeartbeatService>,
        scheduler: crate::schedule::Scheduler,
    ) {
        if let Ok(mut slot) = self.schedule.lock() {
            *slot = Some((store, heartbeats, scheduler));
        }
    }

    pub fn set_goal_service(&self, service: Arc<crate::goal::GoalService>) {
        if let Ok(mut slot) = self.goal.lock() {
            *slot = Some(service);
        }
    }

    pub fn set_rules_engine(&self, engine: Arc<crate::rules::RuleEngine>) {
        if let Ok(mut slot) = self.rules.lock() {
            *slot = Some(engine);
        }
    }

    pub fn rules_engine(&self) -> Option<Arc<crate::rules::RuleEngine>> {
        self.rules
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
    }

    pub fn set_plan_service(&self, service: Arc<crate::plan::PlanService>) {
        if let Ok(mut slot) = self.plan.lock() {
            *slot = Some(service);
        }
    }

    pub fn plan_service(&self) -> Option<Arc<crate::plan::PlanService>> {
        self.plan
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
    }

    pub fn goal_service(&self) -> Option<Arc<crate::goal::GoalService>> {
        self.goal
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
    }

    pub fn heartbeat_service(&self) -> Option<Arc<crate::schedule::HeartbeatService>> {
        self.schedule
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|(_, service, _)| Arc::clone(service)))
    }

    /// The hook must be non-blocking — spawn any kernel work.
    pub fn set_on_compacted(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.on_compacted.lock() {
            *slot = Some(hook);
        }
    }

    /// Checked at every message boundary inside the tool loop, so a tool-heavy
    /// turn compacts before it blows the window.
    pub fn enable_compaction(&mut self) {
        self.enable_compaction_with(yi_context::Settings::default());
    }

    pub fn enable_compaction_with(&mut self, settings: yi_context::Settings) {
        self.enable_compaction_with_summarizer(settings, None);
    }

    pub fn enable_compaction_with_summarizer(
        &mut self,
        settings: yi_context::Settings,
        summarizer: Option<Model>,
    ) {
        let window_id = format!("win-{}", yi_session::now_ms());
        let mut compactor = crate::compaction::Compactor::new(window_id);
        compactor.settings = settings;
        compactor.summarizer = summarizer;
        self.compactor = Some(Arc::new(compactor));
    }

    pub fn compactor(&self) -> Option<Arc<crate::compaction::Compactor>> {
        self.compactor.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.shared.events.subscribe()
    }

    pub fn status(&self) -> Status {
        self.shared
            .status
            .lock()
            .map(|status| *status)
            .unwrap_or(Status::Idle)
    }

    pub fn last_usage(&self) -> Option<Usage> {
        self.shared
            .last_usage
            .lock()
            .ok()
            .and_then(|usage| usage.clone())
    }

    pub fn set_tools(&mut self, tools: Vec<Arc<dyn yi_loop::AgentTool>>) {
        self.tools = tools;
    }

    /// Aborting the session cancels any tool subprocess still running.
    /// `permission` gates every call; None runs ungated.
    pub fn use_tools(
        &mut self,
        tools: Vec<Arc<dyn yi_tools::Tool>>,
        cwd: std::path::PathBuf,
        permission: Option<Arc<crate::permission::PermissionBroker>>,
    ) {
        self.use_tools_with_background(tools, cwd, permission, None);
    }

    pub fn use_tools_with_background(
        &mut self,
        tools: Vec<Arc<dyn yi_tools::Tool>>,
        cwd: std::path::PathBuf,
        permission: Option<Arc<crate::permission::PermissionBroker>>,
        auto_background: Option<std::time::Duration>,
    ) {
        if let Ok(mut slot) = self.permission.lock() {
            slot.clone_from(&permission);
        }
        let adapters = tools
            .into_iter()
            .map(|tool| {
                let shared = Arc::clone(&self.shared);
                let cancelled: yi_tools::CancelFlag = Arc::new(move || shared.signal.is_fired());
                Arc::new(
                    crate::tools::ToolAdapter::new(
                        tool,
                        cwd.clone(),
                        cancelled,
                        permission.clone(),
                    )
                    .with_auto_background(auto_background)
                    .with_rules(self.rules_engine()),
                ) as Arc<dyn yi_loop::AgentTool>
            })
            .collect();
        self.tools = adapters;
    }

    pub fn events_sender(&self) -> tokio::sync::broadcast::Sender<AgentEvent> {
        self.shared.events.clone()
    }

    /// Loads the main branch as the in-memory history, then persists every
    /// subsequent MessageEnd to it.
    pub fn attach_store(
        &self,
        store: yi_session::SharedSession,
    ) -> Result<usize, yi_session::SessionError> {
        let loaded: Vec<AgentMessage> = {
            let session = yi_session::lock_session(&store);
            let entries = session.find_entries_on_branch(
                "main",
                &yi_session::EntryQuery {
                    order: yi_session::EntryOrder::OldestFirst,
                    ..yi_session::EntryQuery::default()
                },
                &yi_session::BranchBounds::default(),
            )?;
            yi_context::project(&entries)
        };
        let count = loaded.len();
        if let Ok(mut messages) = self.shared.messages.lock() {
            *messages = loaded;
        }
        if let Ok(mut slot) = self.shared.store.lock() {
            *slot = Some(store);
        }
        Ok(count)
    }

    pub fn store_handle(
        &self,
    ) -> std::sync::Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        std::sync::Arc::new(move || {
            shared
                .store
                .lock()
                .map(|handle| handle.clone())
                .unwrap_or_default()
        })
    }

    pub fn store(&self) -> Option<yi_session::SharedSession> {
        self.shared
            .store
            .lock()
            .map(|handle| handle.clone())
            .unwrap_or_default()
    }

    pub fn store_error(&self) -> Option<String> {
        self.shared
            .store_error
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default()
    }

    pub fn model(&self) -> Model {
        self.model
            .lock()
            .map(|model| model.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    pub fn set_model(&self, model: Model) {
        if let Ok(mut slot) = self.model.lock() {
            *slot = model;
        }
    }

    pub fn set_thinking_level(&self, level: Option<String>) {
        self.provider.set_thinking_level(level);
    }

    pub fn pending_count(&self) -> usize {
        let steer = self
            .shared
            .steer
            .lock()
            .map(|queue| queue.len())
            .unwrap_or(0);
        let follow = self
            .shared
            .follow_up
            .lock()
            .map(|queue| queue.len())
            .unwrap_or(0);
        steer.saturating_add(follow)
    }

    /// Detaches any store; `attach_store` afterwards points at a new file.
    pub fn reset(&self) {
        if let Ok(mut messages) = self.shared.messages.lock() {
            messages.clear();
        }
        if let Ok(mut store) = self.shared.store.lock() {
            *store = None;
        }
        if let Ok(mut slot) = self.shared.store_error.lock() {
            *slot = None;
        }
    }

    pub fn steer(&self, text: &str) {
        if let Ok(mut queue) = self.shared.steer.lock() {
            queue.push(user_message(text));
        }
    }

    pub fn follow_up(&self, text: &str) {
        if let Ok(mut queue) = self.shared.follow_up.lock() {
            queue.push(user_message(text));
        }
    }

    pub fn abort(&self) {
        self.shared.signal.fire();
    }

    pub async fn wait_idle(&self) {
        loop {
            if self.status() == Status::Idle {
                return;
            }
            self.shared.idle.notified().await;
        }
    }

    /// Returns at admission; the run streams in a spawned task (R6).
    pub fn prompt(&self, text: &str) -> Result<(), SessionError> {
        self.prompt_message(user_message(text))
    }

    /// Starts an idle session's turn without re-wrapping the message as plain
    /// user text.
    pub fn prompt_message(&self, prompt: AgentMessage) -> Result<(), SessionError> {
        let parts = RunParts {
            shared: Arc::clone(&self.shared),
            provider: Arc::clone(&self.provider),
            system_prompt: self.config.system_prompt.clone(),
            model: self.model(),
            tool_execution: self.config.tool_execution,
            tools: self.tools.clone(),
            compactor: self.compactor.clone(),
            on_compacted: self.on_compacted.lock().ok().and_then(|slot| slot.clone()),
        };
        Self::spawn_run(parts, prompt)
    }

    /// Lets a heartbeat wake an idle session without a `&self` borrow. Tools and
    /// model are snapshotted here — re-wire after `set_model`/`use_tools`.
    pub fn run_handle(
        &self,
    ) -> Arc<dyn Fn(AgentMessage) -> Result<(), SessionError> + Send + Sync> {
        let parts = RunParts {
            shared: Arc::clone(&self.shared),
            provider: Arc::clone(&self.provider),
            system_prompt: self.config.system_prompt.clone(),
            model: self.model(),
            tool_execution: self.config.tool_execution,
            tools: self.tools.clone(),
            compactor: self.compactor.clone(),
            on_compacted: self.on_compacted.lock().ok().and_then(|slot| slot.clone()),
        };
        Arc::new(move |prompt| Self::spawn_run(parts.clone(), prompt))
    }

    /// Running session ⇒ queued (Steer drains at the next message boundary,
    /// FollowUp at turn end); idle session ⇒ the message starts a run.
    pub fn heartbeat_hook(
        &self,
    ) -> Arc<dyn Fn(AgentMessage, yi_types::schedule::DeliveryMode) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        let run = self.run_handle();
        Arc::new(move |message, mode| {
            let running = shared
                .status
                .lock()
                .map(|status| *status == Status::Running)
                .unwrap_or(false);
            if running {
                let queue = match mode {
                    yi_types::schedule::DeliveryMode::Steer => &shared.steer,
                    yi_types::schedule::DeliveryMode::FollowUp => &shared.follow_up,
                };
                if let Ok(mut pending) = queue.lock() {
                    pending.push(message);
                }
            } else {
                let _ = run(message);
            }
        })
    }

    /// Waits out any running turn rather than gating on status: AgentEnd is
    /// emitted while the status is still Running, so a status-gated hook queued
    /// into a follow-up that never drained.
    pub fn wake_idle_hook(&self) -> Arc<dyn Fn(AgentMessage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        let run = self.run_handle();
        Arc::new(move |message| {
            let shared = Arc::clone(&shared);
            let run = Arc::clone(&run);
            tokio::spawn(async move {
                loop {
                    let idle = shared
                        .status
                        .lock()
                        .map(|status| *status == Status::Idle)
                        .unwrap_or(true);
                    if idle {
                        break;
                    }
                    shared.idle.notified().await;
                }
                let _busy_means_queued = run(message);
            });
        })
    }

    /// Running ⇒ steer queue (next tool boundary); idle ⇒ follow-up queue. The
    /// advisor never wakes an idle primary.
    pub fn advisory_hook(&self) -> Arc<dyn Fn(AgentMessage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |message| {
            let running = shared
                .status
                .lock()
                .map(|status| *status == Status::Running)
                .unwrap_or(false);
            let queue = if running {
                &shared.steer
            } else {
                &shared.follow_up
            };
            if let Ok(mut pending) = queue.lock() {
                pending.push(message);
            }
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

    fn spawn_run(parts: RunParts, prompt: AgentMessage) -> Result<(), SessionError> {
        {
            let Ok(mut status) = parts.shared.status.lock() else {
                return Err(SessionError::Busy);
            };
            if *status == Status::Running {
                return Err(SessionError::Busy);
            }
            *status = Status::Running;
        }
        // Nothing ever cleared the session-wide signal, so the first abort made
        // every later turn abort at its first checkpoint. Reading the epoch at
        // admission still stops a turn interrupted in the gap before the spawn.
        let admitted_epoch = parts.shared.signal.epoch();
        let RunParts {
            shared,
            provider,
            system_prompt,
            model,
            tool_execution,
            tools,
            compactor,
            on_compacted,
        } = parts;
        tokio::spawn(async move {
            let hook = shared
                .on_turn_start
                .lock()
                .ok()
                .and_then(|slot| slot.clone());
            if let Some(hook) = hook {
                // Snapshotting shells out to git; the turn waits for it but the
                // runtime thread does not.
                let _hook_failure_never_fails_a_turn =
                    tokio::task::spawn_blocking(move || hook()).await;
            }
            let mut context = LoopContext {
                system_prompt: system_prompt.clone(),
                messages: shared
                    .messages
                    .lock()
                    .map(|messages| messages.clone())
                    .unwrap_or_default(),
                tools,
            };
            let steer = Arc::clone(&shared);
            let follow = Arc::clone(&shared);
            let mut config = LoopConfig::new(model.clone());
            config.tool_execution = tool_execution;
            config.convert_to_llm = Box::new(yi_context::convert_to_llm);
            if let Some(compactor) = compactor.clone() {
                let compact_shared = Arc::clone(&shared);
                let compact_provider = Arc::clone(&provider);
                let compact_model = model.clone();
                let compact_system = system_prompt.clone();
                let compact_hook = on_compacted.clone();
                config.maybe_compact = Some(Box::new(move |messages: &[AgentMessage]| {
                    let compactor = Arc::clone(&compactor);
                    let shared = Arc::clone(&compact_shared);
                    let provider = Arc::clone(&compact_provider);
                    let model = compact_model.clone();
                    let system_prompt = compact_system.clone();
                    let hook = compact_hook.clone();
                    let messages = messages.to_vec();
                    Box::pin(async move {
                        let store = shared
                            .store
                            .lock()
                            .map(|handle| handle.clone())
                            .unwrap_or_default();
                        let signal = yi_loop::interrupt::InterruptSignal::default();
                        let replaced = compactor
                            .maybe_compact(
                                &messages,
                                &model,
                                &system_prompt,
                                provider.as_ref(),
                                store.as_ref(),
                                &signal,
                            )
                            .await;
                        if replaced.is_some()
                            && let Some(hook) = &hook
                        {
                            hook();
                        }
                        replaced
                    })
                }));
            }
            config.get_steering_messages = Some(Box::new(move || {
                steer
                    .steer
                    .lock()
                    .map(|mut queue| std::mem::take(&mut *queue))
                    .unwrap_or_default()
            }));
            config.get_follow_up_messages = Some(Box::new(move || {
                follow
                    .follow_up
                    .lock()
                    .map(|mut queue| std::mem::take(&mut *queue))
                    .unwrap_or_default()
            }));
            let emit_shared = Arc::clone(&shared);
            let emit_compactor = compactor.clone();
            let mut emit = move |event: AgentEvent| {
                if let AgentEvent::MessageEnd { message } = &event {
                    if let AgentMessage::Assistant { usage, .. } = message {
                        if let Ok(mut last) = emit_shared.last_usage.lock() {
                            *last = Some(usage.clone());
                        }
                        if let Some(compactor) = &emit_compactor {
                            compactor.on_usage(usage);
                        }
                    }
                    persist_message(&emit_shared, message);
                }
                let _ = emit_shared.events.send(event);
            };
            shared.signal.reset_if_epoch(admitted_epoch);
            run_loop(
                &mut context,
                vec![prompt],
                &config,
                &shared.signal,
                &mut emit,
                provider.as_ref(),
            )
            .await;
            if let Ok(mut messages) = shared.messages.lock() {
                *messages = context.messages;
            }
            if let Ok(mut status) = shared.status.lock() {
                *status = Status::Idle;
            }
            shared.idle.notify_waiters();
            // The end capture runs after the session is idle again: holding
            // Running across it rejects the follow-up the user types the
            // moment the answer lands.
            let end_hook = shared.on_turn_end.lock().ok().and_then(|slot| slot.clone());
            if let Some(hook) = end_hook {
                let _hook_failure_never_fails_a_turn =
                    tokio::task::spawn_blocking(move || hook()).await;
            }
        });
        Ok(())
    }

    /// Running sessions compact at the next message boundary inside the tool
    /// loop, idle sessions immediately. True when one was applied now.
    pub async fn compact_now(&self) -> bool {
        let Some(compactor) = &self.compactor else {
            return false;
        };
        compactor.schedule();
        if self.status() == Status::Running {
            return false;
        }
        let messages = self.messages();
        let model = self.model();
        let store = self.store();
        let signal = yi_loop::interrupt::InterruptSignal::default();
        let replaced = compactor
            .maybe_compact(
                &messages,
                &model,
                &self.config.system_prompt,
                self.provider.as_ref(),
                store.as_ref(),
                &signal,
            )
            .await;
        match replaced {
            Some(new_messages) => {
                if let Ok(mut slot) = self.shared.messages.lock() {
                    *slot = new_messages;
                }
                if let Ok(slot) = self.on_compacted.lock()
                    && let Some(hook) = slot.as_ref()
                {
                    hook();
                }
                true
            }
            None => false,
        }
    }

    /// A user-role steering message consumed at the next message boundary, or
    /// the next turn when idle — R3 delivery for work finishing outside a turn.
    pub fn follow_up_hook(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |text: &str| {
            if let Ok(mut queue) = shared.follow_up.lock() {
                queue.push(user_message(text));
            }
        })
    }

    pub fn notice_hook(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |text: &str| {
            if let Ok(mut queue) = shared.steer.lock() {
                queue.push(user_message(text));
            }
        })
    }

    /// The model snapshot is taken now: a later `set_model` leaves percent
    /// computed against the old window until re-wired.
    pub fn compact_status_handle(
        &self,
    ) -> Option<Arc<dyn Fn() -> crate::compaction::CompactStatus + Send + Sync>> {
        let compactor = self.compactor.clone()?;
        let shared = Arc::clone(&self.shared);
        let model = self.model();
        Some(Arc::new(move || {
            let messages = shared
                .messages
                .lock()
                .map(|messages| messages.clone())
                .unwrap_or_default();
            compactor.status(&messages, &model)
        }))
    }

    /// Usable after the session moves — the hook holds only shared state.
    pub fn attribution_handle(&self) -> Arc<dyn Fn(&Usage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |child: &Usage| attribute_to_shared(&shared, child))
    }

    /// In-memory usage aggregates; an attached store gains a
    /// `child_usage_attributed` record.
    pub fn attribute_child_usage(&self, child: &Usage) {
        attribute_to_shared(&self.shared, child);
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.shared
            .messages
            .lock()
            .map(|messages| messages.clone())
            .unwrap_or_default()
    }

    pub fn provider(&self) -> &ProviderStream {
        &self.provider
    }

    pub fn provider_arc(&self) -> &Arc<ProviderStream> {
        &self.provider
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: UserContent::Text(text.to_owned()),
        timestamp: 0,
    }
}

fn attribute_to_shared(shared: &Arc<Shared>, child: &Usage) {
    if let Ok(mut messages) = shared.messages.lock()
        && let Some(AgentMessage::Assistant { usage, .. }) = messages
            .iter_mut()
            .rev()
            .find(|message| matches!(message, AgentMessage::Assistant { .. }))
    {
        yi_context::attribution::attribute_child_usage(usage, child);
    }
    let store = shared.store.lock().ok().and_then(|slot| slot.clone());
    if let Some(store) = store {
        let mut session = yi_session::lock_session(&store);
        let id = session.next_id();
        let _ = session.append_record(yi_types::record::LaneRecord::Usage {
            id,
            lane: "main".to_owned(),
            usage: child.clone(),
            cause: yi_context::attribution::CHILD_USAGE_CAUSE.to_owned(),
            run_id: None,
            entry_id: None,
            attempt: None,
            stop_reason: None,
            tool_call_id: None,
            details: None,
            seq: 0,
            timestamp: 0,
        });
    }
}
