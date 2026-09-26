use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::plan::doc::{
    Check, DocError, Plan, PlanId, PlanState, TodoState, TodoStateName, TouchCount,
};
use yi_types::schedule::DeliveryMode;

use crate::goal::{DeliverFn, StoreHandle};

pub mod acceptance;
pub mod artifact;
pub mod authority;
mod brief;
pub mod capacity;
pub mod covers;
pub mod declare;
pub mod dispatch;
pub mod done;
pub mod finish;
pub mod import;
pub mod journal;
pub mod judge;
pub mod ledger;
pub mod loop_coupling;
pub mod ops;
pub mod output;
pub mod probe;
pub mod program;
pub mod recovery;
pub mod request;
pub mod schedule;
pub mod snapshot;
pub mod state;
pub mod store;
pub mod submit;
pub mod table;
pub mod tool;
pub mod verify;
pub mod why;

pub use loop_coupling::gate;
use store::{PlanStore, StoreError};

pub fn subplans_of(plan: &Plan, plans_dir: &Path) -> Vec<Plan> {
    let ids: Vec<_> = plan
        .todos
        .iter()
        .filter_map(|todo| todo.subplan.clone())
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let Ok(store) = PlanStore::open(plans_dir.to_path_buf()) else {
        return Vec::new();
    };
    ids.iter().filter_map(|id| store.read(id).ok()).collect()
}

pub type PlanChangeHook = Arc<dyn Fn(&Plan) + Send + Sync>;

/// A plan untouched this many completed turns while work flows earns one
/// staleness reminder; the latch clears when `touched` moves, never `version`.
pub const DEFAULT_STALE_TURNS: u64 = 12;

pub const PLANS_DIR: &str = ".yi/plans";

pub fn default_plans_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(PLANS_DIR)
}

#[derive(Debug, thiserror::Error)]
pub enum CanonicalPlanError {
    #[error("no plan is open under {dir}")]
    NoPlanOpen { dir: PathBuf },
    #[error("the session's plan pointer {id:?} is not a plan id: {cause}")]
    Pointer { id: String, cause: DocError },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The one plan the session is working: the fact's doc pointer when the host wrote one, else
/// the Active root in the plans directory. The fact carries no task bodies; the file is truth.
pub fn canonical_plan(store: &StoreHandle, plans_dir: &Path) -> Result<Plan, CanonicalPlanError> {
    let no_plan = || CanonicalPlanError::NoPlanOpen {
        dir: plans_dir.to_path_buf(),
    };
    if !plans_dir.is_dir() {
        return Err(no_plan());
    }
    let pointer = store().and_then(|handle| {
        yi_session::lock_session(&handle)
            .plan()
            .and_then(|fact| fact.doc)
    });
    let plans = PlanStore::open(plans_dir.to_path_buf())?;
    if let Some(raw) = pointer {
        let id = PlanId::new(raw.as_str())
            .map_err(|cause| CanonicalPlanError::Pointer { id: raw, cause })?;
        return Ok(plans.read(&id)?);
    }
    for id in plans.roots()? {
        let plan = plans.read(&id)?;
        if plan.state == PlanState::Active {
            return Ok(plan);
        }
    }
    Err(no_plan())
}

/// The runnable acceptance of the named todo — a stated acceptance is for
/// models and adjudicates nothing.
pub fn todo_check(plan: &Plan, name: &str) -> Option<String> {
    plan.todos
        .iter()
        .find(|todo| todo.label.as_str() == name)
        .and_then(|todo| todo.delegation.as_ref())
        .and_then(|delegation| match &delegation.accept {
            Check::Command(command) => Some(command.clone()),
            Check::Stated(_) | Check::Other(_) => None,
        })
}

fn count(plan: &Plan, name: TodoStateName) -> usize {
    plan.todos
        .iter()
        .filter(|todo| TodoStateName::of(&todo.state) == name)
        .count()
}

/// One line for the advisor digest header: states and counts only.
pub fn summary_line(plan: &Plan) -> String {
    let mut line = format!(
        "plan {} v{} touched {}: {} ready, {} running, {} blocked, {} done of {}",
        plan.id,
        plan.version.0,
        plan.touched.0,
        plan.ready().len(),
        count(plan, TodoStateName::Running),
        count(plan, TodoStateName::Blocked),
        count(plan, TodoStateName::Done),
        plan.todos.len()
    );
    let failed = count(plan, TodoStateName::Failed);
    if failed > 0 {
        line.push_str(&format!(", {failed} failed"));
    }
    let abandoned = count(plan, TodoStateName::Abandoned);
    if abandoned > 0 {
        line.push_str(&format!(", {abandoned} abandoned"));
    }
    line
}

pub fn accept_text(check: &Check) -> &str {
    match check {
        Check::Command(command) => command,
        Check::Stated(stated) => stated,
        Check::Other(other) => other,
    }
}

pub fn frontier_text(plan: &Plan) -> String {
    if plan.finished() {
        return String::new();
    }
    let mut lines = Vec::new();
    let ready = plan.ready();
    if !ready.is_empty() {
        lines.push("Ready todos (edges satisfied):".to_owned());
        for todo in &ready {
            let mut line = format!("- {}", todo.label);
            if let Some(delegation) = &todo.delegation {
                line.push_str(&format!(" — accept: {}", accept_text(&delegation.accept)));
            }
            lines.push(line);
        }
    }
    for todo in &plan.todos {
        match &todo.state {
            TodoState::Running { by } => {
                lines.push(format!("In progress: {} (by {by})", todo.label));
            }
            TodoState::Blocked { on: _, note } => {
                lines.push(format!("Blocked: {} — {note}", todo.label));
            }
            TodoState::Pending
            | TodoState::Done { .. }
            | TodoState::Failed { .. }
            | TodoState::Abandoned
            | TodoState::Other(_) => {}
        }
    }
    lines.join("\n")
}

/// §12's plan-aware compaction, as an instruction rather than a filter: the ledger names what
/// is load-bearing and the summarizer disposes. Per-todo entry attribution does not exist.
pub fn compaction_directive(plan: &Plan) -> Option<String> {
    if plan.state != PlanState::Active || plan.finished() {
        return None;
    }
    let mut live = Vec::new();
    let mut settled = Vec::new();
    for todo in &plan.todos {
        match &todo.state {
            TodoState::Pending | TodoState::Running { .. } | TodoState::Blocked { .. } => {
                live.push(todo.label.as_str());
            }
            TodoState::Done { .. } | TodoState::Failed { .. } | TodoState::Abandoned => {
                settled.push(todo.label.as_str());
            }
            TodoState::Other(_) => {}
        }
    }
    if live.is_empty() {
        return None;
    }
    let mut text = format!(
        "This session is working plan {}. Material for these todos is still load-bearing and \
         must survive in enough detail to act on: {}.",
        plan.id,
        live.join("; ")
    );
    if !settled.is_empty() {
        text.push_str(&format!(
            " Work on these is finished, so compress it to its outcome: {}.",
            settled.join("; ")
        ));
    }
    text.push_str(" The ledger itself rehydrates from its file; do not restate it.");
    Some(text)
}

fn plan_json(plan: &Plan) -> Result<Value, String> {
    let ready: Vec<&str> = plan
        .ready()
        .iter()
        .map(|todo| todo.label.as_str())
        .collect();
    let mut value = serde_json::to_value(plan).map_err(|error| error.to_string())?;
    if let Some(map) = value.as_object_mut() {
        map.insert("ready".to_owned(), json!(ready));
        map.insert("finished".to_owned(), json!(plan.finished()));
    }
    Ok(value)
}

#[derive(Default)]
struct StaleTracker {
    last_touched: Option<TouchCount>,
    quiet_turns: u64,
    reminded: bool,
}

/// Read-only view of the canonical plan for the session's surfaces, plus the staleness
/// reminder. Mutation lives in the plan tool; this service writes nothing.
pub struct PlanService {
    store: StoreHandle,
    plans_dir: PathBuf,
    deliver: DeliverFn,
    stale: Mutex<StaleTracker>,
    stale_turns: u64,
    on_change: Mutex<Option<PlanChangeHook>>,
    engine: std::sync::OnceLock<(Arc<ops::PlanEngine>, ops::Actor)>,
}

impl PlanService {
    pub fn new(store: StoreHandle, deliver: DeliverFn) -> Self {
        Self {
            store,
            plans_dir: default_plans_dir(),
            deliver,
            stale: Mutex::new(StaleTracker::default()),
            stale_turns: crate::levers::get().plan_stale_turns,
            on_change: Mutex::new(None),
            engine: std::sync::OnceLock::new(),
        }
    }

    pub fn set_engine(&self, engine: Arc<ops::PlanEngine>, actor: ops::Actor) {
        let _first_wiring_wins = self.engine.set((engine, actor));
    }

    pub fn engine(&self) -> Option<(Arc<ops::PlanEngine>, ops::Actor)> {
        self.engine.get().cloned()
    }

    pub fn with_stale_turns(mut self, turns: Option<u64>) -> Self {
        if let Some(turns) = turns.filter(|turns| *turns > 0) {
            self.stale_turns = turns;
        }
        self
    }

    pub fn with_plans_dir(mut self, dir: PathBuf) -> Self {
        self.plans_dir = dir;
        self
    }

    pub fn set_on_change(&self, hook: PlanChangeHook) {
        if let Ok(mut slot) = self.on_change.lock() {
            *slot = Some(hook);
        }
    }

    fn notify_change(&self, plan: &Plan) {
        if let Some(hook) = self
            .on_change
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
        {
            hook(plan);
        }
    }

    pub fn read_plan(&self) -> Result<Plan, CanonicalPlanError> {
        canonical_plan(&self.store, &self.plans_dir)
    }

    pub fn plans_dir(&self) -> &Path {
        &self.plans_dir
    }

    pub fn get(&self) -> Result<Value, String> {
        let plan = self.read_plan().map_err(|error| error.to_string())?;
        plan_json(&plan)
    }

    fn stale_reminder(&self, plan: &Plan) -> Option<String> {
        let mut tracker = self.stale.lock().ok()?;
        if tracker.last_touched != Some(plan.touched) {
            tracker.last_touched = Some(plan.touched);
            tracker.quiet_turns = 0;
            tracker.reminded = false;
            return None;
        }
        tracker.quiet_turns = tracker.quiet_turns.saturating_add(1);
        if tracker.reminded || tracker.quiet_turns < self.stale_turns {
            return None;
        }
        tracker.reminded = true;
        let running: Vec<&str> = plan
            .todos
            .iter()
            .filter(|todo| matches!(todo.state, TodoState::Running { .. }))
            .map(|todo| todo.label.as_str())
            .collect();
        Some(format!(
            "plan stale: untouched for {} turns; {} ready todo(s) unclaimed; in progress: {}. Step the ledger or reconsider the cut.",
            tracker.quiet_turns,
            plan.ready().len(),
            if running.is_empty() {
                "none".to_owned()
            } else {
                running.join(", ")
            }
        ))
    }

    fn touched_moved(&self, plan: &Plan) -> bool {
        self.stale
            .lock()
            .map(|tracker| tracker.last_touched != Some(plan.touched))
            .unwrap_or(false)
    }

    pub fn observe(&self, event: &AgentEvent) {
        if let AgentEvent::MessageEnd {
            message:
                AgentMessage::Assistant {
                    stop_reason: StopReason::Stop | StopReason::ToolUse,
                    ..
                },
        } = event
        {
            let Ok(plan) = self.read_plan() else {
                return;
            };
            if self.touched_moved(&plan) {
                self.notify_change(&plan);
            }
            let finished = plan.finished();
            if let Some(text) = self.stale_reminder(&plan)
                && !finished
            {
                (self.deliver)(
                    AgentMessage::Custom {
                        custom_type: "reminder".to_owned(),
                        content: UserContent::Text(text),
                        display: true,
                        details: None,
                        timestamp: yi_session::now_ms(),
                    },
                    DeliveryMode::Steer,
                );
            }
        }
    }

    pub fn register(self: &Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let service = Arc::clone(self);
        registry.register("plan.get", move |_payload| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                // The read touches the plans directory; keep it off the executor.
                tokio::task::spawn_blocking(move || service.get())
                    .await
                    .map_err(|error| format!("plan.get task failed: {error}"))?
                    .and_then(|value| match value {
                        Value::Object(map) => Ok(map),
                        other => Err(format!("plan payload was not an object: {other}")),
                    })
            })
        });
    }
}

/// Events are observed in a spawned task, mirroring [`crate::goal::attach_goal`].
pub fn attach_plan(
    session: &crate::AgentSession,
    stale_turns: Option<u64>,
    plans_dir: PathBuf,
) -> Arc<PlanService> {
    let steer = session.heartbeat_hook();
    let deliver: DeliverFn = Arc::new(move |message, _mode| {
        steer(message, DeliveryMode::Steer);
    });
    let service = Arc::new(
        PlanService::new(session.store_handle(), deliver)
            .with_stale_turns(stale_turns)
            .with_plans_dir(plans_dir),
    );
    let mut events = session.subscribe();
    let observer = Arc::clone(&service);
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => observer.observe(&event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    service
}
