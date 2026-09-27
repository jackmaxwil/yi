use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use yi_types::message::{AgentMessage, ENVIRONMENT_TAG, UserContent};

use crate::session::{AgentSession, EnvironmentFn};
use crate::subagent::SubagentHost;
use crate::wiring::RuntimeWiring;

const PROBE: Duration = Duration::from_secs(5);

/// The branch from the one HEAD reader, and the (modified, untracked) counts from `git status`.
pub fn git_summary(cwd: &Path) -> Option<(String, usize, usize)> {
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

pub fn tracked(root: &Path, paths: &[String]) -> Vec<bool> {
    let Ok(root) = std::fs::canonicalize(root) else {
        return vec![false; paths.len()];
    };
    let rels: Vec<Option<String>> = paths
        .iter()
        .map(|path| {
            let canonical = std::fs::canonicalize(path).ok()?;
            let rel = canonical.strip_prefix(&root).ok()?;
            Some(rel.to_string_lossy().into_owned())
        })
        .collect();
    let mut args = vec!["ls-files", "-z", "--"];
    args.extend(rels.iter().flatten().map(String::as_str));
    if args.len() == 3 {
        return vec![false; paths.len()];
    }
    let listed: std::collections::HashSet<String> =
        crate::lane::capture(&root, "git", &args, PROBE)
            .map(|out| {
                out.split('\0')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
    rels.iter()
        .map(|rel| rel.as_deref().is_some_and(|rel| listed.contains(rel)))
        .collect()
}

pub const FILES_SHOWN: usize = 20;

/// The cwd's top level, sorted, hidden names skipped, capped: the oracle beside the spec.
pub fn files_line(cwd: &Path) -> Option<String> {
    let entries = std::fs::read_dir(cwd).ok()?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
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
        line.push_str(&format!(" …(+{})", total.saturating_sub(FILES_SHOWN)));
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

pub fn append(messages: &[AgentMessage], block: &str) -> Vec<AgentMessage> {
    let mut out = messages.to_vec();
    out.push(AgentMessage::host_user(
        UserContent::Text(block.to_owned()),
        0,
    ));
    out
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
    Arc::new(move || {
        let mut lines = Vec::new();
        let git = {
            let _span = yi_types::trace::span("env.git_summary");
            git_summary(&cwd)
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
            if let Some(cost) = cost().filter(|cost| *cost > 0.0) {
                line.push_str(&format!(" · session ${cost:.2}"));
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
