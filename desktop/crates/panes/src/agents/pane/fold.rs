//! The shown thread's child agents folded into the rows the list draws.

use yd_core::feed::{FeedEvent, Table};
use yd_core::ids::{Millis, RowId};
use yd_core::model::Model;
use yd_core::reduce::agents::{Agent, Doing, Flag};
use yd_core::reduce::approvals::Ask;
use yd_core::reduce::threads::{Run, Thread};
use yd_core::reduce::turns::{ToolCall, ToolState};
use yd_core::row::{Dot, Row, Rows, Tone, Value};

/// How far back the timeline column looks.
const WINDOW: u64 = 14 * 60 * 1000;
/// Characters of a command or an answer shown in a cell.
const SHORT: usize = 60;
/// A value the server does not send.
pub(super) const DASH: &str = "–";

/// Tokens as the record writes them: `842`, `5.0K`, `41.2K`, `1.2M`.
pub(super) fn count(tokens: u64) -> String {
    let n = tokens as f64;
    if tokens >= 1_000_000 {
        format!("{:.1}M", n / 1e6)
    } else if tokens >= 1_000 {
        format!("{:.1}K", n / 1e3)
    } else {
        tokens.to_string()
    }
}

/// Run time as the record writes it: `0:40` and `1:10` under two minutes, then `6m`, `1h05m`.
fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..=119 => format!("{}:{:02}", secs / 60, secs % 60),
        120..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
    }
}

fn dollars(cost: f64) -> String {
    format!("${cost:.2}")
}

fn short(words: &str) -> String {
    words.chars().take(SHORT).collect()
}

fn first_line(words: &str) -> &str {
    words
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

/// The calls of `agent` the waiting asks gate: the first with a command to show.
fn asked<'a>(agent: &'a Agent, waiting: &[&Ask]) -> Option<&'a ToolCall> {
    waiting
        .iter()
        .filter_map(|ask| ask.tool.as_ref())
        .find_map(|call| agent.transcript.tools.get(call))
}

fn wants(call: &ToolCall) -> String {
    match call
        .args
        .get("command")
        .and_then(|command| command.as_str())
    {
        Some(command) => format!("wants to run {}", short(first_line(command))),
        None => format!("wants to use {}", call.name),
    }
}

fn running(agent: &Agent) -> Option<&ToolCall> {
    let live = agent
        .transcript
        .tools
        .values()
        .filter(|call| call.state == ToolState::Running);
    live.max_by_key(|call| call.started)
}

fn doing(agent: &Agent, asking: Option<&ToolCall>) -> String {
    let said = &agent.update;
    if let Some(call) = asking {
        return wants(call);
    }
    if said.exit.is_some() || said.status.as_str() != "running" {
        let line = |words: &Option<String>| words.as_deref().map(first_line).map(short);
        return match said.status.as_str() {
            "completed" => line(&said.answer_preview).unwrap_or_else(|| "done".to_owned()),
            _ => line(&said.error).unwrap_or_else(|| "failed".to_owned()),
        };
    }
    if agent.flagged.is_some() {
        return match agent.flag() {
            Some(Flag::Stuck) => "stuck".to_owned(),
            Some(Flag::NeedsYou | Flag::Other) | None => "waiting on you".to_owned(),
        };
    }
    if let Some(call) = running(agent) {
        let command = call
            .args
            .get("command")
            .and_then(|command| command.as_str());
        let path = call.args.get("path").and_then(|path| path.as_str());
        let target = command.or(path).map(first_line).map(short);
        return match target {
            Some(target) => format!("{} {target}", call.name),
            None => call.name.clone(),
        };
    }
    match agent.doing() {
        Doing::Waiting => "waiting",
        Doing::Writing => "writing",
        Doing::Executing => "running a tool",
    }
    .to_owned()
}

fn dot(agent: &Agent, asking: bool) -> Dot {
    if asking || (agent.flagged.is_some() && agent.update.exit.is_none()) {
        return Dot::Need;
    }
    match agent.update.status.as_str() {
        "running" => Dot::Work,
        "completed" => Dot::Done,
        "error" => Dot::Fail,
        _ => Dot::Idle,
    }
}

/// Files an agent read or wrote, as the record's tags: `tests/upload.rs +41`, `read upload.rs`.
fn touched(agent: &Agent) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for call in agent.transcript.tools.values() {
        let Some(path) = call.args.get("path").and_then(|path| path.as_str()) else {
            continue;
        };
        let tag = match call.name.as_str() {
            "read" => {
                let name = path.rsplit('/').next().unwrap_or(path);
                format!("read {name}")
            }
            "edit" | "write" => {
                let added = call.details.get("added").and_then(|added| added.as_u64());
                match added {
                    Some(added) => format!("{path} +{added}"),
                    None => path.to_owned(),
                }
            }
            _ => continue,
        };
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// The agent's bar in the last 14 minutes, from when this app first and last heard of it.
fn lane(first: Millis, last: Millis, live: bool, now: Millis, tone: Tone) -> Option<Value> {
    if now.0 == 0 {
        return None;
    }
    let from = now.0.saturating_sub(WINDOW);
    let end = if live { now.0 } else { last.0.min(now.0) };
    let start = first.0.max(from).min(end);
    let share = |span: u64| span as f32 / WINDOW as f32;
    Some(Value::Bar {
        start: share(start.saturating_sub(from)),
        len: share(end.saturating_sub(start)),
        tone,
    })
}

fn buttons(names: &[&str]) -> Value {
    Value::Tags(names.iter().map(|name| (*name).to_owned()).collect())
}

fn child(agent: &Agent, thread: &Thread, root: &RowId, now: Millis, waiting: &[&Ask]) -> Row {
    let said = &agent.update;
    let asking = asked(agent, waiting);
    let state = dot(agent, asking.is_some());
    let live = said.status.as_str() == "running";
    let tone = match state {
        Dot::Need => Tone::Need,
        Dot::Work | Dot::Run => Tone::Accent,
        Dot::Ask | Dot::Done | Dot::Idle | Dot::Stop | Dot::Fail | Dot::Pause => Tone::Dim,
    };
    let parent = agent
        .parent
        .as_ref()
        .filter(|parent| **parent != agent.id)
        .filter(|parent| thread.agents.iter().any(|other| other.id == **parent))
        .map(|parent| RowId::new(parent.as_str()));
    let kids = thread
        .agents
        .iter()
        .filter(|other| other.parent.as_ref() == Some(&agent.id))
        .count();
    let tokens = count(said.token_count);
    let budget = match kids {
        0 => format!("{tokens} of {DASH} tokens"),
        1 => format!("{tokens} of {DASH} tokens · 1 child"),
        kids => format!("{tokens} of {DASH} tokens · {kids} children"),
    };
    let cost = said
        .cost
        .as_ref()
        .and_then(|cost| cost.as_f64())
        .map_or_else(|| DASH.to_owned(), dollars);
    let last = said
        .answer_preview
        .as_deref()
        .map(first_line)
        .map(str::to_owned)
        .or_else(|| agent.transcript.last_words());
    let mut names = Vec::new();
    if asking.is_some() {
        names.push("Run its command ↵");
    }
    if said.session.is_some() {
        names.push("Open thread");
    }
    if live {
        names.extend(["Pause", "More budget", "Stop"]);
    }
    let mut row = Row::new(agent.id.as_str())
        .text("agent", &said.name)
        .cell("state", Value::Dot(state))
        .text("doing_now", doing(agent, asking))
        .text("model", DASH)
        .text("tokens", tokens)
        .text("cost", cost)
        .text(
            "time",
            said.elapsed_ms.map_or_else(|| DASH.to_owned(), clock),
        )
        .text("asked_to", said.task.as_deref().unwrap_or(DASH))
        .text(
            "said_last",
            last.map_or_else(|| DASH.to_owned(), |last| format!("\"{last}\"")),
        )
        .cell("touched", Value::Tags(touched(agent)))
        .text("budget", budget)
        .cell("actions", buttons(&names));
    if let Some(bar) = lane(agent.first_seen, agent.last_seen, live, now, tone) {
        row = row.cell("last_14_min", bar);
    }
    row.parent = Some(parent.unwrap_or_else(|| root.clone()));
    row
}

/// The model's letter as its option names it: `Opus 5.5` is `O`.
fn model_letter(thread: &Thread) -> Option<String> {
    let option = thread.option("model")?;
    let choice = option
        .options
        .iter()
        .find(|choice| choice.value == option.current_value)?;
    let letter = choice.name.chars().find(|c| c.is_alphabetic())?;
    Some(letter.to_uppercase().collect())
}

fn root(thread: &Thread, root: &RowId, working: bool, waiting: &[&Ask]) -> Row {
    let state = match &thread.state {
        Run::NeedsYou if thread.lost() => Dot::Idle,
        Run::NeedsYou => Dot::Need,
        Run::Running => Dot::Work,
        Run::Idle { .. } if thread.failed() => Dot::Fail,
        Run::Idle { .. } => Dot::Done,
        Run::Other(_) => Dot::Idle,
    };
    let asking = waiting
        .iter()
        .filter_map(|ask| ask.tool.as_ref())
        .find_map(|call| thread.transcript.tools.get(call));
    let doing = match (asking, &thread.state, working) {
        (Some(call), _, _) => wants(call),
        (None, Run::NeedsYou, _) if thread.lost() => "not answered".to_owned(),
        (None, Run::NeedsYou, _) => "waiting on you".to_owned(),
        (None, Run::Running, true) => "waiting on a child".to_owned(),
        (None, Run::Running, false) => "working".to_owned(),
        (None, Run::Idle { .. }, _) if thread.failed() => "failed".to_owned(),
        (None, Run::Idle { .. }, _) => "idle".to_owned(),
        (None, Run::Other(_), _) => DASH.to_owned(),
    };
    let title = thread.title.as_deref().unwrap_or(thread.id.as_str());
    let tokens = count(thread.usage.used.0);
    let last = thread.transcript.last_words();
    let model = match model_letter(thread) {
        Some(letter) => Value::Badge(letter, Tone::Model),
        None => Value::Text(DASH.to_owned()),
    };
    let kids = thread.agents.len();
    let names: &[&str] = if matches!(thread.state, Run::Idle { .. }) {
        &[]
    } else {
        &["Pause", "More budget", "Stop"]
    };
    Row::new(root.as_str())
        .text("agent", title)
        .cell("state", Value::Dot(state))
        .text("doing_now", doing)
        .cell("model", model)
        .text("tokens", tokens.as_str())
        .text("cost", dollars(thread.cost()))
        .text("time", DASH)
        .text(
            "said_last",
            last.map_or_else(|| DASH.to_owned(), |last| format!("\"{last}\"")),
        )
        .text(
            "budget",
            format!("{tokens} of {DASH} tokens · {kids} agents"),
        )
        .cell("actions", buttons(names))
}

/// The rows of the thread's agents, its own row first. A thread that never ran a turn has
/// none; one that did has its own agent's row, with any children under it.
pub(super) fn rows(model: &Model, thread: &Thread) -> Vec<Row> {
    if thread.agents.is_empty() && thread.transcript.turns().is_empty() {
        return Vec::new();
    }
    let waiting: Vec<&Ask> = model
        .wire
        .waiting()
        .filter(|ask| ask.thread == thread.id)
        .collect();
    let top = RowId::new(format!("thread:{}", thread.id));
    let working = thread
        .agents
        .iter()
        .any(|agent| agent.update.status.as_str() == "running");
    let own = root(thread, &top, working, &waiting);
    let kids = thread
        .agents
        .iter()
        .map(|agent| child(agent, thread, &top, model.now, &waiting));
    std::iter::once(own).chain(kids).collect()
}

/// A table of `rows`, as a feed would have it after one reset.
pub(super) fn table(rows: Vec<Row>) -> Table {
    let mut table = Table::default();
    table.apply(FeedEvent::Reset(rows));
    table
}

/// What the header, the footer and the tab say of a table of agents.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Summary {
    pub rows: usize,
    pub depth: usize,
    pub tokens: u64,
    pub cost: f64,
    pub working: usize,
    pub need: usize,
    pub done: usize,
    pub failed: usize,
    /// Agents that hold a slot: not finished.
    pub slots: usize,
}

fn tokens_of(words: &str) -> u64 {
    let (number, scale) = match words.chars().last() {
        Some('K') => (words.trim_end_matches('K'), 1e3),
        Some('M') => (words.trim_end_matches('M'), 1e6),
        _ => (words, 1.0),
    };
    let n: f64 = number.parse().unwrap_or(0.0);
    (n * scale).round() as u64
}

fn depth_of(table: &Table, row: &Row) -> usize {
    let mut seen = vec![&row.id];
    let mut at = row;
    while let Some(parent) = at.parent.as_ref().and_then(|id| table.find(id)) {
        if seen.contains(&&parent.id) {
            break;
        }
        seen.push(&parent.id);
        at = parent;
    }
    seen.len().saturating_sub(1)
}

pub(super) fn summarise(table: &Table) -> Summary {
    let mut sum = Summary::default();
    for at in 0..table.len() {
        let Some(row) = table.get(at) else { continue };
        sum.rows = sum.rows.saturating_add(1);
        sum.depth = sum.depth.max(depth_of(table, row));
        sum.tokens = sum
            .tokens
            .saturating_add(tokens_of(row.words("tokens").unwrap_or("")));
        let cost = row.words("cost").unwrap_or("").trim_start_matches('$');
        sum.cost += cost.parse::<f64>().unwrap_or(0.0);
        let state = match row.get("state") {
            Some(Value::Dot(state)) => Some(*state),
            _ => None,
        };
        let slot = match state {
            Some(Dot::Work | Dot::Run) => {
                sum.working = sum.working.saturating_add(1);
                true
            }
            Some(Dot::Need | Dot::Ask) => {
                sum.need = sum.need.saturating_add(1);
                true
            }
            Some(Dot::Done) => {
                sum.done = sum.done.saturating_add(1);
                false
            }
            Some(Dot::Fail) => {
                sum.failed = sum.failed.saturating_add(1);
                false
            }
            Some(Dot::Idle | Dot::Stop | Dot::Pause) | None => false,
        };
        if slot {
            sum.slots = sum.slots.saturating_add(1);
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_times_are_written_as_the_record_writes_them() {
        assert_eq!(
            [0, 842, 5_000, 41_200, 1_250_000].map(count),
            ["0", "842", "5.0K", "41.2K", "1.2M"]
        );
        assert_eq!(
            [40_000, 70_000, 360_000, 840_000, 3_900_000].map(clock),
            ["0:40", "1:10", "6m", "14m", "1h05m"]
        );
    }

    #[test]
    fn a_long_non_ascii_command_is_cut_on_a_character() {
        let said = short(&"é".repeat(100));
        assert_eq!(said.chars().count(), SHORT);
    }

    #[test]
    fn a_figure_read_back_from_its_cell_sums_to_the_headline() {
        assert_eq!(tokens_of("41.2K"), 41_200);
        assert_eq!(tokens_of("842"), 842);
        assert_eq!(tokens_of("–"), 0);
    }
}
