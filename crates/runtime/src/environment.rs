use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use yi_types::message::{AgentMessage, ENVIRONMENT_TAG, UserContent};
use yi_types::subagent::ChildStatus;

use crate::session::{AgentSession, EnvironmentFn};
use crate::subagent::SubagentHost;
use crate::wiring::RuntimeWiring;

const PROBE: Duration = Duration::from_secs(5);

pub fn git_summary(cwd: &Path) -> Option<(String, usize)> {
    let args = ["status", "--porcelain", "--branch"];
    let out = crate::lane::capture(cwd, "git", &args, PROBE).ok()?;
    let mut lines = out.lines();
    let head = lines.next()?.strip_prefix("## ")?;
    let branch = match head {
        head if head.contains("(no branch)") => "detached",
        head => head
            .strip_prefix("No commits yet on ")
            .unwrap_or(head)
            .split("...")
            .next()
            .unwrap_or(head),
    };
    Some((sanitize(branch), lines.count()))
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

pub fn hook(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    host: Arc<SubagentHost>,
) -> Arc<EnvironmentFn> {
    let cwd = wiring.cwd.clone();
    let broker = wiring.broker.clone();
    let settings = session.settings_handle();
    let usage = session.usage_handle();
    let history = session.history_handle();
    let context = session.compact_status_handle();
    let lane = session.lane_handle();
    let todos = session.todos_handle();
    Arc::new(move || {
        let mut lines = Vec::new();
        let git = git_summary(&cwd)
            .map(|(branch, dirty)| format!(" (git: {branch}, {dirty} modified)"))
            .unwrap_or_default();
        lines.push(format!("cwd: {}{git}", cwd.display()));
        if let Some(line) = lane().and_then(|lane| lane.describe()) {
            lines.push(line);
        }
        if let Some(list) = todos().map(|store| store.list())
            && list.progress().total > 0
        {
            lines.push(format!(
                "todos: {}",
                crate::todo::text::header(&list).trim_start_matches("Todos ")
            ));
        }
        if let Some(time) = local_time(&cwd) {
            lines.push(format!("time: {time}"));
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
        lines.push(format!(
            "platform: {} {}{shell}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        let (model, effort) = settings();
        let mode = broker
            .as_ref()
            .map(|broker| format!(" · permission {}", crate::gate::mode_label(broker.mode())))
            .unwrap_or_default();
        let effort = format!("{effort:?}").to_lowercase();
        lines.push(format!("model: {} · effort {effort}{mode}", model.id));
        let mut budget = context
            .as_ref()
            .map(|status| status())
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
        let cost: f64 = history()
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Assistant { usage, .. } => usage.cost.total.as_f64(),
                _ => None,
            })
            .sum();
        if let Some(mut line) = budget {
            if cost > 0.0 {
                line.push_str(&format!(" · session ${cost:.2}"));
            }
            lines.push(format!("context: {line}"));
        }
        let running: Vec<String> = host
            .children
            .lock()
            .map(|children| {
                children
                    .values()
                    .filter(|child| child.status == ChildStatus::Running)
                    .map(|child| sanitize(&child.session_name))
                    .collect()
            })
            .unwrap_or_default();
        if !running.is_empty() {
            lines.push(format!(
                "children: {} running ({})",
                running.len(),
                running.join(", ")
            ));
        }
        Some(render(&lines))
    })
}
