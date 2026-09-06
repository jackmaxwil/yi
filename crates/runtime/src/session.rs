use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use yi_loop::interrupt::InterruptSignal;
use yi_loop::{ExecutionMode, LoopConfig, LoopContext, run_loop};
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Usage, UserContent};
use yi_types::model::{Effort, Model};

use crate::provider::ProviderStream;

mod hooks;

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
    pub thinking_level: Option<Effort>,
    pub tool_execution: ExecutionMode,
}

struct Shared {
    ext: Mutex<Option<Arc<Mutex<crate::ext::Host>>>>,
    model: Mutex<Model>,
    effort: Mutex<Effort>,
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
    coupling: Mutex<Option<TurnCoupling>>,
    environment: Mutex<Option<Arc<EnvironmentFn>>>,
    lane: Mutex<Option<Arc<crate::lane::land::LaneHandle>>>,
    telemetry: Mutex<Option<Arc<crate::telemetry::Telemetry>>>,
    todos: Mutex<Option<Arc<crate::todo::TodoStore>>>,
}

pub type PromptChoiceFn =
    dyn Fn(&AgentMessage) -> Option<yi_types::model::ToolChoice> + Send + Sync;
pub type TurnObserveFn = dyn Fn(&yi_loop::TurnSnapshot) + Send + Sync;
pub type InterceptStopFn = dyn Fn(&yi_loop::TurnSnapshot) -> Option<AgentMessage> + Send + Sync;

/// Loop hooks the runtime fills; read live at run start, so late installation
/// still binds handles minted earlier.
#[derive(Clone)]
pub struct TurnCoupling {
    pub on_prompt: Arc<PromptChoiceFn>,
    pub on_turn: Arc<TurnObserveFn>,
    pub intercept_stop: Arc<InterceptStopFn>,
}

/// The start hook captures the tree the turn is about to change, the end hook
/// what it left behind.
pub type TurnHook = dyn Fn() + Send + Sync;

pub type EnvironmentFn = dyn Fn() -> Option<String> + Send + Sync;

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

pub type PromptSource = Arc<dyn Fn() -> String + Send + Sync>;

pub type ExtHook = Arc<dyn Fn(crate::ext::Event) + Send + Sync>;

#[derive(Clone)]
struct RunParts {
    shared: Arc<Shared>,
    provider: Arc<ProviderStream>,
    system_prompt: PromptSource,
    model: Model,
    effort: Effort,
    tool_execution: ExecutionMode,
    tools: Vec<Arc<dyn yi_loop::AgentTool>>,
    compactor: Option<Arc<crate::compaction::Compactor>>,
    on_compacted: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub struct AgentSession {
    config: SessionConfig,
    provider: Arc<ProviderStream>,
    shared: Arc<Shared>,
    tools: Vec<Arc<dyn yi_loop::AgentTool>>,
    compactor: Option<Arc<crate::compaction::Compactor>>,
    on_compacted: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    schedule: Mutex<Option<Arc<crate::schedule::HeartbeatService>>>,
    advisor: Mutex<Option<Arc<crate::advisor::AdvisorRuntime>>>,
    permission: Mutex<Option<Arc<crate::permission::PermissionBroker>>>,
    goal: Mutex<Option<Arc<crate::goal::GoalService>>>,
    plan: Mutex<Option<Arc<crate::plan::PlanService>>>,
    rules: Mutex<Option<Arc<crate::rules::RuleEngine>>>,
    wall: Mutex<crate::wall::Wall>,
    kernel: Mutex<Option<Arc<crate::kernel::KernelService>>>,
}

impl AgentSession {
    pub fn new(config: SessionConfig, provider: Arc<ProviderStream>) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            shared: Arc::new(Shared {
                ext: Mutex::new(None),
                model: Mutex::new(config.model.clone()),
                effort: Mutex::new(
                    config
                        .model
                        .clamp_effort(config.thinking_level.unwrap_or_default()),
                ),
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
                environment: Mutex::new(None),
                lane: Mutex::new(None),

                telemetry: Mutex::new(None),
                todos: Mutex::new(None),
                on_turn_end: Mutex::new(None),
                coupling: Mutex::new(None),
            }),
            config,
            provider,
            tools: Vec::new(),
            compactor: None,
            on_compacted: Mutex::new(None),
            schedule: Mutex::new(None),
            advisor: Mutex::new(None),
            permission: Mutex::new(None),
            goal: Mutex::new(None),
            plan: Mutex::new(None),
            rules: Mutex::new(None),
            wall: Mutex::new(crate::wall::Wall::default()),
            kernel: Mutex::new(None),
        }
    }

    pub fn set_kernel_service(&self, service: Arc<crate::kernel::KernelService>) {
        if let Ok(mut slot) = self.kernel.lock() {
            *slot = Some(service);
        }
    }

    /// Incident: the kernel pump's monitor task owns the tokio Child, so a dropped session
    /// leaks its IPython process. Every path that retires a session calls this.
    pub fn dispose_kernel(&self) {
        let Some(kernel) = self.kernel_service() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move { kernel.dispose().await });
        }
    }

    pub fn kernel_service(&self) -> Option<Arc<crate::kernel::KernelService>> {
        self.kernel
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
    }

    pub fn install_extensions(&self, host: crate::ext::Host) {
        if let Ok(mut slot) = self.shared.ext.lock() {
            *slot = Some(Arc::new(Mutex::new(host)));
        }
        let notice = self.notice_hook();
        if let Some(host) = self.extensions()
            && let Ok(mut host) = host.lock()
        {
            host.set_notice(notice);
        }
    }

    /// The attached store's id, read when asked: the store may attach after wiring.
    pub fn store_id_hook(&self) -> Arc<dyn Fn() -> Option<String> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            store_of(&shared).map(|store| yi_session::lock_session(&store).metadata().id.clone())
        })
    }

    pub fn extensions(&self) -> Option<Arc<Mutex<crate::ext::Host>>> {
        extensions_of(&self.shared)
    }

    pub fn system_prompt(&self) -> String {
        assembled_prompt(&self.shared, &self.config.system_prompt)
    }

    fn prompt_source(&self) -> PromptSource {
        let shared = Arc::clone(&self.shared);
        let fallback = self.config.system_prompt.clone();
        Arc::new(move || assembled_prompt(&shared, &fallback))
    }

    pub fn set_environment(&self, hook: Arc<EnvironmentFn>) {
        if let Ok(mut slot) = self.shared.environment.lock() {
            *slot = Some(hook);
        }
    }

    pub fn set_todos(&self, todos: Arc<crate::todo::TodoStore>) {
        if let Ok(mut slot) = self.shared.todos.lock() {
            *slot = Some(todos);
        }
    }

    pub fn todos(&self) -> Option<Arc<crate::todo::TodoStore>> {
        self.shared
            .todos
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
    }

    pub fn set_lane(&self, lane: Arc<crate::lane::land::LaneHandle>) {
        if let Ok(mut slot) = self.shared.lane.lock() {
            *slot = Some(lane);
        }
    }

    pub fn lane(&self) -> Option<Arc<crate::lane::land::LaneHandle>> {
        self.shared.lane.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn lane_handle(
        &self,
    ) -> Arc<dyn Fn() -> Option<Arc<crate::lane::land::LaneHandle>> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || shared.lane.lock().ok().and_then(|slot| slot.clone()))
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

    pub fn set_schedule(&self, heartbeats: Arc<crate::schedule::HeartbeatService>) {
        if let Ok(mut slot) = self.schedule.lock() {
            *slot = Some(heartbeats);
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

    pub fn set_wall(&self, wall: crate::wall::Wall) {
        if let Ok(mut slot) = self.wall.lock() {
            *slot = wall;
        }
    }

    pub fn wall(&self) -> crate::wall::Wall {
        self.wall
            .lock()
            .map(|wall| wall.clone())
            .unwrap_or_default()
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
            .and_then(|slot| slot.as_ref().map(Arc::clone))
    }

    /// The hook must be non-blocking — spawn any kernel work.
    pub fn set_turn_coupling(&self, coupling: TurnCoupling) {
        if let Ok(mut slot) = self.shared.coupling.lock() {
            *slot = Some(coupling);
        }
    }

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

    pub fn set_telemetry(&self, telemetry: Arc<crate::telemetry::Telemetry>) {
        if let Ok(mut slot) = self.shared.telemetry.lock() {
            *slot = Some(telemetry);
        }
    }

    pub fn telemetry(&self) -> Option<Arc<crate::telemetry::Telemetry>> {
        self.shared
            .telemetry
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
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
                    .with_rules(self.rules_engine())
                    .with_wall(self.wall())
                    .with_extensions(Some(self.ext_hook())),
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
        let sidecar = {
            let session = yi_session::lock_session(&store);
            session
                .file_path()
                .cloned()
                .map(|file| (file, session.metadata().id.clone()))
        };
        let entries = {
            let session = yi_session::lock_session(&store);
            session.find_entries_on_branch(
                "main",
                &yi_session::EntryQuery {
                    order: yi_session::EntryOrder::OldestFirst,
                    ..yi_session::EntryQuery::default()
                },
                &yi_session::BranchBounds::default(),
            )?
        };
        let loaded = yi_context::project(&entries);
        let count = loaded.len();
        if let Ok(mut messages) = self.shared.messages.lock() {
            *messages = loaded;
        }
        if let Some(service) = self.heartbeat_service() {
            let id = yi_session::lock_session(&store).metadata().id.clone();
            service.bind_session(id);
        }
        if let Ok(mut slot) = self.shared.store.lock() {
            *slot = Some(store);
        }
        if let (Some(telemetry), Some((file, id))) = (self.telemetry(), sidecar) {
            telemetry.bind(&file, &id);
        }
        self.restore_settings(&entries);
        Ok(count)
    }

    fn restore_settings(&self, entries: &[Entry]) {
        let mut effort = None;
        for entry in entries {
            match entry {
                Entry::ModelChange {
                    provider, model_id, ..
                } => {
                    if let Some(model) = crate::provider::resolve_model(provider, model_id)
                        && let Ok(mut slot) = self.shared.model.lock()
                    {
                        *slot = model;
                    }
                }
                Entry::ThinkingLevelChange { thinking_level, .. } => {
                    effort = thinking_level.parse().ok();
                }
                _ => {}
            }
        }
        let restored = self.model().clamp_effort(effort.unwrap_or(self.effort()));
        if let Ok(mut slot) = self.shared.effort.lock() {
            *slot = restored;
        }
    }

    pub fn store(&self) -> Option<yi_session::SharedSession> {
        store_of(&self.shared)
    }

    pub fn store_error(&self) -> Option<String> {
        self.shared
            .store_error
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default()
    }

    pub fn model(&self) -> Model {
        self.shared
            .model
            .lock()
            .map(|model| model.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Re-clamps the effort onto the new ladder without recording a level change of its own;
    /// a caller wanting a specific level calls [`AgentSession::set_effort`] after.
    pub fn set_model(&self, model: Model) {
        let current = self.model();
        if current.provider == model.provider && current.id == model.id {
            return;
        }
        let effort = model.clamp_effort(self.effort());
        if let Ok(mut slot) = self.shared.model.lock() {
            *slot = model.clone();
        }
        if let Ok(mut slot) = self.shared.effort.lock() {
            *slot = effort;
        }
        self.append_state_entry(Entry::ModelChange {
            id: String::new(),
            provider: model.provider,
            model_id: model.id,
            parent_id: None,
            seq: 0,
            timestamp: 0,
        });
    }

    pub fn effort(&self) -> Effort {
        self.shared
            .effort
            .lock()
            .map(|effort| *effort)
            .unwrap_or_else(|poisoned| *poisoned.into_inner())
    }

    /// Returns what was actually set, which the clamp may have moved.
    pub fn set_effort(&self, effort: Effort) -> Effort {
        let effective = self.model().clamp_effort(effort);
        let changed = match self.shared.effort.lock() {
            Ok(mut slot) => {
                let changed = *slot != effective;
                *slot = effective;
                changed
            }
            Err(_) => false,
        };
        if changed {
            self.append_state_entry(Entry::ThinkingLevelChange {
                id: String::new(),
                thinking_level: effective.to_string(),
                parent_id: None,
                seq: 0,
                timestamp: 0,
            });
        }
        effective
    }

    fn append_state_entry(&self, mut entry: Entry) {
        let Some(store) = self.store() else {
            return;
        };
        let mut session = yi_session::lock_session(&store);
        let id = session.next_id();
        match &mut entry {
            Entry::ModelChange { id: slot, .. } | Entry::ThinkingLevelChange { id: slot, .. } => {
                *slot = id;
            }
            _ => return,
        }
        if let Err(error) = session.append_entry(entry, "main")
            && let Ok(mut slot) = self.shared.store_error.lock()
        {
            *slot = Some(error.to_string());
        }
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

    /// Detaches any store; [`AgentSession::attach_store`] afterwards points at a new file.
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
        self.steer_message(user_message(text));
    }

    pub fn steer_message(&self, message: AgentMessage) {
        if let Ok(mut queue) = self.shared.steer.lock() {
            queue.push(message);
        }
    }

    pub fn follow_up(&self, text: &str) {
        self.follow_up_message(user_message(text));
    }

    /// B13 `send`: queued for the next turn, never starting one.
    pub fn follow_up_message(&self, message: AgentMessage) {
        if let Ok(mut queue) = self.shared.follow_up.lock() {
            queue.push(message);
        }
    }

    /// B13 `followup`: delivered into a running turn at its next boundary, or
    /// starting one when the session is idle.
    pub fn deliver(&self, message: AgentMessage) {
        (self.heartbeat_hook())(message, yi_types::schedule::DeliveryMode::Steer);
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
        if self.status() == Status::Idle {
            start_ext(&self.shared, &prompt_text(&prompt));
        }
        let parts = RunParts {
            shared: Arc::clone(&self.shared),
            provider: Arc::clone(&self.provider),
            system_prompt: self.prompt_source(),
            model: self.model(),
            effort: self.effort(),
            tool_execution: self.config.tool_execution,
            tools: self.tools.clone(),
            compactor: self.compactor.clone(),
            on_compacted: self.on_compacted.lock().ok().and_then(|slot| slot.clone()),
        };
        Self::spawn_run(parts, prompt)
    }

    /// Lets a heartbeat wake an idle session without a `&self` borrow. Tools and
    /// model are snapshotted here — re-wire after [`AgentSession::set_model`]/[`AgentSession::use_tools`].
    pub fn run_handle(
        &self,
    ) -> Arc<dyn Fn(AgentMessage) -> Result<(), SessionError> + Send + Sync> {
        let parts = RunParts {
            shared: Arc::clone(&self.shared),
            provider: Arc::clone(&self.provider),
            system_prompt: self.prompt_source(),
            model: self.model(),
            effort: self.effort(),
            tool_execution: self.config.tool_execution,
            tools: self.tools.clone(),
            compactor: self.compactor.clone(),
            on_compacted: self.on_compacted.lock().ok().and_then(|slot| slot.clone()),
        };
        Arc::new(move |prompt| Self::spawn_run(parts.clone(), prompt))
    }

    /// Invariant: pre-first-turn only (B5) — mid-run it races the appending turn.
    pub fn seed_messages(&self, seed: Vec<AgentMessage>) {
        if let Ok(mut messages) = self.shared.messages.lock() {
            *messages = seed;
        }
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
        // Incident: nothing cleared the session-wide signal, so the first abort aborted every
        // later turn. Reading the epoch at admission still stops one hit before the spawn.
        let admitted_epoch = parts.shared.signal.epoch();
        let RunParts {
            shared,
            provider,
            system_prompt,
            model,
            effort,
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
                system_prompt: system_prompt(),
                messages: shared
                    .messages
                    .lock()
                    .map(|messages| messages.clone())
                    .unwrap_or_default(),
                tools,
            };
            let mut config = LoopConfig::new(model.clone());
            config.effort = effort;
            config.tool_execution = tool_execution;
            config.convert_to_llm = Box::new(yi_context::convert_to_llm);
            if let Some(compactor) = compactor.clone() {
                let stores = Arc::clone(&shared);
                let notify = Arc::clone(&shared);
                let hook = on_compacted.clone();
                config.maybe_compact = Some(crate::compaction::loop_hook(
                    compactor,
                    Arc::clone(&provider),
                    model.clone(),
                    Arc::clone(&system_prompt),
                    Arc::new(move || store_of(&stores)),
                    Arc::new(move || {
                        dispatch_ext(&notify, &crate::ext::Event::Compacted);
                        if let Some(hook) = &hook {
                            hook();
                        }
                    }),
                ));
            }
            wire_queues_and_coupling(&mut config, &shared, &prompt);
            wire_environment(&mut config, &shared).await;
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
                        dispatch_ext(
                            &emit_shared,
                            &crate::ext::Event::Usage {
                                input: usage.input,
                                cache_read: usage.cache_read,
                                cache_write: usage.cache_write,
                            },
                        );
                    }
                    persist_message(&emit_shared, message);
                }
                if let Some(telemetry) = emit_shared
                    .telemetry
                    .lock()
                    .ok()
                    .and_then(|slot| slot.clone())
                {
                    telemetry.on_event(&event);
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
            if let Some(host) = extensions_of(&shared) {
                let event = host.lock().ok().map(|host| host.turn_end_event());
                if let Some(event) = event {
                    dispatch_ext(&shared, &event);
                }
            }
            if let Ok(mut status) = shared.status.lock() {
                *status = Status::Idle;
            }
            shared.idle.notify_waiters();
            // The end capture runs after the session is idle again: holding Running across it
            // rejects the follow-up the user types the moment the answer lands.
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
                &self.system_prompt(),
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
                dispatch_ext(&self.shared, &crate::ext::Event::Compacted);
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

async fn wire_environment(config: &mut LoopConfig, shared: &Arc<Shared>) {
    let hook = shared.environment.lock().ok().and_then(|slot| slot.clone());
    let Some(hook) = hook else {
        return;
    };
    let Some(block) = tokio::task::spawn_blocking(move || hook())
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    config.transform_context = Some(Box::new(move |messages| {
        Some(crate::environment::append(messages, &block))
    }));
}

fn wire_queues_and_coupling(config: &mut LoopConfig, shared: &Arc<Shared>, prompt: &AgentMessage) {
    let steer = Arc::clone(shared);
    let follow = Arc::clone(shared);
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
    let coupling = shared
        .coupling
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    if let Some(coupling) = coupling {
        config.first_turn_tool_choice = (coupling.on_prompt)(prompt);
        let observe = Arc::clone(&coupling.on_turn);
        config.prepare_next_turn = Some(Box::new(move |snapshot| {
            observe(snapshot);
            None
        }));
        let intercept = Arc::clone(&coupling.intercept_stop);
        config.intercept_stop = Some(Box::new(move |snapshot| intercept(snapshot)));
    }
}

fn prompt_text(prompt: &AgentMessage) -> String {
    match prompt {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        } => text.clone(),
        AgentMessage::User {
            content: UserContent::Blocks(blocks),
            ..
        } => blocks
            .iter()
            .filter_map(|block| match block {
                yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn extensions_of(shared: &Arc<Shared>) -> Option<Arc<Mutex<crate::ext::Host>>> {
    shared
        .ext
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(Arc::clone))
}

fn store_of(shared: &Arc<Shared>) -> Option<yi_session::SharedSession> {
    shared
        .store
        .lock()
        .map(|handle| handle.clone())
        .unwrap_or_default()
}

fn assembled_prompt(shared: &Arc<Shared>, fallback: &str) -> String {
    let Some(host) = extensions_of(shared) else {
        return fallback.to_owned();
    };
    let assembled = host
        .lock()
        .map(|host| host.system_prompt())
        .unwrap_or_default();
    if assembled.is_empty() {
        fallback.to_owned()
    } else {
        assembled
    }
}

fn dispatch_ext(shared: &Arc<Shared>, event: &crate::ext::Event) {
    let Some(host) = extensions_of(shared) else {
        return;
    };
    let store = store_of(shared);
    if let Ok(mut host) = host.lock() {
        host.dispatch(event, store.as_ref());
    }
}

fn start_ext(shared: &Arc<Shared>, prompt: &str) {
    let Some(host) = extensions_of(shared) else {
        return;
    };
    let store = store_of(shared);
    let resumed = shared
        .messages
        .lock()
        .map(|messages| !messages.is_empty())
        .unwrap_or(false);
    let event = {
        let Ok(mut host) = host.lock() else {
            return;
        };
        host.start(store.as_ref(), resumed);
        host.prompt_event(prompt)
    };
    dispatch_ext(shared, &event);
}

pub(crate) fn user_message(text: &str) -> AgentMessage {
    AgentMessage::host_user(UserContent::Text(text.to_owned()), 0)
}

/// Invariant: only input that crossed the process boundary mints
/// [`yi_types::message::Attribution::User`], so no `user://` address serves a host-written one.
pub fn user_input(text: &str) -> AgentMessage {
    AgentMessage::user_input(UserContent::Text(text.to_owned()), 0)
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
