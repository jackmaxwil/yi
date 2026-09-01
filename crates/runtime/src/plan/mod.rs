use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::plan::{Plan, PlanVersion, SubtaskSpec, Task, TaskId, TaskState};
use yi_types::schedule::DeliveryMode;

use crate::goal::{DeliverFn, StoreHandle};

pub mod yaml;

pub type PlanChangeHook = Arc<dyn Fn(&Plan) + Send + Sync>;

pub const NO_PLAN_ERROR: &str = "No plan exists for this session; create one with plan.create.";
pub const PLAN_EXISTS_ERROR: &str = "An unfinished plan already exists; work its tasks with plan.update, or add tasks with plan.edit.";
/// Weakening the standard is gated; growing it is free (expand-only).
pub const SHRINK_ERROR: &str = "Removing a task or weakening its acceptance requires the user; plan.edit only adds tasks or reopens done ones.";

/// State transitions are one table; Done is a sink except `plan.edit reopen`.
const LEGAL_TRANSITIONS: &[(TaskState, TaskState)] = &[
    (TaskState::Pending, TaskState::Running),
    (TaskState::Pending, TaskState::Done),
    (TaskState::Pending, TaskState::Blocked),
    (TaskState::Running, TaskState::Done),
    (TaskState::Running, TaskState::Blocked),
    (TaskState::Running, TaskState::Pending),
    (TaskState::Blocked, TaskState::Running),
    (TaskState::Blocked, TaskState::Pending),
];

/// A plan untouched this many completed turns while work flows earns one
/// staleness reminder; the latch clears when the plan version moves.
pub const DEFAULT_STALE_TURNS: u64 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escalation {
    Retry,
    ForceChange,
    Ask,
}

/// Invariant: the sole input to this table is a task's consecutive-red count —
/// no path reaching it reads tokens, budget, or elapsed wall, so the abandon
/// threshold is stationary and sunk cost cannot raise it.
const LADDER: [(u8, Escalation); 3] = [
    (1, Escalation::Retry),
    (2, Escalation::ForceChange),
    (3, Escalation::Ask),
];

fn escalation(count: u8) -> Escalation {
    LADDER
        .iter()
        .rev()
        .find(|(threshold, _)| count >= *threshold)
        .map_or(Escalation::Retry, |(_, rung)| *rung)
}

fn escalation_demand(rung: Escalation, count: u8) -> Option<String> {
    let tail = match rung {
        Escalation::Retry => return None,
        Escalation::ForceChange => {
            "change the task's structure — split it into subtasks, backtrack a dep, or ask — do not retry the same approach"
        }
        Escalation::Ask => "stop and ask the user; do not spend another attempt on this approach",
    };
    Some(format!(
        "this check has stayed red through {count} attempts; {tail}"
    ))
}

/// Invariant: the refusal never runs the check, so an unearned attempt costs
/// nothing; every escape it names is reachable at depth 1, which is what keeps
/// a subtask `SPLIT_MAX_DEPTH` refuses from wedging.
fn rung_refusal(rung: Escalation, id: &str, count: u8) -> Option<String> {
    let head = match rung {
        Escalation::Retry => return None,
        Escalation::ForceChange => String::new(),
        Escalation::Ask => " — stop and ask the user".to_owned(),
    };
    Some(format!(
        "done claim for {id} refused without running its check: red through {count} attempts{head}. One structural move buys one more claim, and it has to name {id}: plan.split of {id}, plan.split of its parent, plan.edit add of an investigation task with for_task {id}, or plan.edit reopen of a task {id} assumes."
    ))
}

/// Invariant: a grant buys one claim for the tasks the move actually reshaped
/// and never clears a streak, so the rung a task sits on is monotone in its
/// reds and the price of the next claim stays one move.
fn grant_readmission(plan: &mut Plan, ids: &[TaskId]) {
    for task in &mut plan.tasks {
        if ids.contains(&task.id) && escalation(task.red_count.unwrap_or(0)) != Escalation::Retry {
            task.readmit = Some(true);
        }
    }
}

fn failure_fingerprint(evidence: &str) -> String {
    let collapse = |line: &str| line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut lines = evidence.lines().filter(|line| !line.trim().is_empty());
    let exit = lines.next().map(collapse);
    let last = lines.next_back().map(collapse);
    let mut hasher = DefaultHasher::new();
    exit.hash(&mut hasher);
    last.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// A restructure or a green check starts the next streak at rung one.
fn clear_red(task: &mut Task) {
    task.red_count = None;
    task.red_fingerprint = None;
}

/// Lowered child ids are `{parent}.{n}`, so a parent id already carrying a
/// '.' is a second generation: depth stays 1 until a campaign shows the class.
const SPLIT_MAX_DEPTH: u8 = 1;
const SPLIT_MAX_WIDTH: usize = 4;

/// Invariant: only the guaranteed-wrong — every variant is decided by set
/// arithmetic over the proposal, so no judgment of the plan's merit gates it.
enum SplitRefusal {
    UnknownReadKey { index: usize, key: String },
    OverlappingWrites { a: usize, b: usize, key: String },
    DepthExceeded { depth: u8, max: u8 },
    WidthExceeded { width: usize, max: usize },
    EmptyTitle { index: usize },
    EmptyAcceptance { index: usize },
}

impl std::fmt::Display for SplitRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownReadKey { index, key } => write!(
                formatter,
                "subtask {index}: reads \"{key}\" that only a sibling writes; siblings carry no ordering, so add the key to this subtask's writes or keep the work in one subtask"
            ),
            Self::OverlappingWrites { a, b, key } => write!(
                formatter,
                "subtasks {a} and {b} both write \"{key}\"; sibling write sets must be disjoint"
            ),
            Self::DepthExceeded { depth, max } => write!(
                formatter,
                "split depth {depth} exceeds the maximum {max}; a subtask is not split again — reshape the parent instead"
            ),
            Self::WidthExceeded { width, max } => {
                write!(formatter, "{width} subtasks exceeds the maximum {max}")
            }
            Self::EmptyTitle { index } => {
                write!(formatter, "subtask {index}: title must be non-empty")
            }
            Self::EmptyAcceptance { index } => {
                write!(formatter, "subtask {index}: acceptance must be non-empty")
            }
        }
    }
}

/// Collect-all: one round trip names every reason the proposal cannot lower.
fn validate_split(parent: &Task, subtasks: &[SubtaskSpec]) -> Vec<SplitRefusal> {
    let mut refusals = Vec::new();
    let generation = u8::try_from(parent.id.as_str().matches('.').count())
        .unwrap_or(u8::MAX)
        .saturating_add(1);
    if generation > SPLIT_MAX_DEPTH {
        refusals.push(SplitRefusal::DepthExceeded {
            depth: generation,
            max: SPLIT_MAX_DEPTH,
        });
    }
    if subtasks.len() > SPLIT_MAX_WIDTH {
        refusals.push(SplitRefusal::WidthExceeded {
            width: subtasks.len(),
            max: SPLIT_MAX_WIDTH,
        });
    }
    for (index, spec) in subtasks.iter().enumerate() {
        if spec.title.trim().is_empty() {
            refusals.push(SplitRefusal::EmptyTitle { index });
        }
        if spec.acceptance.trim().is_empty() {
            refusals.push(SplitRefusal::EmptyAcceptance { index });
        }
    }
    for (a, first) in subtasks.iter().enumerate() {
        for (b, second) in subtasks.iter().enumerate().skip(a.saturating_add(1)) {
            for key in first
                .writes
                .iter()
                .filter(|key| second.writes.contains(key))
            {
                refusals.push(SplitRefusal::OverlappingWrites {
                    a,
                    b,
                    key: key.clone(),
                });
            }
        }
    }
    for (index, spec) in subtasks.iter().enumerate() {
        for key in &spec.reads {
            let sibling_writes = subtasks
                .iter()
                .enumerate()
                .any(|(other, sibling)| other != index && sibling.writes.contains(key));
            if sibling_writes && !spec.writes.contains(key) {
                refusals.push(SplitRefusal::UnknownReadKey {
                    index,
                    key: key.clone(),
                });
            }
        }
    }
    refusals
}

fn lower_subtask(parent: &TaskId, index: usize, spec: &SubtaskSpec) -> Task {
    Task {
        id: TaskId(format!("{}.{}", parent.as_str(), index.saturating_add(1))),
        title: spec.title.trim().to_owned(),
        acceptance: spec.acceptance.trim().to_owned(),
        schema: None,
        check: spec
            .check
            .as_deref()
            .map(str::trim)
            .filter(|check| !check.is_empty())
            .map(str::to_owned),
        deps: Vec::new(),
        state: TaskState::Pending,
        blocked_reason: None,
        assignee: None,
        red_count: None,
        red_fingerprint: None,
        readmit: None,
        extra: Map::new(),
    }
}

fn transition_legal(from: &TaskState, to: &TaskState) -> bool {
    LEGAL_TRANSITIONS
        .iter()
        .any(|(legal_from, legal_to)| legal_from == from && legal_to == to)
}

/// One line for the advisor digest header: states and counts only.
pub fn summary_line(plan: &Plan) -> String {
    let count = |state: &TaskState| {
        plan.tasks
            .iter()
            .filter(|task| &task.state == state)
            .count()
    };
    let mut line = format!(
        "plan v{}: {} ready, {} running, {} blocked, {} done of {}",
        plan.version.0,
        plan.frontier().len(),
        count(&TaskState::Running),
        count(&TaskState::Blocked),
        count(&TaskState::Done),
        plan.tasks.len()
    );
    let unchecked: Vec<&str> = plan
        .tasks
        .iter()
        .filter(|task| task.state == TaskState::Done && task.check.is_none())
        .map(|task| task.id.as_str())
        .collect();
    if !unchecked.is_empty() {
        let named: Vec<&str> = unchecked.iter().take(4).copied().collect();
        let mut text = named.join(", ");
        if let Some(more) = unchecked
            .len()
            .checked_sub(named.len())
            .filter(|more| *more > 0)
        {
            text.push_str(&format!(" +{more} more"));
        }
        line.push_str(&format!(", done unchecked: {text}"));
    }
    line
}

pub fn frontier_text(plan: &Plan) -> String {
    if plan.is_finished() {
        return String::new();
    }
    let mut lines = Vec::new();
    let frontier = plan.frontier();
    if !frontier.is_empty() {
        lines.push("Ready tasks (deps satisfied):".to_owned());
        for task in &frontier {
            lines.push(format!(
                "- {}: {} — {}",
                task.id.as_str(),
                task.title,
                task.acceptance
            ));
        }
    }
    for task in &plan.tasks {
        match task.state {
            TaskState::Running => {
                lines.push(format!("In progress: {}: {}", task.id.as_str(), task.title));
            }
            TaskState::Blocked => lines.push(format!(
                "Blocked: {}: {} — {}",
                task.id.as_str(),
                task.title,
                task.blocked_reason
                    .as_deref()
                    .unwrap_or("no reason recorded")
            )),
            _ => {}
        }
    }
    lines.join("\n")
}

fn plan_json(plan: &Plan) -> Value {
    let frontier: Vec<&str> = plan
        .frontier()
        .iter()
        .map(|task| task.id.as_str())
        .collect();
    let mut value = json!(plan);
    if let Some(map) = value.as_object_mut() {
        map.insert("frontier".to_owned(), json!(frontier));
        map.insert("finished".to_owned(), json!(plan.is_finished()));
    }
    value
}

fn parse_task(spec: &Value, index: usize) -> Result<Task, String> {
    let title = spec
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| format!("task {index}: title must be a non-empty string"))?;
    let acceptance = spec
        .get("acceptance")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|acceptance| !acceptance.is_empty())
        .ok_or_else(|| format!("task {index} ({title}): acceptance must be a non-empty string"))?;
    let id = spec
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map_or_else(|| format!("t{}", index.saturating_add(1)), str::to_owned);
    let deps = match spec.get("deps") {
        None => Vec::new(),
        Some(Value::Array(deps)) => deps
            .iter()
            .map(|dep| {
                dep.as_str()
                    .map(|dep| TaskId(dep.to_owned()))
                    .ok_or_else(|| format!("task {id}: deps must be an array of task id strings"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(format!(
                "task {id}: deps must be an array of task id strings"
            ));
        }
    };
    Ok(Task {
        id: TaskId(id),
        title: title.to_owned(),
        acceptance: acceptance.to_owned(),
        schema: spec.get("schema").cloned(),
        check: spec
            .get("check")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|check| !check.is_empty())
            .map(str::to_owned),
        deps,
        state: TaskState::Pending,
        blocked_reason: None,
        assignee: spec
            .get("assignee")
            .and_then(Value::as_str)
            .map(str::to_owned),
        red_count: None,
        red_fingerprint: None,
        readmit: None,
        extra: Map::new(),
    })
}

/// Unique ids, deps resolve, no cycles (Kahn) — checked at every write.
fn validate(tasks: &[Task]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for task in tasks {
        if !seen.insert(task.id.as_str()) {
            return Err(format!("duplicate task id {}", task.id.as_str()));
        }
    }
    for task in tasks {
        for dep in &task.deps {
            if !seen.contains(dep.as_str()) {
                return Err(format!(
                    "task {} depends on unknown task {}",
                    task.id.as_str(),
                    dep.as_str()
                ));
            }
        }
    }
    let mut remaining: Vec<&Task> = tasks.iter().collect();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    loop {
        let before = remaining.len();
        remaining.retain(|task| {
            let ready = task.deps.iter().all(|dep| done.contains(dep.as_str()));
            if ready {
                done.insert(task.id.as_str());
            }
            !ready
        });
        if remaining.is_empty() {
            return Ok(());
        }
        if remaining.len() == before {
            let cycle: Vec<&str> = remaining.iter().map(|task| task.id.as_str()).collect();
            return Err(format!(
                "dependency cycle among tasks: {}",
                cycle.join(", ")
            ));
        }
    }
}

#[derive(Default)]
struct StaleTracker {
    last_version: Option<PlanVersion>,
    quiet_turns: u64,
    reminded: bool,
}

/// Task DAG beside the goal fact. Single writer: every mutation goes through
/// this service on the parent session; children only report.
pub struct PlanService {
    store: StoreHandle,
    deliver: DeliverFn,
    stale: Mutex<StaleTracker>,
    stale_turns: u64,
    on_change: Mutex<Option<PlanChangeHook>>,
}

impl PlanService {
    pub fn new(store: StoreHandle, deliver: DeliverFn) -> Self {
        Self {
            store,
            deliver,
            stale: Mutex::new(StaleTracker::default()),
            stale_turns: DEFAULT_STALE_TURNS,
            on_change: Mutex::new(None),
        }
    }

    pub fn with_stale_turns(mut self, turns: Option<u64>) -> Self {
        if let Some(turns) = turns.filter(|turns| *turns > 0) {
            self.stale_turns = turns;
        }
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

    /// The stored plan, for a surface that renders it rather than a caller
    /// that mutates it.
    pub fn read_plan(&self) -> Option<Plan> {
        let store = (self.store)()?;
        yi_session::lock_session(&store).plan()
    }

    fn write_plan(&self, plan: Plan) -> Result<(), String> {
        let store = (self.store)().ok_or("no session store is attached")?;
        yi_session::lock_session(&store)
            .set_plan(plan)
            .map_err(|error| error.to_string())
    }

    pub fn get(&self) -> Result<Value, String> {
        match self.read_plan() {
            Some(plan) => Ok(plan_json(&plan)),
            None => Err(NO_PLAN_ERROR.to_owned()),
        }
    }

    /// Fails while an unfinished plan exists (replace-by-create is the
    /// delete-every-task shrink; growth goes through plan.edit).
    pub fn create(&self, tasks: &Value) -> Result<Value, String> {
        if let Some(existing) = self.read_plan()
            && !existing.is_finished()
        {
            return Err(PLAN_EXISTS_ERROR.to_owned());
        }
        let specs = tasks
            .as_array()
            .ok_or("plan.create takes an array of task objects")?;
        if specs.is_empty() {
            return Err("a plan needs at least one task".to_owned());
        }
        let tasks = specs
            .iter()
            .enumerate()
            .map(|(index, spec)| parse_task(spec, index))
            .collect::<Result<Vec<_>, _>>()?;
        validate(&tasks)?;
        let now = yi_session::now_ms();
        let plan = Plan {
            version: PlanVersion(1),
            tasks,
            created: now,
            updated: now,
            doc: None,
            extra: Map::new(),
        };
        self.write_plan(plan.clone())?;
        Ok(plan_json(&plan))
    }

    /// The model reports task state; the host verifies a done claim by
    /// running the task's check and validating evidence against its schema.
    /// Blocking, bounded by the check timeout; async callers spawn_blocking.
    pub fn update(
        &self,
        task_id: &str,
        state: &str,
        evidence: Option<&Value>,
        reason: Option<&str>,
    ) -> Result<Value, String> {
        let target = match state {
            "running" => TaskState::Running,
            "done" => TaskState::Done,
            "blocked" => TaskState::Blocked,
            "pending" => TaskState::Pending,
            other => {
                return Err(format!(
                    "plan.update accepts state running|done|blocked|pending, got \"{other}\""
                ));
            }
        };
        let mut plan = self.read_plan().ok_or(NO_PLAN_ERROR)?;
        let position = plan
            .tasks
            .iter()
            .position(|task| task.id.as_str() == task_id)
            .ok_or_else(|| format!("no task {task_id} in the plan"))?;
        let current = plan.tasks[position].state.clone();
        if current == target {
            return Ok(plan_json(&plan));
        }
        if !transition_legal(&current, &target) {
            return Err(format!(
                "illegal transition for {task_id}: {:?} -> {:?} (reopen a done task with plan.edit)",
                current, target
            ));
        }
        if target == TaskState::Blocked && reason.map(str::trim).unwrap_or("").is_empty() {
            return Err(format!("blocking {task_id} requires a reason"));
        }
        // (Pending, Done) skips the Running admission entirely, so the same
        // gate has to sit on it or a checkless leaf completes itself unnamed.
        let unmeasured_admission = target == TaskState::Running
            || (target == TaskState::Done && current == TaskState::Pending);
        if unmeasured_admission
            && plan.tasks[position].check.is_none()
            && reason.map(str::trim).unwrap_or("").is_empty()
        {
            return Err(format!(
                "task {task_id} has no executable check, so it is not admitted; give a task a check when the plan is written, or pass reason to record this one as an ask or an explicitly unmeasured leaf"
            ));
        }
        if target == TaskState::Done {
            let task = &mut plan.tasks[position];
            let reds = task.red_count.unwrap_or(0);
            if task.readmit.take() != Some(true)
                && let Some(refusal) = rung_refusal(escalation(reds), task_id, reds)
            {
                return Err(refusal);
            }
        }
        if target == TaskState::Done
            && let Err(rejection) = self.verify_done(&plan.tasks[position], evidence)
        {
            let escalation = self.record_red(&mut plan.tasks[position], &rejection);
            let task = &mut plan.tasks[position];
            task.state = TaskState::Blocked;
            task.blocked_reason = Some(rejection.clone());
            plan.version = plan.version.bump();
            plan.updated = yi_session::now_ms();
            self.write_plan(plan.clone())?;
            self.notify_change(&plan);
            return Err(format!(
                "done claim for {task_id} rejected; task is now blocked with the evidence: {rejection}{escalation}"
            ));
        }
        let task = &mut plan.tasks[position];
        if target == TaskState::Done {
            clear_red(task);
        }
        task.state = target.clone();
        task.blocked_reason = match target {
            TaskState::Blocked => reason.map(str::trim).map(str::to_owned),
            _ => None,
        };
        plan.version = plan.version.bump();
        plan.updated = yi_session::now_ms();
        self.write_plan(plan.clone())?;
        self.notify_change(&plan);
        Ok(plan_json(&plan))
    }

    /// The streak rides the task, so the plan write that records the rejection
    /// is also what makes the rung survive a resume.
    fn record_red(&self, task: &mut Task, evidence: &str) -> String {
        let fingerprint = failure_fingerprint(evidence);
        let repeated = task.red_fingerprint.as_deref() == Some(fingerprint.as_str());
        let count = task.red_count.unwrap_or(0).saturating_add(1);
        task.red_count = Some(count);
        task.red_fingerprint = Some(fingerprint);

        let mut clauses = Vec::new();
        if let Some(demand) = escalation_demand(escalation(count), count) {
            clauses.push(demand);
        }
        if repeated {
            let deps: Vec<&str> = task.deps.iter().map(TaskId::as_str).collect();
            clauses.push(if deps.is_empty() {
                "same failure twice; re-verify this task's own premise before spending more"
                    .to_owned()
            } else {
                format!(
                    "same failure twice; re-verify the assumption tasks this depends on ({}) before spending more",
                    deps.join(", ")
                )
            });
        }
        if clauses.is_empty() {
            return String::new();
        }
        let text = clauses.join(" — ");
        (self.deliver)(
            AgentMessage::Custom {
                custom_type: "reminder".to_owned(),
                content: UserContent::Text(format!("{}: {text}", task.id.as_str())),
                display: true,
                details: None,
                timestamp: yi_session::now_ms(),
            },
            DeliveryMode::Steer,
        );
        format!(" — {text}")
    }

    fn verify_done(&self, task: &Task, evidence: Option<&Value>) -> Result<(), String> {
        if let Some(schema) = &task.schema {
            let evidence = evidence.ok_or_else(|| {
                format!(
                    "task {} declares an output schema; pass the structured result as evidence",
                    task.id.as_str()
                )
            })?;
            crate::schema::Schema::from_value(schema.clone())
                .validate(evidence)
                .map_err(|error| format!("evidence does not match the task schema: {error}"))?;
        }
        if let Some(check) = &task.check {
            crate::goal::run_check(check, crate::goal::DEFAULT_CHECK_TIMEOUT_MS)?;
        }
        Ok(())
    }

    /// Expand-only surface: add tasks or reopen a done one. Deletion and
    /// acceptance edits are refused (SHRINK_ERROR); the user drives those.
    pub fn edit(&self, action: &str, payload: &Value) -> Result<Value, String> {
        let mut plan = self.read_plan().ok_or(NO_PLAN_ERROR)?;
        let granted: Vec<TaskId> = match action {
            "add" => {
                let specs = payload
                    .get("tasks")
                    .and_then(Value::as_array)
                    .ok_or("plan.edit add takes {tasks: [...]}")?;
                let offset = plan.tasks.len();
                for (index, spec) in specs.iter().enumerate() {
                    plan.tasks
                        .push(parse_task(spec, offset.saturating_add(index))?);
                }
                validate(&plan.tasks)?;
                // An add reshapes no existing task, so it buys a claim only for
                // the one it names; untargeted, it readmitted every stuck task.
                payload
                    .get("for_task")
                    .or_else(|| payload.get("forTask"))
                    .and_then(Value::as_str)
                    .map(|id| vec![TaskId(id.to_owned())])
                    .unwrap_or_default()
            }
            "reopen" => {
                let id = payload
                    .get("task_id")
                    .or_else(|| payload.get("taskId"))
                    .and_then(Value::as_str)
                    .ok_or("plan.edit reopen takes {task_id}")?;
                let task = plan
                    .tasks
                    .iter_mut()
                    .find(|task| task.id.as_str() == id)
                    .ok_or_else(|| format!("no task {id} in the plan"))?;
                if task.state != TaskState::Done {
                    return Err(format!("task {id} is not done; nothing to reopen"));
                }
                task.state = TaskState::Pending;
                task.blocked_reason = None;
                clear_red(task);
                // Reopening a premise is the escape the refusal names for the
                // tasks that assumed it, so the grant follows the dep edge.
                plan.tasks
                    .iter()
                    .filter(|task| task.deps.iter().any(|dep| dep.as_str() == id))
                    .map(|task| task.id.clone())
                    .collect()
            }
            "remove" | "reword" | "replace" => return Err(SHRINK_ERROR.to_owned()),
            other => return Err(format!("unknown plan.edit action {other}; use add|reopen")),
        };
        grant_readmission(&mut plan, &granted);
        plan.version = plan.version.bump();
        plan.updated = yi_session::now_ms();
        self.write_plan(plan.clone())?;
        self.notify_change(&plan);
        Ok(plan_json(&plan))
    }

    /// Invariant: the model proposes subtasks, never topology — ids, deps,
    /// and state are written here, and a refused proposal writes nothing.
    pub fn split(&self, payload: &Value) -> Result<Value, String> {
        let shape =
            "plan.split takes {task_id, subtasks: [{title, acceptance, check?, reads?, writes?}]}";
        let id = payload
            .get("task_id")
            .or_else(|| payload.get("taskId"))
            .and_then(Value::as_str)
            .ok_or(shape)?
            .to_owned();
        let specs = payload.get("subtasks").cloned().ok_or(shape)?;
        let subtasks: Vec<SubtaskSpec> =
            serde_json::from_value(specs).map_err(|error| format!("{shape}: {error}"))?;
        if subtasks.is_empty() {
            return Err(format!("{shape}: a split needs at least one subtask"));
        }
        let mut plan = self.read_plan().ok_or(NO_PLAN_ERROR)?;
        let position = plan
            .tasks
            .iter()
            .position(|task| task.id.as_str() == id)
            .ok_or_else(|| format!("no task {id} in the plan"))?;
        if plan.tasks[position].state == TaskState::Done {
            return Err(format!(
                "task {id} is done; reopen it with plan.edit before splitting it"
            ));
        }
        let refusals = validate_split(&plan.tasks[position], &subtasks);
        if !refusals.is_empty() {
            let lines: Vec<String> = refusals.iter().map(ToString::to_string).collect();
            return Err(format!("split of {id} refused:\n{}", lines.join("\n")));
        }
        let parent_id = plan.tasks[position].id.clone();
        let reds = plan.tasks[position].red_count.unwrap_or(0);
        // A depth-1 child has no dep to reopen and cannot be split, so its own
        // streak is what earns the parent's reshape — the escape the
        // DepthExceeded message promises, and the only one that node has.
        let stuck_children: Vec<TaskId> = plan.tasks[position]
            .deps
            .iter()
            .filter(|dep| {
                plan.task(dep).is_some_and(|child| {
                    escalation(child.red_count.unwrap_or(0)) != Escalation::Retry
                })
            })
            .cloned()
            .collect();
        if escalation(reds) == Escalation::Retry && stuck_children.is_empty() {
            return Err(format!(
                "split of {id} refused: its check has come back red {reds} time(s) and no subtask of it has earned a rung, so the ladder still reads retry — attempt the whole task, and split when a red streak forces the change"
            ));
        }
        // Incident: a second split of the same parent minted `t1.1` again and
        // `validate` rejected the whole reshape; child ids continue the run.
        let prefix = format!("{}.", parent_id.as_str());
        let offset = plan
            .tasks
            .iter()
            .filter(|task| task.id.as_str().starts_with(&prefix))
            .count();
        let children: Vec<Task> = subtasks
            .iter()
            .enumerate()
            .map(|(index, spec)| lower_subtask(&parent_id, offset.saturating_add(index), spec))
            .collect();
        plan.tasks[position]
            .deps
            .extend(children.iter().map(|child| child.id.clone()));
        plan.tasks.extend(children);
        validate(&plan.tasks)?;
        clear_red(&mut plan.tasks[position]);
        grant_readmission(&mut plan, &stuck_children);
        plan.version = plan.version.bump();
        plan.updated = yi_session::now_ms();
        self.write_plan(plan.clone())?;
        self.notify_change(&plan);
        Ok(plan_json(&plan))
    }

    fn stale_reminder(&self, plan: &Plan) -> Option<String> {
        let mut tracker = self.stale.lock().ok()?;
        if tracker.last_version != Some(plan.version) {
            tracker.last_version = Some(plan.version);
            tracker.quiet_turns = 0;
            tracker.reminded = false;
            return None;
        }
        tracker.quiet_turns = tracker.quiet_turns.saturating_add(1);
        if tracker.reminded || tracker.quiet_turns < self.stale_turns {
            return None;
        }
        tracker.reminded = true;
        let frontier = plan.frontier().len();
        let running: Vec<&str> = plan
            .tasks
            .iter()
            .filter(|task| task.state == TaskState::Running)
            .map(|task| task.id.as_str())
            .collect();
        Some(format!(
            "plan stale: untouched for {} turns; {frontier} ready task(s) unclaimed; in progress: {}. Update task states or reconsider orchestration.",
            tracker.quiet_turns,
            if running.is_empty() {
                "none".to_owned()
            } else {
                running.join(", ")
            }
        ))
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
            let Some(plan) = self.read_plan() else {
                return;
            };
            if plan.is_finished() {
                return;
            }
            if let Some(text) = self.stale_reminder(&plan) {
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
            let outcome = service.get();
            Box::pin(async move { outcome.and_then(as_object) })
        });
        let service = Arc::clone(self);
        registry.register("plan.create", move |payload| {
            let tasks = payload.get("tasks").cloned().unwrap_or(Value::Null);
            let outcome = service.create(&tasks);
            Box::pin(async move { outcome.and_then(as_object) })
        });
        let service = Arc::clone(self);
        registry.register("plan.update", move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let state = payload
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let evidence = payload.get("evidence").cloned();
            let reason = payload
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let service = Arc::clone(&service);
            Box::pin(async move {
                // A done claim may run the task check; keep it off the executor.
                tokio::task::spawn_blocking(move || {
                    service.update(&task_id, &state, evidence.as_ref(), reason.as_deref())
                })
                .await
                .map_err(|error| format!("plan.update task failed: {error}"))?
                .and_then(as_object)
            })
        });
        let service = Arc::clone(self);
        registry.register("plan.edit", move |payload| {
            let action = payload
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let outcome = service.edit(&action, &Value::Object(payload));
            Box::pin(async move { outcome.and_then(as_object) })
        });
        let service = Arc::clone(self);
        registry.register("plan.split", move |payload| {
            let outcome = service.split(&Value::Object(payload));
            Box::pin(async move { outcome.and_then(as_object) })
        });
    }
}

fn as_object(value: Value) -> Result<Map<String, Value>, String> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(format!("plan payload was not an object: {other}")),
    }
}

/// Events are observed in a spawned task, mirroring [`crate::goal::attach_goal`].
pub fn attach_plan(session: &crate::AgentSession, stale_turns: Option<u64>) -> Arc<PlanService> {
    let steer = session.heartbeat_hook();
    let deliver: DeliverFn = Arc::new(move |message, _mode| {
        steer(message, DeliveryMode::Steer);
    });
    let service =
        Arc::new(PlanService::new(session.store_handle(), deliver).with_stale_turns(stale_turns));
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
