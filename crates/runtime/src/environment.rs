use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, ENVIRONMENT_TAG};

use crate::session::{AgentSession, EnvironmentFn};
use crate::subagent::SubagentHost;
use crate::wiring::RuntimeWiring;

pub(crate) const PROBE: Duration = Duration::from_secs(5);

type Summary = Option<(String, usize, usize)>;

/// The branch from the one HEAD reader, and the (modified, untracked) counts from `git status`.
pub fn git_summary(cwd: &Path) -> Summary {
    let head = crate::lane::head(cwd).ok()?;
    let out = crate::lane::capture(cwd, "git", &["status", "--porcelain"], PROBE).ok()?;
    let (mut modified, mut untracked) = (0usize, 0usize);
    for line in out.lines() {
        if line.starts_with("??") {
            untracked += 1;
        } else {
            modified += 1;
        }
    }
    Some((sanitize(&head.label()), modified, untracked))
}

const WRITERS: [&str; 4] = ["bash", "edit", "write", "ipython"];

/// [`git_summary`] read again only once a run starts or a writing tool ends: the model's own
/// change shows at the next request; a same-tree child's or one made outside Yi, later.
pub struct GitSummary {
    cwd: PathBuf,
    held: Mutex<(broadcast::Receiver<AgentEvent>, Option<Summary>)>,
}

impl GitSummary {
    pub fn new(cwd: PathBuf, events: broadcast::Receiver<AgentEvent>) -> Self {
        Self {
            cwd,
            held: Mutex::new((events, None)),
        }
    }

    /// Invariant: the lock is held across the read, so callers at once run `git status` once.
    /// ponytail: a reply past the channel's capacity lags the receiver and costs one more read.
    pub fn read(&self) -> Summary {
        use broadcast::error::TryRecvError;
        let Ok(mut held) = self.held.lock() else {
            return git_summary(&self.cwd);
        };
        let (events, cached) = &mut *held;
        loop {
            match events.try_recv() {
                Ok(AgentEvent::AgentStart) | Err(TryRecvError::Lagged(_)) => *cached = None,
                Ok(AgentEvent::ToolExecutionEnd {
                    tool_name, result, ..
                }) if WRITERS.contains(&tool_name.as_str())
                    || result
                        .details
                        .get("applied")
                        .and_then(serde_json::Value::as_u64)
                        > Some(0) =>
                {
                    *cached = None;
                }
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        if let Some(summary) = cached.clone() {
            return summary;
        }
        let read = git_summary(&self.cwd);
        *cached = read.is_some().then(|| read.clone());
        read
    }
}

/// The cwd line's trailing git clause: zero counts are omitted, but a clean tree keeps the
/// pre-existing "0 modified" rather than printing neither.
pub fn git_line(branch: &str, modified: usize, untracked: usize) -> String {
    let mut parts = Vec::new();
    if modified > 0 {
        parts.push(format!("{modified} modified"));
    }
    if untracked > 0 {
        parts.push(format!("{untracked} untracked"));
    }
    if parts.is_empty() {
        parts.push("0 modified".to_owned());
    }
    format!(" (git: {branch}, {})", parts.join(", "))
}

pub fn sanitize(text: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "._/-".contains(c);
    if text.is_empty() || text.chars().any(|c| !plain(c)) {
        return "?".to_owned();
    }
    text.chars().take(64).collect()
}

fn local_time(cwd: &Path) -> Option<String> {
    let out = crate::lane::capture(cwd, "date", &["+%Y-%m-%d %H:%M %Z"], PROBE).ok()?;
    let time = out.trim();
    (!time.is_empty()).then(|| time.to_owned())
}

/// The line shows minutes, so `date` runs once per minute of wall clock, not once per request.
pub fn time_per_minute(
    last: &Mutex<Option<(u64, Option<String>)>>,
    now_ms: u64,
    read: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let minute = now_ms / 60_000;
    let Ok(mut last) = last.lock() else {
        return read();
    };
    if let Some((at, time)) = last.as_ref()
        && *at == minute
    {
        return time.clone();
    }
    let time = read();
    *last = Some((minute, time.clone()));
    time
}

fn tokens(count: u64) -> String {
    match count {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        n if n >= 10_000 => format!("{}K", n / 1000),
        n if n >= 1000 => format!("{:.1}K", n as f64 / 1000.0),
        n => n.to_string(),
    }
}

pub const FILES_SHOWN: usize = 20;

/// The cwd's top level, sorted, hidden names skipped, capped: the oracle beside the spec.
pub fn files_line(cwd: &Path) -> Option<String> {
    let entries = std::fs::read_dir(cwd).ok()?;
    let mut hidden = 0usize;
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                hidden = hidden.saturating_add(1);
                return None;
            }
            let dir = entry.file_type().ok()?.is_dir();
            Some(if dir {
                format!("{}/", sanitize(&name))
            } else {
                sanitize(&name)
            })
        })
        .collect();
    if names.is_empty() {
        return None;
    }
    names.sort();
    let total = names.len();
    names.truncate(FILES_SHOWN);
    let mut line = format!("files: {}", names.join(" "));
    if total > FILES_SHOWN {
        let what = format!("top-level names, {hidden} hidden skipped");
        let (cap, next) = ("FILES_SHOWN", "ls -A");
        let row = crate::listed::cut_row(FILES_SHOWN, total, &what, cap, FILES_SHOWN, next);
        line.push_str(&format!(" {row}"));
    }
    Some(line)
}

pub fn deadline_line(total: Duration, elapsed: Duration) -> String {
    format!(
        "deadline: {}s left of {}s",
        total.saturating_sub(elapsed).as_secs(),
        total.as_secs()
    )
}

pub fn render(lines: &[String]) -> String {
    format!("{ENVIRONMENT_TAG}\n{}\n</environment>", lines.join("\n"))
}

/// The block's time, model id, cwd and branch: never its counts, context or cost, which the
/// model still has to source.
pub fn printed_facts(block: &str) -> String {
    let mut facts = String::new();
    for line in block.lines() {
        let fact = if let Some(time) = line.strip_prefix("time: ") {
            Some(time)
        } else if let Some(model) = line.strip_prefix("model: ") {
            model.split(" · ").next()
        } else if let Some(cwd) = line.strip_prefix("cwd: ") {
            let (path, git) = cwd.split_once(" (git: ").unwrap_or((cwd, ""));
            facts.push_str(path);
            facts.push('\n');
            git.split(',').next()
        } else {
            None
        };
        if let Some(fact) = fact {
            facts.push_str(fact);
            facts.push('\n');
        }
    }
    facts
}

pub fn message(block: String) -> AgentMessage {
    yi_context::wrap_internal("environment", &block, 0)
}

/// What the turn says about the machine: os, arch, shell, and where yi runs (D209).
pub fn platform_line(shell: &str) -> String {
    format!(
        "platform: {} {}{shell} · host {}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::host::facts().line()
    )
}

pub fn hook(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    host: Arc<SubagentHost>,
) -> Arc<EnvironmentFn> {
    let cwd = wiring.cwd.clone();
    let broker = wiring.broker.clone();
    let settings = session.settings_handle();
    let usage = session.usage_handle();
    let cost = session.cost_handle();
    let context = session.compact_status_handle();
    let lane = session.lane_handle();
    let todos = session.todos_handle();
    let deadline = session.deadline();
    let kernel = session.kernel_state_handle();
    let clock = Mutex::new(None);
    let summary = GitSummary::new(cwd.clone(), session.subscribe());
    Arc::new(move || {
        let mut lines = Vec::new();
        let git = {
            let _span = yi_types::trace::span("env.git_summary");
            summary.read()
        }
        .map(|(branch, modified, untracked)| git_line(&branch, modified, untracked))
        .unwrap_or_default();
        lines.push(format!("cwd: {}{git}", cwd.display()));
        if let Some(files) = files_line(&cwd) {
            lines.push(files);
        }
        // The model commits; the person lands. It sees the landing, never a verb.
        if let Some(landing) = lane().map(|lane| lane.landing())
            && matches!(
                landing,
                yi_types::lane::Landing::Open { .. } | yi_types::lane::Landing::Merged { .. }
            )
        {
            lines.push(format!("landing: {}", crate::slash::landing_line(&landing)));
        }
        if let Some(list) = todos().map(|store| store.list())
            && list.progress().total > 0
        {
            lines.push(format!(
                "todos: {}",
                crate::todo::text::header(&list).trim_start_matches("Todos ")
            ));
        }
        if let Some(time) = time_per_minute(&clock, yi_session::now_ms(), || local_time(&cwd)) {
            lines.push(format!("time: {time}"));
        }
        if let Some(deadline) = deadline {
            lines.push(deadline_line(deadline.total, deadline.started.elapsed()));
        }
        let shell = std::env::var("SHELL")
            .ok()
            .and_then(|shell| {
                Path::new(&shell)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .map(|name| format!(" · shell {name}"))
            .unwrap_or_default();
        lines.push(platform_line(&shell));
        let (model, effort) = settings();
        let mode = broker
            .as_ref()
            .map(|broker| format!(" · permission {}", crate::gate::mode_label(broker.mode())))
            .unwrap_or_default();
        let effort = format!("{effort:?}").to_lowercase();
        lines.push(format!("model: {} · effort {effort}{mode}", model.id));
        let mut budget = context
            .as_ref()
            .map(|status| {
                let _span = yi_types::trace::span("env.compact_status");
                status()
            })
            .map(|s| format!("{} of {} used", tokens(s.tokens), tokens(s.context_window)));
        if let Some(last) = usage() {
            let read = last
                .input
                .saturating_add(last.cache_read)
                .saturating_add(last.cache_write);
            let turn = format!(
                "last turn {} in / {} out",
                tokens(u64::try_from(read).unwrap_or(0)),
                tokens(u64::try_from(last.output).unwrap_or(0))
            );
            budget = Some(budget.map_or(turn.clone(), |b| format!("{b} · {turn}")));
        }
        if let Some(mut line) = budget {
            if let Some((cost, lower_bound)) = cost().filter(|(cost, _)| *cost > 0.0) {
                let cost = yi_types::message::fmt_cost(cost, lower_bound);
                line.push_str(&format!(" · session {cost}"));
            }
            lines.push(format!("context: {line}"));
        }
        if let Some(state) = kernel() {
            lines.push(format!("kernel: {state}"));
        }
        if let Some(line) = crate::family::children_line(&host.states()) {
            lines.push(line);
        }
        Some(render(&lines))
    })
}
