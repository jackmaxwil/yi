mod check;

pub(crate) use check::{CheckShell, captured};

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Map, Value, json};
use yi_types::event::AgentEvent;
use yi_types::goal::{Goal, GoalStatus};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::schedule::DeliveryMode;
use yi_types::subagent::Discovery;

use crate::args::Args;

pub(crate) static CONTINUATION: std::sync::LazyLock<String> =
    crate::packed::packed!("continuation");
pub(crate) static BUDGET_LIMIT: std::sync::LazyLock<String> =
    crate::packed::packed!("budget_limit");
pub(crate) static OBJECTIVE_UPDATED: std::sync::LazyLock<String> =
    crate::packed::packed!("objective_updated");

pub const GOAL_EXISTS_ERROR: &str = "A goal already exists and is not finished; use goal.update(status=\"complete\"|\"blocked\") to finish it first.";
pub const NO_GOAL_ERROR: &str = "No goal exists for this session; create one with goal.create.";

/// Checks are integration gates (`just check`-class), not unit runs; 10 min
/// covers a cold cargo build without letting a hang eat the session.
pub const DEFAULT_CHECK_TIMEOUT_MS: u64 = 600_000;

/// A failed check's output through [`yi_tools::view`]: a cut row's recovery is the rerun from
/// `dir`, or a file under `spill`; the capture's own cut row follows the view's.
pub(crate) fn check_view(
    command: &str,
    dir: &std::path::Path,
    spill: Option<&std::path::Path>,
    capture: &yi_tools::CommandCapture,
) -> String {
    let (stdout, stderr) = (&capture.stdout, &capture.stderr);
    let exit = capture.exit_code.unwrap_or(-1);
    let rerun = rerun_in(dir, command);
    let producer = yi_tools::Involuntary::Job(&rerun);
    let view = yi_tools::view(producer, stdout, stderr, exit, None, spill);
    let text = view.text.trim();
    let rows = capture.rows.join("\n");
    let body = if rows.is_empty() {
        text.to_owned()
    } else {
        format!("{text}\n{rows}")
    };
    capture
        .cut_row()
        .map_or_else(|| body.clone(), |row| format!("{body}\n{row}"))
}

/// The rerun a cut row names: `command` from `dir`, where the check ran. The reader's own bash
/// is rooted elsewhere for a child's lane or a `cwd_subdir`, and a bare command would pass there.
pub(crate) fn rerun_in(dir: &std::path::Path, command: &str) -> String {
    let dir = dir.to_string_lossy().replace('\'', r"'\''");
    format!("cd '{dir}' && {command}")
}

const REPEAT_BYTES: usize = 2_000;

/// A later copy of evidence the model already read whole: its first line and last
/// [`REPEAT_BYTES`], where the verdict sits, with a row for the cut.
pub(crate) fn evidence_repeat(evidence: &str) -> String {
    if evidence.len() <= REPEAT_BYTES {
        return evidence.to_owned();
    }
    let head: String = evidence
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    let from = evidence.ceil_char_boundary(evidence.len().saturating_sub(REPEAT_BYTES));
    let tail = evidence.get(from..).unwrap_or_default();
    let (kept, total, cap) = (tail.len(), evidence.len(), "REPEAT_BYTES");
    let next = "the refusal that first carried this output, or run the check again";
    let row = yi_tools::cut_row(kept, total, "bytes kept", cap, REPEAT_BYTES, next);
    format!(
        "{head}
{row}
{tail}"
    )
}

pub(crate) fn evidence_once(first: bool, evidence: String) -> String {
    if first {
        evidence
    } else {
        "(the same evidence as the first row naming this task, above)".to_owned()
    }
}

/// Why a check did not pass, typed so a caller can release instead of re-prompting when the
/// failure is not the model's to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckFailure {
    /// A Claude Code shell hook refused the command.
    Refused(String),
    Spawn(String),
    TimedOut(u64),
    /// The command ran and its exit was not 0; the evidence is the viewed capture.
    Red(String),
}

impl CheckFailure {
    pub fn evidence(&self) -> String {
        match self {
            Self::Refused(why) => why.clone(),
            Self::Spawn(error) => format!("goal check failed to run: {error}"),
            Self::TimedOut(ms) => format!("goal check timed out after {ms} ms"),
            Self::Red(evidence) => evidence.clone(),
        }
    }
}
/// What a check runs under: the project's Claude Code hooks and the session's sandbox.
#[derive(Clone, Default)]
pub struct Checker {
    hooks: Option<Arc<crate::claude_hooks::Hooks>>,
    broker: Option<Arc<crate::permission::PermissionBroker>>,
    wall: crate::wall::Wall,
}

/// The lease's proxy variables: the one place a spawned check learns the proxy's address.
fn lease_env(lease: Option<&crate::egress::Lease>) -> impl Iterator<Item = (String, String)> + '_ {
    lease.into_iter().flat_map(crate::egress::Lease::env)
}

impl Checker {
    pub fn new(
        hooks: Option<Arc<crate::claude_hooks::Hooks>>,
        broker: Option<Arc<crate::permission::PermissionBroker>>,
        wall: crate::wall::Wall,
    ) -> Self {
        Self {
            hooks,
            broker,
            wall,
        }
    }

    /// Hooks with no sandbox, as a session with no broker has.
    pub fn from_hooks(hooks: Option<Arc<crate::claude_hooks::Hooks>>) -> Self {
        Self::new(hooks, None, crate::wall::Wall::default())
    }

    pub(crate) fn guard(&self, command: &str, cwd: &std::path::Path) -> Result<(), String> {
        crate::claude_hooks::guard_shell(self.hooks.as_deref(), command, cwd)
    }

    /// Incident: a check ran on the host, so `cargo test` as one ran a child's `build.rs` outside
    /// the sandbox the same command met in bash. Yolo keeps its promise and contains nothing.
    fn profile(&self, cwd: &std::path::Path) -> Option<yi_tools::Sandbox> {
        let broker = self.broker.as_ref()?;
        if broker.mode() == yi_permission::PermissionMode::Yolo {
            return None;
        }
        broker.sandbox_for(cwd, &self.wall, &broker.kept_writes())
    }

    /// `shell -c script` in `cwd`: in the session's container, else in the sandbox where one holds,
    /// with that spawn's proxy lease. Past a failed lease a check has no network, as its output says.
    pub(crate) fn shell(&self, shell: &str, script: &str, cwd: &std::path::Path) -> CheckShell {
        // Invariant: a container child's commands never run on the host, a check included.
        let placed = (self.wall.container.as_deref())
            .map(|container| yi_tools::jobs::in_container(container, cwd, script));
        let (run, sweep) = placed.map_or((None, None), |(run, sweep)| (Some(run), Some(sweep)));
        let profile = run.is_none().then(|| self.profile(cwd)).flatten();
        let lease = (profile.as_ref().and(self.broker.as_ref()))
            .and_then(|broker| broker.egress_lease(self.wall.clone()).ok());
        let (program, args) = match (run, profile) {
            (Some(run), _) => (shell.to_owned(), vec!["-c".to_owned(), run]),
            (None, Some(sandbox)) => sandbox.wrap(shell, &["-c", script]),
            (None, None) => (shell.to_owned(), vec!["-c".to_owned(), script.to_owned()]),
        };
        let mut command = yi_tools::keyless_command(program);
        command.args(args).current_dir(cwd);
        command.envs(lease_env(lease.as_ref()));
        CheckShell {
            command,
            lease,
            sweep,
        }
    }

    /// What the sandbox stopped in this run, in the words a contained bash failure gets.
    pub(crate) fn refusal(
        &self,
        cwd: &std::path::Path,
        script: &str,
        capture: &yi_tools::CommandCapture,
    ) -> Option<String> {
        let sandbox = self.profile(cwd)?;
        let output = format!("{}{}", capture.stdout, capture.stderr);
        yi_tools::sandbox_refusal(&sandbox, cwd, capture.exit_code, &output, script)
            .map(|refusal| yi_tools::denial_hint(&refusal))
    }
}

/// Exit 0 is the only pass; the Err carries the model-facing evidence.
pub(crate) fn run_check(
    check: &str,
    cwd: &std::path::Path,
    timeout_ms: u64,
    checker: &Checker,
) -> Result<(), String> {
    run_check_typed_under(check, cwd, timeout_ms, checker).map_err(|failed| failed.evidence())
}

/// [`run_check`] without a session's hooks, its failure still typed: `yi ask --check` releases
/// on a spawn or a timeout and only re-prompts a check that ran and was red.
///
/// # Errors
/// Any check that is not exit 0, with the reason it did not run.
pub fn run_check_typed(
    check: &str,
    cwd: &std::path::Path,
    timeout_ms: u64,
) -> Result<(), CheckFailure> {
    run_check_typed_under(check, cwd, timeout_ms, &Checker::from_hooks(None))
}

fn run_check_typed_under(
    check: &str,
    cwd: &std::path::Path,
    timeout_ms: u64,
    checker: &Checker,
) -> Result<(), CheckFailure> {
    let runner = ShellCheck::new(check, cwd, checker)
        .within(timeout_ms)
        .probe();
    runner.guard().map_err(CheckFailure::Refused)?;
    let capture = runner.capture().map_err(CheckFailure::Spawn)?;
    if capture.cancelled {
        return Err(CheckFailure::TimedOut(timeout_ms));
    }
    match capture.exit_code {
        Some(0) => Ok(()),
        code => {
            let exit = code.map_or_else(|| "signal".to_owned(), |code| code.to_string());
            let contained = checker
                .refusal(cwd, check, &capture)
                .map_or_else(String::new, |why| format!("[contained: {why}]\n"));
            Err(CheckFailure::Red(format!(
                "{contained}goal check `{check}` exited {exit}:\n{}",
                check_view(check, cwd, None, &capture)
            )))
        }
    }
}

/// Invariant: a check is refused before it runs when the shell cannot parse it or a segment's
/// first word is not a program in `cwd`, so an exit 127 is never read as the check's verdict.
pub fn probe(command: &str, cwd: &std::path::Path) -> Option<String> {
    let mut syntax = yi_tools::keyless_command("sh");
    syntax.arg("-n").arg("-c").arg(command).current_dir(cwd);
    let parsed = probe_capture(syntax)?;
    if parsed.exit_code != Some(0) {
        let first = parsed.stderr.lines().next().unwrap_or("no diagnostic");
        let exit = parsed
            .exit_code
            .map_or_else(|| "signal".to_owned(), |code| code.to_string());
        return Some(format!(
            "the check `{command}` cannot run: the shell cannot parse it (exit {exit}): {first}
The ways out: fix the syntax, or declare no check."
        ));
    }
    let words = probe_words(command);
    if words.is_empty() {
        return None;
    }
    let mut scan = yi_tools::keyless_command("sh");
    scan.arg("-c")
        .arg(PROBE_WORDS)
        .arg("_")
        .args(&words)
        .current_dir(cwd);
    let found = probe_capture(scan)?;
    let (word, code) = found.stdout.lines().next()?.split_once('\t')?;
    let code: i32 = code.parse().ok()?;
    let reason = match code {
        126 => "a path that is not an executable file",
        _ => "not on the PATH",
    };
    Some(format!(
        "the check `{command}` cannot run: `{word}` is not runnable (exit {code}: {reason}).
Only the first word of each segment is probed, so a compound command is checked to that depth.
The ways out: fix the syntax, install the program, or declare no check."
    ))
}

const PROBE_WORDS: &str = "for w; do case $w in */*) if [ ! -e \"$w\" ]; then printf '%s\\t127\\n' \"$w\"; elif [ ! -f \"$w\" ] || [ ! -x \"$w\" ]; then printf '%s\\t126\\n' \"$w\"; fi;; *) command -v -- \"$w\" >/dev/null 2>&1 || printf '%s\\t127\\n' \"$w\";; esac; done";

fn probe_capture(command: std::process::Command) -> Option<yi_tools::CommandCapture> {
    let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
    yi_tools::run_captured(command, None, &cancelled, yi_tools::OUTPUT_CAP).ok()
}

/// The first word of each segment an assignment prefix does not name: `FOO=1 true` starts `true`.
/// A word the shell expands is not probed, and an expansion the splitter cannot read skips them all.
fn probe_words(command: &str) -> Vec<String> {
    let segments = match yi_permission::parse(command) {
        yi_permission::Parsed::Segments(segments) => segments,
        yi_permission::Parsed::Unparsed if command.contains(['$', '`']) => Vec::new(),
        yi_permission::Parsed::Unparsed => yi_permission::command_segments(command),
    };
    segments
        .into_iter()
        .filter_map(|argv| argv.into_iter().find(|token| !probe_assignment(token)))
        .filter(|word| !word.contains(['$', '`', '*']))
        .collect()
}

fn probe_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The environment a checker may see beyond what its manifest declares (plan section 6.3).
pub const CHECKER_ENV_BASE: [&str; 4] = ["PATH", "HOME", "LANG", "TMPDIR"];

mod shell_check;
pub(crate) use shell_check::{ShellCheck, run_check_in};

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

/// L5 ledger writer: a derived-HIGH discovery becomes a goal fact, so it survives compaction
/// and resume to block completion. `false` means no goal exists, so no completion gate holds.
pub fn record_discovery(store: &StoreHandle, row: &Discovery) -> Result<bool, String> {
    let handle = store().ok_or("no session store is attached")?;
    let mut session = yi_session::lock_session(&handle);
    let Some(mut goal) = session.goal() else {
        return Ok(false);
    };
    if goal
        .discoveries
        .iter()
        .any(|kept| kept.fingerprint == row.fingerprint)
    {
        return Ok(true);
    }
    goal.discoveries.push(row.clone());
    goal.updated = yi_session::now_ms();
    session.set_goal(goal).map_err(|error| error.to_string())?;
    Ok(true)
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

/// Invariant: one scan of the template, so a `{{ name }}` inside a value (the objective,
/// check output, a plan label) is copied verbatim, never filled.
fn fill(template: &str, goal: &Goal, extra: Vec<(&'static str, String)>) -> String {
    let mut values = budget_values(goal).to_vec();
    values.push(("time_used_seconds", goal.time_used_seconds.to_string()));
    values.push(("objective", goal.objective.clone()));
    values.extend(extra);
    let lookup = |name: &str| values.iter().find(|(key, _)| *key == name).map(|(_, v)| v);
    let mut text = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((head, tail)) = rest.split_once("{{ ") {
        text.push_str(head);
        let filled = tail
            .split_once(" }}")
            .and_then(|(name, after)| Some((lookup(name)?, after)));
        let Some((value, after)) = filled else {
            text.push_str("{{ ");
            rest = tail;
            continue;
        };
        text.push_str(value);
        rest = after;
    }
    text.push_str(rest);
    text
}

pub fn continuation_text(goal: &Goal, plan: Option<&yi_types::plan::doc::Plan>) -> String {
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
    let extra = vec![
        ("check_status", check_status),
        ("plan_frontier", plan_frontier),
    ];
    fill(CONTINUATION.as_str(), goal, extra)
}

pub fn budget_limit_text(goal: &Goal) -> String {
    fill(BUDGET_LIMIT.as_str(), goal, Vec::new())
}

pub fn objective_updated_text(goal: &Goal) -> String {
    fill(OBJECTIVE_UPDATED.as_str(), goal, Vec::new())
}

/// The objective-edit and budget notices ride the continuation kind but are never superseded.
fn goal_notice_message(text: String) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: "goal_prompt".to_owned(),
        content: UserContent::Text(text),
        display: true,
        details: Some(json!({ yi_context::wrapper::NOTICE_DETAIL: true })),
        timestamp: yi_session::now_ms(),
    }
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
    checker: Checker,
}

impl GoalService {
    pub fn new(store: StoreHandle, deliver: DeliverFn, cwd: std::path::PathBuf) -> Self {
        Self {
            store,
            plans_dir: crate::plan::default_plans_dir(),
            cwd,
            deliver,
            deferred: Mutex::new(false),
            checker: Checker::default(),
            pending: Mutex::new(false),
            wall_mark: Mutex::new(None),
        }
    }

    pub fn with_plans_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.plans_dir = dir;
        self
    }

    pub fn with_checker(mut self, checker: Checker) -> Self {
        self.checker = checker;
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
            if let Err(evidence) = run_check(&check, &self.cwd, timeout, &self.checker) {
                goal.check_failure = Some(evidence_repeat(&evidence));
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
        let mut shown = HashSet::new();
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
                            run_check(&check, &self.cwd, DISCOVERY_CHECK_TIMEOUT_MS, &self.checker)
                                .err()
                        })
                        .clone();
                    match evidence {
                        None => {
                            drained_message(&row, json!({ "drained": true, "task": id.as_str() }))
                        }
                        Some(evidence) => {
                            let evidence = evidence_once(shown.insert(id.clone()), evidence);
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
        (self.deliver)(
            goal_notice_message(objective_updated_text(&goal)),
            DeliveryMode::Steer,
        );
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
        goal.tokens_used = goal.tokens_used.saturating_add(usage.fresh());
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
        if crossed {
            (self.deliver)(
                goal_notice_message(budget_limit_text(&goal)),
                DeliveryMode::Steer,
            );
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
        let text = continuation_text(&goal, self.read_plan().ok().as_ref());
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
    let service = Arc::new(
        GoalService::new(session.store_handle(), deliver, cwd.clone())
            .with_plans_dir(plans_dir)
            .with_checker(session.checker(&cwd)),
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

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::shell_check::Env;
    use super::{CHECKER_ENV_BASE, Checker, ShellCheck};

    fn names(env_dump: &str) -> Vec<&str> {
        env_dump
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn the_env_policy_decides_what_the_check_sees() -> Result<(), Box<dyn Error>> {
        let cwd = std::env::current_dir()?;
        let declared = ["DUP_SMOKE_UNSET".to_owned()];
        let allowlisted = ShellCheck::new("env", &cwd, &Checker::default())
            .env(Env::Allowlist(&declared))
            .run()?;
        let seen = names(&allowlisted.stdout);
        assert!(seen.contains(&"PATH"), "{seen:?}");
        let allowed: Vec<&str> = CHECKER_ENV_BASE
            .iter()
            .copied()
            .chain(declared.iter().map(String::as_str))
            .collect();
        // `/bin/sh` sets these itself after `env_clear()`; anything else is inheritance.
        let shell_own = ["PWD", "SHLVL", "OLDPWD", "_"];
        for name in &seen {
            assert!(
                allowed.contains(name) || shell_own.contains(name),
                "{name} reached the allowlisted check: {seen:?}"
            );
        }
        let host: Vec<String> = std::env::vars_os()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .filter(|name| !allowed.contains(&name.as_str()))
            .filter(|name| !shell_own.contains(&name.as_str()))
            .collect();
        for name in &host {
            assert!(
                !seen.contains(&name.as_str()),
                "{name} was inherited past the allowlist: {seen:?}"
            );
        }
        let keyless = ShellCheck::new("env", &cwd, &Checker::default()).run()?;
        let inherited = names(&keyless.stdout);
        assert!(
            inherited.len() >= seen.len(),
            "the keyless check inherits what the allowlisted one was denied: {inherited:?}"
        );
        Ok(())
    }

    #[test]
    fn a_check_past_its_deadline_is_cancelled() -> Result<(), Box<dyn Error>> {
        let cwd = std::env::current_dir()?;
        let capture = ShellCheck::new("sleep 5", &cwd, &Checker::default())
            .within(50)
            .run()?;
        assert!(capture.cancelled, "{capture:?}");
        Ok(())
    }
}

#[cfg(test)]
mod checker_tests {
    use super::*;

    #[test]
    fn a_probe_refusal_stays_typed_through_the_shared_check()
    -> Result<(), Box<dyn std::error::Error>> {
        let failure = run_check_typed(
            "yi-missing-check-2009f3389",
            &std::env::current_dir()?,
            30_000,
        )
        .err()
        .ok_or("a missing check program passed")?;
        assert!(matches!(failure, CheckFailure::Refused(_)), "{failure:?}");
        Ok(())
    }

    /// Dies with a container child's check running on the host, which that child's wall forbids.
    #[test]
    fn a_container_childs_check_runs_in_its_container() {
        let wall = crate::wall::Wall {
            container: Some("yi-child".to_owned()),
            ..crate::wall::Wall::default()
        };
        let checker = Checker::new(None, None, wall);
        let shell = checker.shell("sh", "cargo test", std::path::Path::new("/work"));
        assert!(shell.lease.is_none());
        assert!(shell.sweep.is_some(), "a killed check must be swept");
        let args: Vec<String> = (shell.command.get_args())
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let run = args.join(" ");
        assert!(
            run.contains("docker exec") && run.contains("yi-child"),
            "{run}"
        );
        assert!(run.contains("cargo test"), "{run}");
    }

    /// Dies with a check running on the host: `cargo test` as one ran a child's `build.rs` outside
    /// the sandbox the same command met in bash.
    #[test]
    fn a_check_cannot_write_outside_the_tree_and_says_so() -> Result<(), Box<dyn std::error::Error>>
    {
        if !yi_tools::Sandbox::available() {
            return Ok(());
        }
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .ok_or("no HOME")?;
        let tree = std::env::temp_dir().join(format!("yi-check-tree-{}", std::process::id()));
        std::fs::create_dir_all(&tree)?;
        let outside = home.join(format!("yi-check-outside-{}", std::process::id()));
        let broker = crate::permission::PermissionBroker::new(
            yi_permission::PermissionMode::Auto,
            tree.clone(),
            Vec::new(),
            None,
            tokio::sync::broadcast::channel(8).0,
        )
        .with_sandbox(Some(yi_tools::Sandbox::for_workspace(&tree, &home, None)));
        let checker = Checker::new(None, Some(Arc::new(broker)), crate::wall::Wall::default());
        let check = format!("touch {}", outside.display());
        let refused = run_check(&check, &tree, 30_000, &checker);
        let wrote = outside.exists();
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&tree);
        assert!(!wrote, "the check wrote outside its tree");
        let evidence = refused.err().ok_or("the refused check passed")?;
        assert!(evidence.starts_with("[contained: "), "{evidence}");
        run_check("true", &std::env::temp_dir(), 30_000, &checker).map_err(|error| error.into())
    }

    /// Dies with no lease on a check: `cargo test` as one could not fetch a seeded crate, or, on
    /// the second path, with the environment clear running after the proxy was set.
    #[test]
    fn a_check_is_handed_its_proxy_on_both_paths() -> Result<(), Box<dyn std::error::Error>> {
        if !yi_tools::Sandbox::available() {
            return Ok(());
        }
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .ok_or("no HOME")?;
        let tree = std::env::temp_dir();
        let broker = crate::permission::PermissionBroker::new(
            yi_permission::PermissionMode::Auto,
            tree.clone(),
            Vec::new(),
            None,
            tokio::sync::broadcast::channel(8).0,
        )
        .with_sandbox(Some(yi_tools::Sandbox::for_workspace(&tree, &home, None)));
        let checker = Checker::new(None, Some(Arc::new(broker)), crate::wall::Wall::default());
        let script = "test -n \"$HTTPS_PROXY\"";
        run_check(script, &tree, 30_000, &checker).map_err(|error| error.to_string())?;
        let deadline = Instant::now()
            .checked_add(std::time::Duration::from_secs(30))
            .ok_or("no deadline")?;
        let capture = run_check_in(&tree, script, &[], None, deadline, None, &checker)?;
        assert_eq!(capture.exit_code, Some(0));
        Ok(())
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::{CheckShell, captured};
    use std::sync::Arc;

    /// Dies with `captured` dropping its sweep: a cancelled check leaves what it started in the
    /// container writing the tree, which is the invariant `jobs::run_sweep` exists for.
    #[test]
    fn a_cancelled_check_is_swept() -> Result<(), Box<dyn std::error::Error>> {
        let dir = crate::scratch::Scratch::new("yi-swept")?;
        let marker = dir.join("swept");
        let mut command = yi_tools::keyless_command("sh");
        command.args(["-c", "sleep 5"]);
        let shell = CheckShell {
            command,
            lease: None,
            sweep: Some(format!("touch '{}'", marker.display())),
        };
        let cancelled: yi_tools::CancelFlag = Arc::new(|| true);
        let capture = captured(shell, None, &cancelled)?;
        assert!(
            capture.cancelled,
            "the flag trips a check that never finishes"
        );
        assert!(marker.exists(), "a cancelled check's sweep ran");
        Ok(())
    }

    /// Incident: a sweep that exited nonzero was reported as a SWEEP_BOUND timeout, hiding the
    /// failure and telling the model a hang happened when none did.
    #[test]
    fn a_failed_sweep_names_its_exit_code_not_the_time_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut command = yi_tools::keyless_command("sh");
        command.args(["-c", "sleep 5"]);
        let shell = CheckShell {
            command,
            lease: None,
            sweep: Some("exit 23".to_owned()),
        };
        let cancelled: yi_tools::CancelFlag = Arc::new(|| true);
        let capture = captured(shell, None, &cancelled)?;
        assert!(capture.cancelled);
        let rows = capture.rows.join("\n");
        assert!(rows.contains("sweep failed with exit code 23"), "{rows}");
        assert!(!rows.contains("SWEEP_BOUND"), "{rows}");
        Ok(())
    }
}
