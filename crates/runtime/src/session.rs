use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::broadcast;
use yi_loop::ExecutionMode;
use yi_loop::interrupt::InterruptSignal;
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Usage, UserContent};
use yi_types::model::{Effort, Model};

use crate::provider::ProviderStream;

mod deadline;
mod hooks;
mod run;

use deadline::Deadline;
use run::Queued;
pub use run::StillNews;

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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequestShape {
    pub schema: Option<serde_json::Value>,
    pub shared_through: Option<usize>,
}

struct Shared {
    ext: Mutex<Option<Arc<Mutex<crate::ext::Host>>>>,
    model: Mutex<Model>,
    effort: Mutex<Effort>,
    messages: Mutex<Vec<AgentMessage>>,
    steer: Mutex<VecDeque<Queued>>,
    mail: Arc<tokio::sync::Notify>,
    follow_up: Mutex<Vec<AgentMessage>>,
    tools: Mutex<Vec<Arc<dyn yi_loop::AgentTool>>>,
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
    waits: Mutex<Option<Arc<dyn Fn() -> u64 + Send + Sync>>>,
    environment: Mutex<Option<Arc<EnvironmentFn>>>,
    reuse: Mutex<yi_types::model::Reuse>,
    /// The system bytes the first request sent; every later request must send the same (D310).
    first_system_prompt: OnceLock<String>,
    lane: Mutex<Option<Arc<crate::lane::land::LaneHandle>>>,
    telemetry: Mutex<Option<Arc<crate::telemetry::Telemetry>>>,
    todos: Mutex<Option<Arc<crate::todo::TodoStore>>>,
    rules: Mutex<Option<Arc<crate::rules::RuleEngine>>>,
    deadline: OnceLock<Deadline>,
    turn_cap: OnceLock<u32>,
    shape: OnceLock<RequestShape>,
    turn_time: Mutex<(Option<std::time::Instant>, Option<Duration>)>,
    cancelled: std::sync::atomic::AtomicBool,
    /// The kill switch's hold: no wake starts a turn until it lifts; a typed prompt still does.
    held: std::sync::atomic::AtomicBool,
    runs: std::sync::atomic::AtomicU64,
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
    {
        record_store_error(shared, &error);
    }
}

fn record_store_error(shared: &Shared, error: &yi_session::SessionError) {
    if let Ok(mut slot) = shared.store_error.lock() {
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
    tool_execution: ExecutionMode,
    compactor: Option<Arc<crate::compaction::Compactor>>,
    on_compacted: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub struct AgentSession {
    config: SessionConfig,
    provider: Arc<ProviderStream>,
    shared: Arc<Shared>,
    compactor: Option<Arc<crate::compaction::Compactor>>,
    on_compacted: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    schedule: Mutex<Option<Arc<crate::schedule::HeartbeatService>>>,
    advisor: Mutex<Option<Arc<crate::advisor::AdvisorRuntime>>>,
    permission: Mutex<Option<Arc<crate::permission::PermissionBroker>>>,
    goal: Mutex<Option<Arc<crate::goal::GoalService>>>,
    plan: Mutex<Option<Arc<crate::plan::PlanService>>>,
    memory: Mutex<Option<Arc<crate::memory::Activity>>>,
    wall: Mutex<crate::wall::Wall>,
    kernel: Arc<Mutex<Option<Arc<crate::kernel::KernelService>>>>,
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
                steer: Mutex::new(VecDeque::new()),
                mail: Arc::default(),
                follow_up: Mutex::new(Vec::new()),
                tools: Mutex::new(Vec::new()),
                status: Mutex::new(Status::Idle),
                last_usage: Mutex::new(None),
                store: Mutex::new(None),
                store_error: Mutex::new(None),
                events,
                idle: tokio::sync::Notify::new(),
                signal: InterruptSignal::default(),
                on_turn_start: Mutex::new(None),
                environment: Mutex::new(None),
                reuse: Mutex::new(yi_types::model::Reuse::Loop),
                first_system_prompt: OnceLock::new(),
                lane: Mutex::new(None),
                telemetry: Mutex::new(None),
                todos: Mutex::new(None),
                rules: Mutex::new(None),
                on_turn_end: Mutex::new(None),
                coupling: Mutex::new(None),
                waits: Mutex::new(None),
                deadline: OnceLock::new(),
                turn_cap: OnceLock::new(),
                shape: OnceLock::new(),
                turn_time: Mutex::new((None, None)),
                cancelled: false.into(),
                held: false.into(),
                runs: 0.into(),
            }),
            config,
            provider,
            compactor: None,
            on_compacted: Mutex::new(None),
            schedule: Mutex::new(None),
            advisor: Mutex::new(None),
            permission: Mutex::new(None),
            goal: Mutex::new(None),
            plan: Mutex::new(None),
            memory: Mutex::new(None),
            wall: Mutex::new(crate::wall::Wall::default()),
            kernel: Arc::new(Mutex::new(None)),
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
        let deliver = self.deliver_hook();
        if let Some(host) = self.extensions()
            && let Ok(mut host) = host.lock()
        {
            host.set_deliver(Arc::new(move |message| {
                deliver(message, false);
            }));
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

    /// Starts the clock; the first call wins, so the budget never moves mid-run.
    pub(crate) fn set_deadline(&self, total: Duration) {
        self.shared.deadline.get_or_init(|| Deadline::new(total));
    }

    pub fn set_turn_cap(&self, turns: u32) {
        self.shared.turn_cap.get_or_init(|| turns);
    }

    pub fn set_request_shape(&self, shape: RequestShape) {
        self.shared.shape.get_or_init(|| shape);
    }

    pub fn request_shape(&self) -> Option<RequestShape> {
        self.shared.shape.get().cloned()
    }

    pub(crate) fn deadline(&self) -> Option<Deadline> {
        self.shared.deadline.get().copied()
    }

    pub fn set_environment(&self, hook: Arc<EnvironmentFn>) {
        if let Ok(mut slot) = self.shared.environment.lock() {
            *slot = Some(hook);
        }
    }

    /// A session whose one prompt is never continued (the auto-reviewer) says so, and its
    /// tail is never marked (D295).
    pub fn reuse(&self) -> yi_types::model::Reuse {
        self.shared
            .reuse
            .lock()
            .map(|reuse| *reuse)
            .unwrap_or_default()
    }

    pub fn set_reuse(&self, reuse: yi_types::model::Reuse) {
        if let Ok(mut slot) = self.shared.reuse.lock() {
            *slot = reuse;
        }
    }

    pub fn set_todos(&self, todos: Arc<crate::todo::TodoStore>) {
        if let Ok(mut slot) = self.shared.todos.lock() {
            *slot = Some(todos);
        }
    }

    pub fn todos_handle(
        &self,
    ) -> Arc<dyn Fn() -> Option<Arc<crate::todo::TodoStore>> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            shared
                .todos
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(Arc::clone))
        })
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
        if let Ok(mut slot) = self.shared.rules.lock() {
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
        self.shared
            .rules
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

    pub fn set_memory(&self, activity: Arc<crate::memory::Activity>) {
        if let Ok(mut slot) = self.memory.lock() {
            *slot = Some(activity);
        }
    }

    pub fn memory(&self) -> Option<Arc<crate::memory::Activity>> {
        self.memory
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

    pub fn set_waits(&self, count: Arc<dyn Fn() -> u64 + Send + Sync>) {
        if let Ok(mut slot) = self.shared.waits.lock() {
            *slot = Some(count);
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

    /// Incident: a wake handle minted before the tools landed woke a turn that could call nothing.
    pub fn set_tools(&mut self, tools: Vec<Arc<dyn yi_loop::AgentTool>>) {
        if let Ok(mut slot) = self.shared.tools.lock() {
            *slot = tools;
        }
    }

    /// The registered table, as attached; the surface lock renders its lock
    /// from these definitions so what it pins is what a model call can name.
    pub fn tools(&self) -> Vec<Arc<dyn yi_loop::AgentTool>> {
        let slot = self.shared.tools.lock();
        slot.as_deref().cloned().unwrap_or_default()
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
        if let Some(broker) = &permission {
            broker.set_rule_journal(crate::wiring::journal_into(self.store_handle()));
        }
        // A session with no store yet still spills under a dir of its own (D340).
        let (store_id, unsaved) = (
            self.store_id_hook(),
            yi_session::IdGenerator::new().next_id(),
        );
        let spill_key: Arc<dyn Fn() -> Option<String> + Send + Sync> =
            Arc::new(move || store_id().or_else(|| Some(unsaved.clone())));
        let adapters = tools
            .into_iter()
            .map(|tool| {
                let shared = Arc::clone(&self.shared);
                // The deadline cancels like Esc, a last word early: bash dies, the cell stops.
                let cancelled: yi_tools::CancelFlag =
                    Arc::new(move || shared.signal.is_fired() || shared.last_word_due());
                Arc::new(
                    crate::tools::ToolAdapter::new(
                        tool,
                        cwd.clone(),
                        cancelled,
                        permission.clone(),
                    )
                    .with_auto_background(auto_background)
                    .with_rules(self.rules_engine())
                    .with_check(crate::plan::covers::write_check(self.plan_service()))
                    .with_wall(self.wall())
                    .with_spill_key(Arc::clone(&spill_key))
                    .with_transcript(self.store_handle())
                    .with_extensions(Some(self.ext_hook())),
                ) as Arc<dyn yi_loop::AgentTool>
            })
            .collect();
        self.set_tools(adapters);
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
        if let Ok(mut queue) = self.shared.steer.lock() {
            for message in crate::mail::unread(&entries) {
                run::push(&mut queue, Queued::new(message, false, None));
            }
        }
        if let Some(broker) = self.permission_broker() {
            let kept = entries.iter().filter_map(|entry| match entry {
                Entry::Custom {
                    custom_type,
                    data: Some(data),
                    ..
                } if custom_type == yi_types::permission::PERMISSION_RULE_ENTRY => {
                    serde_json::from_value(data.clone()).ok()
                }
                _ => None,
            });
            broker.replay(kept.collect(), &self.wall());
        }
        let id = yi_session::lock_session(&store).metadata().id.clone();
        if let Ok(mut slot) = self.shared.store.lock() {
            *slot = Some(store);
        }
        if let Some(todos) = self.todos() {
            todos.rehydrate();
        }
        // Invariant: bound last, so a tick owed since the last process finds the ledger and list.
        if let Some(service) = self.heartbeat_service() {
            service.bind_session(id);
            if let Some(todos) = self.todos() {
                service.watch(&todos.list());
            }
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

    pub fn summarizer(&self) -> Model {
        self.compactor
            .as_ref()
            .and_then(|compactor| compactor.summarizer.clone())
            .unwrap_or_else(|| self.model())
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
        if let Err(error) = session.append_entry(entry, "main") {
            record_store_error(&self.shared, &error);
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
        if let Some(broker) = self.permission_broker() {
            broker.forget_rules();
        }
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
        self.deliver(message, false);
    }

    pub fn follow_up(&self, text: &str) {
        self.follow_up_message(user_message(text));
    }

    /// Runs admitted so far, and how many of them have ended.
    pub(crate) fn runs(&self) -> (u64, u64) {
        let running = self
            .shared
            .status
            .lock()
            .is_ok_and(|status| *status == Status::Running);
        let started = self.shared.runs.load(std::sync::atomic::Ordering::SeqCst);
        (started, started.saturating_sub(u64::from(running)))
    }

    /// Taken after a running turn's answer, or it starts an idle session's turn.
    pub fn follow_up_message(&self, message: AgentMessage) -> bool {
        run::follow(&self.parts(), message)
    }

    /// Presented in arrival order at the next boundary; `wakes` starts an idle session's turn.
    pub fn deliver(&self, message: AgentMessage, wakes: bool) -> yi_types::mail::Delivery {
        run::enqueue(&self.parts(), Queued::new(message, wakes, None))
    }

    pub(crate) fn adopt_pending(&self, dead: &AgentSession) {
        let taken = dead
            .shared
            .steer
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue));
        if let (Ok(taken), Ok(mut queue)) = (taken, self.shared.steer.lock()) {
            taken
                .into_iter()
                .for_each(|entry| run::push(&mut queue, entry));
            self.shared.mail.notify_waiters();
        }
        let follow_ups = dead
            .shared
            .follow_up
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue));
        if let (Ok(taken), Ok(mut queue)) = (follow_ups, self.shared.follow_up.lock()) {
            queue.extend(taken);
        }
    }

    /// Returns at admission; the run streams in a spawned task (§4.3).
    pub fn prompt(&self, text: &str) -> Result<(), SessionError> {
        self.prompt_message(user_message(text))
    }

    /// Starts an idle session's turn without re-wrapping the message as plain user text.
    pub fn prompt_message(&self, prompt: AgentMessage) -> Result<(), SessionError> {
        self.prompt_requested(prompt, None)
    }

    /// `requested` is [`Self::abort_epoch`] read when the run was asked for, so an abort fired
    /// between the request and admission still stops it.
    pub fn prompt_requested(
        &self,
        prompt: AgentMessage,
        requested: Option<u64>,
    ) -> Result<(), SessionError> {
        if self.status() == Status::Idle {
            start_ext(&self.shared, &prompt_text(&prompt));
        }
        run::spawn_run(self.parts(), prompt, requested)
    }

    fn parts(&self) -> RunParts {
        RunParts {
            shared: Arc::clone(&self.shared),
            provider: Arc::clone(&self.provider),
            system_prompt: self.prompt_source(),
            tool_execution: self.config.tool_execution,
            compactor: self.compactor.clone(),
            on_compacted: self.on_compacted.lock().ok().and_then(|slot| slot.clone()),
        }
    }

    /// Invariant: pre-first-turn only (§11) — mid-run it races the appending turn.
    pub fn seed_messages(&self, seed: Vec<AgentMessage>) {
        if let Ok(mut messages) = self.shared.messages.lock() {
            *messages = seed;
        }
    }

    /// Running sessions compact at the next boundary of the tool loop, idle ones now and say
    /// what they did; a failure returns at once and also queues a notice for the next prompt.
    pub async fn compact_now(
        &self,
    ) -> Result<crate::compaction::CompactOutcome, crate::compaction::CompactError> {
        use crate::compaction::CompactOutcome;
        let Some(compactor) = &self.compactor else {
            return Ok(CompactOutcome::NotApplied);
        };
        compactor.schedule();
        if self.status() == Status::Running {
            return Ok(CompactOutcome::NotApplied);
        }
        let messages = self.messages();
        let (model, effort) = hooks::settings_of(&self.shared);
        let request =
            crate::compaction::LoopRequest::new(self.system_prompt(), &self.tools(), effort);
        let store = self.store();
        let signal = yi_loop::interrupt::InterruptSignal::default();
        let replaced = compactor
            .maybe_compact(
                &messages,
                &model,
                &request,
                self.provider.as_ref(),
                store.as_ref(),
                &signal,
            )
            .await;
        match replaced {
            Ok(Some(replaced)) => {
                if let Ok(mut slot) = self.shared.messages.lock() {
                    *slot = replaced.messages;
                }
                if let Some(elision) = replaced.elision {
                    run::queue_compaction_notice(&self.parts(), &elision.to_string());
                }
                dispatch_ext(&self.shared, &crate::ext::Event::Compacted);
                if let Ok(slot) = self.on_compacted.lock()
                    && let Some(hook) = slot.as_ref()
                {
                    hook();
                }
                Ok(replaced
                    .elision
                    .map_or(CompactOutcome::Summarized, CompactOutcome::Elided))
            }
            Ok(None) => Ok(CompactOutcome::NotApplied),
            Err(error) => {
                run::failed_compaction(&self.parts(), &error);
                Err(error)
            }
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

fn prompt_text(prompt: &AgentMessage) -> String {
    match prompt {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        } => text.clone(),
        AgentMessage::User {
            content: UserContent::Blocks(blocks),
            ..
        } => yi_types::message::join_text(blocks, "\n"),
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
        let recorded = session.append_record(yi_types::record::LaneRecord::Usage {
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
        if let Err(error) = recorded {
            record_store_error(shared, &error);
        }
    }
}

/// Invariant: `notify_waiters` stores no permit, so this registers before `poll` reads state.
pub(crate) async fn until<T>(
    notify: &tokio::sync::Notify,
    mut poll: impl FnMut() -> std::ops::ControlFlow<T, Option<std::time::Instant>>,
) -> T {
    loop {
        let mut woken = std::pin::pin!(notify.notified());
        woken.as_mut().enable();
        match poll() {
            std::ops::ControlFlow::Break(done) => return done,
            std::ops::ControlFlow::Continue(Some(at)) => {
                let at = tokio::time::Instant::from_std(at);
                let _ = tokio::time::timeout_at(at, woken).await;
            }
            std::ops::ControlFlow::Continue(None) => woken.await,
        }
    }
}

pub async fn next_event(
    events: &mut broadcast::Receiver<AgentEvent>,
) -> Option<Result<AgentEvent, yi_types::event::EventGap>> {
    match events.recv().await {
        Ok(event) => Some(Ok(event)),
        Err(broadcast::error::RecvError::Lagged(dropped)) => {
            Some(Err(yi_types::event::EventGap { dropped }))
        }
        Err(broadcast::error::RecvError::Closed) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::ops::ControlFlow;
    use std::time::Duration;

    #[tokio::test]
    async fn a_wake_fired_while_polling_is_not_lost() -> Result<(), tokio::time::error::Elapsed> {
        let notify = tokio::sync::Notify::new();
        let mut polls = 0;
        let woke = super::until(&notify, || {
            polls += 1;
            if polls > 1 {
                return ControlFlow::Break(polls);
            }
            notify.notify_waiters();
            ControlFlow::Continue(None)
        });
        assert_eq!(tokio::time::timeout(Duration::from_secs(1), woke).await?, 2);
        Ok(())
    }
}
