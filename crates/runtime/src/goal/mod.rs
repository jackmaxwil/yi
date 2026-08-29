pub mod template;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Map, Value, json};
use yi_types::event::AgentEvent;
use yi_types::goal::{Goal, GoalStatus};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::schedule::DeliveryMode;
use yi_types::subagent::Discovery;

pub const CONTINUATION_TEMPLATE: &str = include_str!("prompts/continuation.md");
pub const BUDGET_LIMIT_TEMPLATE: &str = include_str!("prompts/budget_limit.md");
pub const OBJECTIVE_UPDATED_TEMPLATE: &str = include_str!("prompts/objective_updated.md");

pub const GOAL_EXISTS_ERROR: &str = "A goal already exists and is not finished; use goal.update(status=\"complete\"|\"blocked\") to finish it first.";
pub const NO_GOAL_ERROR: &str = "No goal exists for this session; create one with goal.create.";

/// Checks are integration gates (`just check`-class), not unit runs; 10 min
/// covers a cold cargo build without letting a hang eat the session.
pub const DEFAULT_CHECK_TIMEOUT_MS: u64 = 600_000;
const CHECK_TAIL_CHARS: usize = 2_000;

fn output_tail(capture: &yi_tools::CommandCapture) -> String {
    let mut combined = String::new();
    if !capture.stdout.trim().is_empty() {
        combined.push_str(capture.stdout.trim_end());
    }
    if !capture.stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(capture.stderr.trim_end());
    }
    let start = combined
        .char_indices()
        .rev()
        .nth(CHECK_TAIL_CHARS.saturating_sub(1))
        .map_or(0, |(index, _)| index);
    combined.get(start..).unwrap_or(&combined).to_owned()
}

/// Exit 0 is the only pass; the Err carries the model-facing evidence.
pub(crate) fn run_check(check: &str, timeout_ms: u64) -> Result<(), String> {
    let mut command = yi_tools::command("sh");
    command.arg("-c").arg(check);
    let deadline = Instant::now()
        .checked_add(std::time::Duration::from_millis(timeout_ms))
        .unwrap_or_else(Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || Instant::now() >= deadline);
    let capture = yi_tools::run_captured(command, None, &cancelled, 30_000)
        .map_err(|error| format!("goal check failed to run: {error}"))?;
    if capture.cancelled {
        return Err(format!("goal check timed out after {timeout_ms} ms"));
    }
    match capture.exit_code {
        Some(0) => Ok(()),
        code => {
            let exit = code.map_or_else(|| "signal".to_owned(), |code| code.to_string());
            Err(format!(
                "goal check `{check}` exited {exit}:\n{}",
                output_tail(&capture)
            ))
        }
    }
}

pub type StoreHandle = Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync>;
pub type DeliverFn = Arc<dyn Fn(AgentMessage, DeliveryMode) + Send + Sync>;

pub fn task_check(plan: &yi_types::plan::Plan, id: &str) -> Option<String> {
    plan.tasks
        .iter()
        .find(|task| task.id.as_str() == id)
        .and_then(|task| task.check.clone())
}

/// L5 ledger writer: a derived-HIGH discovery becomes a goal fact, so it
/// survives compaction and resume and is still there to block completion.
pub fn record_discovery(store: &StoreHandle, row: &Discovery) -> Result<(), String> {
    let handle = store().ok_or("no session store is attached")?;
    let mut session = yi_session::lock_session(&handle);
    let mut goal = session.goal().ok_or(NO_GOAL_ERROR)?;
    if goal
        .discoveries
        .iter()
        .any(|kept| kept.fingerprint == row.fingerprint)
    {
        return Ok(());
    }
    goal.discoveries.push(row.clone());
    goal.updated = yi_session::now_ms();
    session.set_goal(goal).map_err(|error| error.to_string())
}

fn drained_message(row: &Discovery, mut details: Value) -> AgentMessage {
    if let Some(map) = details.as_object_mut() {
        map.insert("discovery".to_owned(), json!(row));
    }
    AgentMessage::Custom {
        custom_type: "discovery".to_owned(),
        content: UserContent::Text(format!(
            "drained discovery {}: {}",
            row.fingerprint, row.text
        )),
        display: true,
        details: Some(details),
        timestamp: yi_session::now_ms(),
    }
}

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

pub fn continuation_text(
    goal: &Goal,
    plan: Option<&yi_types::plan::Plan>,
) -> Result<String, template::TemplateError> {
    let check_status = goal
        .check_failure
        .as_ref()
        .map_or_else(String::new, |failure| {
            format!("The last completion claim was rejected by the goal check:\n{failure}\n")
        });
    let plan_frontier = plan.map_or_else(String::new, |plan| {
        let text = crate::plan::frontier_text(plan);
        if text.is_empty() {
            String::new()
        } else {
            format!("Plan state:\n{text}\n")
        }
    });
    let mut values = vec![
        ("objective", goal.objective.clone()),
        ("check_status", check_status),
        ("plan_frontier", plan_frontier),
    ];
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

    fn read_plan(&self) -> Option<yi_types::plan::Plan> {
        let store = (self.store)()?;
        yi_session::lock_session(&store).plan()
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
    pub fn create(
        &self,
        objective: &str,
        token_budget: Option<u64>,
        check: Option<String>,
        check_timeout_ms: Option<u64>,
    ) -> Result<Value, String> {
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
            check: check.filter(|command| !command.trim().is_empty()),
            check_timeout_ms,
            check_failure: None,
            discoveries: Vec::new(),
            extra: Map::new(),
        };
        self.write_goal(goal.clone())?;
        if let Ok(mut mark) = self.wall_mark.lock() {
            *mark = Some(Instant::now());
        }
        Ok(goal_json(&goal))
    }

    /// The model reports terminal state only; the host verifies a `complete`
    /// report by running the goal check before accepting it. Blocking,
    /// bounded by the check timeout — async callers wrap in spawn_blocking.
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
        if status == GoalStatus::Complete
            && let Some(undrained) = self.drain(&mut goal)
        {
            goal.updated = yi_session::now_ms();
            self.write_goal(goal)?;
            return Err(format!(
                "completion rejected: {undrained}\nDrain each row by making the check it names pass, then call goal.update again."
            ));
        }
        if status == GoalStatus::Complete
            && let Some(check) = goal.check.clone()
        {
            let timeout = goal.check_timeout_ms.unwrap_or(DEFAULT_CHECK_TIMEOUT_MS);
            if let Err(evidence) = run_check(&check, timeout) {
                goal.check_failure = Some(evidence.clone());
                goal.updated = yi_session::now_ms();
                self.write_goal(goal)?;
                return Err(format!(
                    "completion rejected: {evidence}\nFix the failure (or correct the claim), then call goal.update again."
                ));
            }
            goal.check_failure = None;
        }
        goal.status = status;
        goal.updated = yi_session::now_ms();
        self.write_goal(goal.clone())?;
        Ok(goal_json(&goal))
    }

    /// L5 drain gate: every recorded row is re-adjudicated by re-running the
    /// check it names. Green — or a check the plan no longer carries — drains
    /// the row; red keeps it and returns the refusal naming every blocker.
    fn drain(&self, goal: &mut Goal) -> Option<String> {
        if goal.discoveries.is_empty() {
            return None;
        }
        let plan = self.read_plan();
        let mut kept = Vec::new();
        let mut blockers = Vec::new();
        for row in std::mem::take(&mut goal.discoveries) {
            let named = row.violates_check_of.clone().and_then(|id| {
                plan.as_ref()
                    .and_then(|plan| task_check(plan, &id))
                    .map(|check| (id, check))
            });
            let message = match named {
                Some((id, check)) => match run_check(&check, DEFAULT_CHECK_TIMEOUT_MS) {
                    Ok(()) => drained_message(&row, json!({ "drained": true, "task": id })),
                    Err(evidence) => {
                        blockers.push(format!(
                            "undrained HIGH discovery {}: {}\nThe check of task {id} is still red:\n{evidence}",
                            row.fingerprint, row.text
                        ));
                        kept.push(row);
                        continue;
                    }
                },
                None => drained_message(
                    &row,
                    json!({ "drained": true, "reason": "the check it named is no longer in the plan" }),
                ),
            };
            (self.deliver)(message, DeliveryMode::Steer);
        }
        goal.discoveries = kept;
        (!blockers.is_empty()).then(|| blockers.join("\n\n"))
    }

    /// Supersedes the objective and steers `objective_updated` into the turn.
    pub fn set_objective(&self, objective: &str) -> Result<Value, String> {
        if objective.trim().is_empty() {
            return Err("objective must not be empty".to_owned());
        }
        let mut goal = self.read_goal().ok_or(NO_GOAL_ERROR)?;
        goal.objective = objective.to_owned();
        goal.status = GoalStatus::Active;
        goal.check_failure = None;
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
        let Ok(text) = continuation_text(&goal, self.read_plan().as_ref()) else {
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
            let check = payload
                .get("check")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let check_timeout = payload.get("check_timeout_ms").and_then(Value::as_u64);
            let outcome = service.create(&objective, budget, check, check_timeout);
            Box::pin(async move { outcome.and_then(as_object) })
        });
        let service = Arc::clone(self);
        registry.register("goal.update", move |payload| {
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let service = Arc::clone(&service);
            Box::pin(async move {
                // The check run blocks up to its timeout; keep it off the
                // executor so kernel pumps stay live.
                tokio::task::spawn_blocking(move || service.update(&status))
                    .await
                    .map_err(|error| format!("goal.update task failed: {error}"))?
                    .and_then(as_object)
            })
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
