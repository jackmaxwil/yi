pub mod template;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Map, Value, json};
use yi_types::event::AgentEvent;
use yi_types::goal::{Goal, GoalStatus};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::schedule::DeliveryMode;
use yi_types::subagent::Discovery;

use crate::args::Args;

pub const CONTINUATION_TEMPLATE: &str = include_str!("prompts/continuation.md");
pub const BUDGET_LIMIT_TEMPLATE: &str = include_str!("prompts/budget_limit.md");
pub const OBJECTIVE_UPDATED_TEMPLATE: &str = include_str!("prompts/objective_updated.md");

pub const GOAL_EXISTS_ERROR: &str = "A goal already exists and is not finished; use goal.update(status=\"complete\"|\"blocked\") to finish it first.";
pub const NO_GOAL_ERROR: &str = "No goal exists for this session; create one with goal.create.";

/// Checks are integration gates (`just check`-class), not unit runs; 10 min
/// covers a cold cargo build without letting a hang eat the session.
pub const DEFAULT_CHECK_TIMEOUT_MS: u64 = 600_000;
const CHECK_TAIL_CHARS: usize = 2_000;

pub(crate) fn output_tail(capture: &yi_tools::CommandCapture) -> String {
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
pub(crate) fn run_check(check: &str, cwd: &std::path::Path, timeout_ms: u64) -> Result<(), String> {
    let mut command = yi_tools::command("sh");
    command.arg("-c").arg(check).current_dir(cwd);
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

/// The environment a checker may see beyond what its manifest declares (plan section 6.3).
pub const CHECKER_ENV_BASE: [&str; 4] = ["PATH", "HOME", "LANG", "TMPDIR"];

/// # Errors
/// The shell did not spawn. Runs `/bin/sh -c` with `env_clear()` plus the base allowlist.
pub(crate) fn run_check_in(
    cwd: &std::path::Path,
    command: &str,
    env_names: &[String],
    stdin: Option<Vec<u8>>,
    deadline: Instant,
    stop: Option<&yi_tools::CancelFlag>,
) -> Result<yi_tools::CommandCapture, String> {
    let mut shell = yi_tools::command("/bin/sh");
    shell.arg("-c").arg(command).current_dir(cwd).env_clear();
    for name in CHECKER_ENV_BASE
        .iter()
        .map(|name| (*name).to_owned())
        .chain(env_names.iter().cloned())
    {
        if let Some(value) = std::env::var_os(&name) {
            shell.env(name, value);
        }
    }
    let stop = stop.cloned();
    let cancelled: yi_tools::CancelFlag =
        Arc::new(move || Instant::now() >= deadline || stop.as_ref().is_some_and(|stop| stop()));
    yi_tools::run_captured(shell, stdin, &cancelled, yi_tools::OUTPUT_CAP)
}

pub type StoreHandle = Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GoalEditError {
    #[error(
        "goal edit refused: no citation; cite the user:// message that authorizes the new objective"
    )]
    Uncited,
    #[error("goal edit refused: citation {citation} is not a user:// address")]
    NotUser { citation: String },
    #[error("goal edit refused: {citation} does not resolve: {detail}")]
    Unresolved { citation: String, detail: String },
}
pub type DeliverFn = Arc<dyn Fn(AgentMessage, DeliveryMode) + Send + Sync>;

/// A discovery's check is one todo's gate, not the goal's integration gate: re-run per row
/// and per completion attempt, so it gets a per-row budget, not [`DEFAULT_CHECK_TIMEOUT_MS`].
pub(crate) const DISCOVERY_CHECK_TIMEOUT_MS: u64 = 60_000;

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
    plan: Option<&yi_types::plan::doc::Plan>,
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
    AgentMessage::host_note("goal_prompt", text, yi_session::now_ms())
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
pub(crate) fn usage_delta(usage: &yi_types::message::Usage) -> u64 {
    let uncached = usage.input.saturating_sub(usage.cache_read).max(0);
    let output = usage.output.max(0);
    u64::try_from(uncached.saturating_add(output)).unwrap_or(0)
}

/// State machine over the session store fact, idle continuation and budget
/// accounting. One per session.
pub struct GoalService {
    store: StoreHandle,
    plans_dir: std::path::PathBuf,
    cwd: std::path::PathBuf,
    deliver: DeliverFn,
    /// Set by an abort: auto-continuation stops until the next real user input.
    deferred: Mutex<bool>,
    /// Cleared at AgentStart so idle cannot double-fire.
    pending: Mutex<bool>,
    wall_mark: Mutex<Option<Instant>>,
}

impl GoalService {
    pub fn new(store: StoreHandle, deliver: DeliverFn, cwd: std::path::PathBuf) -> Self {
        Self {
            store,
            plans_dir: crate::plan::default_plans_dir(),
            cwd,
            deliver,
            deferred: Mutex::new(false),
            pending: Mutex::new(false),
            wall_mark: Mutex::new(None),
        }
    }

    pub fn with_plans_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.plans_dir = dir;
        self
    }

    fn read_goal(&self) -> Option<Goal> {
        let store = (self.store)()?;
        yi_session::lock_session(&store).goal()
    }

    fn read_plan(&self) -> Result<yi_types::plan::doc::Plan, crate::plan::CanonicalPlanError> {
        crate::plan::canonical_plan(&self.store, &self.plans_dir)
    }

    fn write_goal(&self, goal: Goal) -> Result<(), String> {
        let store = (self.store)().ok_or("no session store is attached")?;
        yi_session::lock_session(&store)
            .set_goal(goal)
            .map_err(|error| error.to_string())
    }

    pub fn act(&self, params: &Map<String, Value>) -> Result<Value, String> {
        let text = |key| params.str_of(key).unwrap_or_default();
        match text("action") {
            "get" => self.get(),
            "create" => self.create(
                text("objective"),
                params.u64_of("tokenBudget"),
                params.str_of("check").map(str::to_owned),
                params.u64_of("checkTimeoutMs"),
            ),
            "update" => self.update(text("status")),
            "objective" => self.set_objective(text("objective"), params.str_of("citation")),
            other => Err(format!(
                "unknown goal action {other}; use get|create|update|objective"
            )),
        }
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

    /// The model reports terminal state only; the host verifies `complete` by running the
    /// goal check first. Blocking, bounded by the check timeout; async callers spawn_blocking.
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
            if let Err(evidence) = run_check(&check, &self.cwd, timeout) {
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

    /// L5 drain gate: every recorded row is re-adjudicated by re-running the check it names.
    /// Green, or a dropped check, drains it; red keeps it, and an unreadable plan refuses.
    fn drain(&self, goal: &mut Goal) -> Option<String> {
        if goal.discoveries.is_empty() {
            return None;
        }
        let plan = match self.read_plan() {
            Ok(plan) => plan,
            Err(cause) => {
                return Some(format!(
                    "cannot adjudicate {} recorded discovery row(s): {cause}, so a row that was confirmed red cannot be shown drained",
                    goal.discoveries.len()
                ));
            }
        };
        // One check run per named todo, however many rows name it.
        let mut adjudged: HashMap<yi_types::plan::TaskId, Option<String>> = HashMap::new();
        let mut kept = Vec::new();
        let mut blockers = Vec::new();
        for row in std::mem::take(&mut goal.discoveries) {
            let named = row.violates_check_of.clone().and_then(|id| {
                crate::plan::todo_check(&plan, id.as_str()).map(|check| (id, check))
            });
            let message = match named {
                Some((id, check)) => {
                    let evidence = adjudged
                        .entry(id.clone())
                        .or_insert_with(|| {
                            run_check(&check, &self.cwd, DISCOVERY_CHECK_TIMEOUT_MS).err()
                        })
                        .clone();
                    match evidence {
                        None => {
                            drained_message(&row, json!({ "drained": true, "task": id.as_str() }))
                        }
                        Some(evidence) => {
                            blockers.push(format!(
                                "undrained HIGH discovery {}: {}\nThe check of task {} is still red:\n{evidence}",
                                row.fingerprint,
                                row.text,
                                id.as_str()
                            ));
                            kept.push(row);
                            continue;
                        }
                    }
                }
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

    /// Supersedes the objective and steers `objective_updated` into the turn. Invariant: it
    /// carries user authority, so the edit demands a `user://` citation (D25).
    pub fn set_objective(&self, objective: &str, citation: Option<&str>) -> Result<Value, String> {
        if objective.trim().is_empty() {
            return Err("objective must not be empty".to_owned());
        }
        self.verify_citation(citation)
            .map_err(|error| error.to_string())?;
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

    fn verify_citation(&self, citation: Option<&str>) -> Result<(), GoalEditError> {
        let Some(citation) = citation else {
            return Err(GoalEditError::Uncited);
        };
        let unresolved = |detail: String| GoalEditError::Unresolved {
            citation: citation.to_owned(),
            detail,
        };
        let url: yi_types::url::Url = citation
            .parse()
            .map_err(|error: yi_types::url::UrlError| unresolved(error.to_string()))?;
        if url.scheme() != &yi_types::url::Scheme::User {
            return Err(GoalEditError::NotUser {
                citation: citation.to_owned(),
            });
        }
        let ordinal: usize = url
            .path()
            .parse()
            .ok()
            .filter(|ordinal| *ordinal > 0)
            .ok_or_else(|| {
                unresolved(format!(
                    "the path must be a 1-based user-message ordinal, got {}",
                    url.path()
                ))
            })?;
        let store =
            (self.store)().ok_or_else(|| unresolved("no session store is attached".to_owned()))?;
        let held = crate::fetch::user_inputs(&store).map_err(unresolved)?.len();
        if ordinal > held {
            return Err(unresolved(format!(
                "the transcript holds {held} attributed user message(s)"
            )));
        }
        Ok(())
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
        let Ok(text) = continuation_text(&goal, self.read_plan().ok().as_ref()) else {
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
                // Block the goal so automatic continuation cannot loop
                // against a failing turn.
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
            let objective = payload.str_of("objective").unwrap_or("").to_owned();
            let budget = payload.u64_of("token_budget");
            let check = payload.str_of("check").map(str::to_owned);
            let check_timeout = payload.u64_of("check_timeout_ms");
            let outcome = service.create(&objective, budget, check, check_timeout);
            Box::pin(async move { outcome.and_then(as_object) })
        });
        let service = Arc::clone(self);
        registry.register("goal.update", move |payload| {
            let status = payload.str_of("status").unwrap_or("").to_owned();
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
pub fn attach_goal(
    session: &crate::AgentSession,
    plans_dir: std::path::PathBuf,
    cwd: std::path::PathBuf,
) -> Arc<GoalService> {
    let deliver: DeliverFn = session.heartbeat_hook();
    let service =
        Arc::new(GoalService::new(session.store_handle(), deliver, cwd).with_plans_dir(plans_dir));
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
