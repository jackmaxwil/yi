pub mod template;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Map, Value, json};
use yi_types::event::AgentEvent;
use yi_types::goal::{Goal, GoalStatus};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::schedule::DeliveryMode;

pub const CONTINUATION_TEMPLATE: &str = include_str!("prompts/continuation.md");
pub const BUDGET_LIMIT_TEMPLATE: &str = include_str!("prompts/budget_limit.md");
pub const OBJECTIVE_UPDATED_TEMPLATE: &str = include_str!("prompts/objective_updated.md");

pub const GOAL_EXISTS_ERROR: &str = "A goal already exists and is not finished; use goal.update(status=\"complete\"|\"blocked\") to finish it first.";
pub const NO_GOAL_ERROR: &str = "No goal exists for this session; create one with goal.create.";

pub type StoreHandle = Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync>;
pub type DeliverFn = Arc<dyn Fn(AgentMessage, DeliveryMode) + Send + Sync>;

fn budget_values(goal: &Goal) -> [(&'static str, String); 3] {
    let budget = goal
        .token_budget
        .map_or_else(|| "unlimited".to_owned(), |budget| budget.to_string());
    let remaining = goal.token_budget.map_or_else(
        || "unlimited".to_owned(),
        |budget| budget.saturating_sub(goal.tokens_used).to_string(),
    );
    [
        ("tokens_used", goal.tokens_used.to_string()),
        ("token_budget", budget),
        ("remaining_tokens", remaining),
    ]
}

pub fn continuation_text(goal: &Goal) -> Result<String, template::TemplateError> {
    let mut values = vec![("objective", goal.objective.clone())];
    values.extend(budget_values(goal));
    template::render(CONTINUATION_TEMPLATE, values)
}

pub fn budget_limit_text(goal: &Goal) -> Result<String, template::TemplateError> {
    let [used, budget, _remaining] = budget_values(goal);
    template::render(
        BUDGET_LIMIT_TEMPLATE,
        vec![
            ("objective", goal.objective.clone()),
            ("time_used_seconds", goal.time_used_seconds.to_string()),
            used,
            budget,
        ],
    )
}

pub fn objective_updated_text(goal: &Goal) -> Result<String, template::TemplateError> {
    let mut values = vec![("objective", goal.objective.clone())];
    values.extend(budget_values(goal));
    template::render(OBJECTIVE_UPDATED_TEMPLATE, values)
}

fn goal_prompt_message(text: String) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: "goal_prompt".to_owned(),
        content: UserContent::Text(text),
        display: true,
        details: None,
        timestamp: yi_session::now_ms(),
    }
}

fn goal_json(goal: &Goal) -> Value {
    let mut value = json!(goal);
    if let (Some(map), Some(budget)) = (value.as_object_mut(), goal.token_budget) {
        map.insert(
            "remainingTokens".to_owned(),
            json!(budget.saturating_sub(goal.tokens_used)),
        );
    }
    value
}

/// Uncached input plus output.
fn usage_delta(usage: &yi_types::message::Usage) -> u64 {
    let uncached = usage.input.saturating_sub(usage.cache_read).max(0);
    let output = usage.output.max(0);
    u64::try_from(uncached.saturating_add(output)).unwrap_or(0)
}

/// State machine over the session store fact, idle continuation and budget
/// accounting. One per session.
pub struct GoalService {
    store: StoreHandle,
    deliver: DeliverFn,
    /// Set by an abort: auto-continuation stops until the next real user input.
    deferred: Mutex<bool>,
    /// Cleared at AgentStart so idle cannot double-fire.
    pending: Mutex<bool>,
    wall_mark: Mutex<Option<Instant>>,
}

impl GoalService {
    pub fn new(store: StoreHandle, deliver: DeliverFn) -> Self {
        Self {
            store,
            deliver,
            deferred: Mutex::new(false),
            pending: Mutex::new(false),
            wall_mark: Mutex::new(None),
        }
    }

    fn read_goal(&self) -> Option<Goal> {
        let store = (self.store)()?;
        yi_session::lock_session(&store).goal()
    }

    fn write_goal(&self, goal: Goal) -> Result<(), String> {
        let store = (self.store)().ok_or("no session store is attached")?;
        yi_session::lock_session(&store)
            .set_goal(goal)
            .map_err(|error| error.to_string())
    }

    pub fn get(&self) -> Result<Value, String> {
        match self.read_goal() {
            Some(goal) => Ok(goal_json(&goal)),
            None => Err(NO_GOAL_ERROR.to_owned()),
        }
    }

    /// Fails while an unfinished goal exists.
    pub fn create(&self, objective: &str, token_budget: Option<u64>) -> Result<Value, String> {
        if objective.trim().is_empty() {
            return Err("objective must not be empty".to_owned());
        }
        if let Some(existing) = self.read_goal()
            && !existing.status.is_terminal()
        {
            return Err(GOAL_EXISTS_ERROR.to_owned());
        }
        let now = yi_session::now_ms();
        let goal = Goal {
            objective: objective.to_owned(),
            status: GoalStatus::Active,
            token_budget,
            tokens_used: 0,
            time_used_seconds: 0,
            created: now,
            updated: now,
            extra: Map::new(),
        };
        self.write_goal(goal.clone())?;
        if let Ok(mut mark) = self.wall_mark.lock() {
            *mark = Some(Instant::now());
        }
        Ok(goal_json(&goal))
    }

    /// The model reports terminal state only; pause, resume and limits belong
    /// to the host.
    pub fn update(&self, status: &str) -> Result<Value, String> {
        let status = match status {
            "complete" => GoalStatus::Complete,
            "blocked" => GoalStatus::Blocked,
            other => {
                return Err(format!(
                    "goal.update accepts status \"complete\" or \"blocked\", got \"{other}\""
                ));
            }
        };
        let mut goal = self.read_goal().ok_or(NO_GOAL_ERROR)?;
        goal.status = status;
        goal.updated = yi_session::now_ms();
        self.write_goal(goal.clone())?;
        Ok(goal_json(&goal))
    }

    /// Supersedes the objective and steers `objective_updated` into the turn.
    pub fn set_objective(&self, objective: &str) -> Result<Value, String> {
        if objective.trim().is_empty() {
            return Err("objective must not be empty".to_owned());
        }
        let mut goal = self.read_goal().ok_or(NO_GOAL_ERROR)?;
        goal.objective = objective.to_owned();
        goal.status = GoalStatus::Active;
        goal.updated = yi_session::now_ms();
        self.write_goal(goal.clone())?;
        if let Ok(text) = objective_updated_text(&goal) {
            (self.deliver)(goal_prompt_message(text), DeliveryMode::Steer);
        }
        if let Ok(mut deferred) = self.deferred.lock() {
            *deferred = false;
        }
        Ok(goal_json(&goal))
    }

    fn set_flag(flag: &Mutex<bool>, value: bool) {
        if let Ok(mut slot) = flag.lock() {
            *slot = value;
        }
    }

    fn flag(flag: &Mutex<bool>) -> bool {
        flag.lock().map(|slot| *slot).unwrap_or(false)
    }

    fn account(&self, usage: &yi_types::message::Usage) {
        let Some(mut goal) = self.read_goal() else {
            return;
        };
        if !matches!(goal.status, GoalStatus::Active | GoalStatus::BudgetLimited) {
            return;
        }
        goal.tokens_used = goal.tokens_used.saturating_add(usage_delta(usage));
        if let Ok(mut mark) = self.wall_mark.lock() {
            if let Some(started) = mark.take() {
                goal.time_used_seconds = goal
                    .time_used_seconds
                    .saturating_add(started.elapsed().as_secs());
            }
            *mark = Some(Instant::now());
        }
        let crossed = goal.status == GoalStatus::Active
            && goal
                .token_budget
                .is_some_and(|budget| goal.tokens_used >= budget);
        if crossed {
            goal.status = GoalStatus::BudgetLimited;
        }
        goal.updated = yi_session::now_ms();
        if self.write_goal(goal.clone()).is_err() {
            return;
        }
        // One-shot by construction: the Active→BudgetLimited transition
        // happens exactly once per goal.
        if crossed && let Ok(text) = budget_limit_text(&goal) {
            (self.deliver)(goal_prompt_message(text), DeliveryMode::Steer);
        }
    }

    fn block_after_error(&self) {
        let Some(mut goal) = self.read_goal() else {
            return;
        };
        if !goal.status.is_active() {
            return;
        }
        goal.status = GoalStatus::Blocked;
        goal.updated = yi_session::now_ms();
        let _best_effort = self.write_goal(goal);
    }

    /// Idle with an Active goal and no deferral queues the continuation prompt,
    /// through the same hook heartbeats use.
    fn continue_if_idle(&self) {
        if Self::flag(&self.deferred) || Self::flag(&self.pending) {
            return;
        }
        let Some(goal) = self.read_goal() else {
            return;
        };
        if !goal.status.is_active() {
            return;
        }
        let Ok(text) = continuation_text(&goal) else {
            return;
        };
        Self::set_flag(&self.pending, true);
        (self.deliver)(goal_prompt_message(text), DeliveryMode::FollowUp);
    }

    pub fn observe(&self, event: &AgentEvent) {
        match event {
            AgentEvent::AgentStart => Self::set_flag(&self.pending, false),
            AgentEvent::MessageStart {
                message: AgentMessage::User { .. },
            } => Self::set_flag(&self.deferred, false),
            AgentEvent::MessageEnd {
                message:
                    AgentMessage::Assistant {
                        usage, stop_reason, ..
                    },
            } => match stop_reason {
                StopReason::Aborted => Self::set_flag(&self.deferred, true),
                // Codex on_turn_error: block the goal so automatic
                // continuation cannot loop against a failing turn.
                StopReason::Error => self.block_after_error(),
                _ => self.account(usage),
            },
            AgentEvent::AgentEnd { .. } => self.continue_if_idle(),
            _ => {}
        }
    }

    pub fn register(self: &Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let service = Arc::clone(self);
        registry.register("goal.get", move |_payload| {
            let outcome = service.get();
            Box::pin(async move { outcome.and_then(as_object) })
        });
        let service = Arc::clone(self);
        registry.register("goal.create", move |payload| {
            let objective = payload
                .get("objective")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let budget = payload.get("token_budget").and_then(Value::as_u64);
            let outcome = service.create(&objective, budget);
            Box::pin(async move { outcome.and_then(as_object) })
        });
        let service = Arc::clone(self);
        registry.register("goal.update", move |payload| {
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let outcome = service.update(&status);
            Box::pin(async move { outcome.and_then(as_object) })
        });
    }
}

fn as_object(value: Value) -> Result<Map<String, Value>, String> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(format!("goal payload was not an object: {other}")),
    }
}

/// Events are observed in a spawned task; the caller registers the host surface.
pub fn attach_goal(session: &crate::AgentSession) -> Arc<GoalService> {
    let steer = session.heartbeat_hook();
    let wake = session.wake_idle_hook();
    let deliver: DeliverFn = Arc::new(move |message, mode| match mode {
        DeliveryMode::Steer => steer(message, DeliveryMode::Steer),
        DeliveryMode::FollowUp => wake(message),
    });
    let service = Arc::new(GoalService::new(session.store_handle(), deliver));
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
