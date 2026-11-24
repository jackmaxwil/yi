//! Trace: one turn as a waterfall of steps, timed client-side from the events as they arrive.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use serde_json::Value;
use yi_types::message::{AgentMessage, Content};
use yi_types::subagent::{ChildFlag, ChildStatus};

use crate::ids::{Millis, RowId};
use crate::reduce::agents::Agent;
use crate::reduce::threads::{Run, Thread};
use crate::reduce::turns::{Body, ToolCall, Transcript};
use crate::row::{Row, Rows, Tone, Value as Cell};

/// What a step is; it picks the bar's colour and what the opened step shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Turn,
    Model,
    Tool,
    Child,
    Wait,
}

impl Sort {
    fn tone(self) -> Tone {
        match self {
            Sort::Turn => Tone::Dim,
            Sort::Model => Tone::Model,
            Sort::Tool => Tone::Ok,
            Sort::Child => Tone::Accent,
            Sort::Wait => Tone::Need,
        }
    }
}

/// One bar of the waterfall. `facts` are the opened step's label fields, in the order shown.
#[derive(Debug, Clone, PartialEq)]
struct Step {
    id: String,
    parent: Option<String>,
    name: String,
    sort: Sort,
    /// Offset from the turn's start.
    start: Millis,
    took: Millis,
    /// Still counting: the step has not ended.
    open: bool,
    /// Dollars and tokens this step adds to the turn's total.
    cost: f64,
    tokens: u64,
    facts: Vec<(&'static str, String)>,
    actions: &'static [&'static str],
}

/// What a step is before it is placed: where it hangs, when it began and ended.
struct Draft {
    id: String,
    parent: String,
    name: String,
    sort: Sort,
    from: Millis,
    to: Option<Millis>,
    facts: Vec<(&'static str, String)>,
    actions: &'static [&'static str],
}

/// One turn's steps, in the order the waterfall draws them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Trace {
    number: u32,
    asked: String,
    live: bool,
    steps: Vec<Step>,
}

/// Characters of the prompt kept in the turn's name.
const ASKED: usize = 60;
/// The whole seconds a third of the lane may be, smallest first.
const SPANS: [u64; 9] = [1, 2, 5, 10, 20, 30, 60, 120, 300];

fn tokens(count: i64) -> u64 {
    u64::try_from(count).unwrap_or(0)
}

/// A short count: `212`, `3.1K`.
fn short(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    format!("{:.1}K", count as f64 / 1_000.0)
}

fn money(dollars: f64) -> String {
    format!("${dollars:.2}")
}

fn length(took: Millis) -> String {
    match took.0 {
        0..1_000 => format!("{} ms", took.0),
        ms => format!("{:.1} s", ms as f64 / 1_000.0),
    }
}

/// How long an ask has waited, as a clock: `0:42`.
fn waited(took: Millis) -> String {
    let secs = took.0 / 1_000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The time of day on the injected clock, in UTC: `17:58:03`.
fn clock(at: Millis) -> String {
    let secs = at.0 / 1_000 % 86_400;
    format!("{:02}:{:02}:{:02}", secs / 3_600, secs / 60 % 60, secs % 60)
}

fn starts(start: Millis) -> String {
    match start.0 {
        0 => "0".to_owned(),
        ms => format!("+{:.1}", ms as f64 / 1_000.0),
    }
}

/// The lane: three equal steps of a whole number of seconds that hold `total`.
fn span(total: Millis) -> Millis {
    let seconds = total.0.div_ceil(1_000).max(1);
    let step = SPANS
        .iter()
        .copied()
        .find(|step| step.saturating_mul(3) >= seconds);
    Millis(step.map_or(seconds, |step| step * 3).saturating_mul(1_000))
}

/// The most telling argument of a tool call: a path, a pattern, a command.
fn target(args: &Value) -> Option<&str> {
    let keys = [
        "path",
        "file_path",
        "file",
        "pattern",
        "query",
        "command",
        "url",
    ];
    keys.iter().find_map(|key| args.get(key)?.as_str())
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_edit(name: &str) -> bool {
    matches!(name, "edit" | "write" | "apply_patch" | "multi_edit")
}

fn actions(call: &str) -> &'static [&'static str] {
    match call {
        "read" => &["Open the file", "Run it again", "Attach"],
        "grep" | "search" | "find" | "glob" => &["Show the matches", "Run it again", "Attach"],
        name if is_edit(name) => &["Open the change", "Put it back", "Attach"],
        _ => &["Show its logs", "Run it again", "Attach"],
    }
}

/// The first line of what a tool printed, or its line count for a read.
fn got(call: &ToolCall) -> Option<String> {
    let said = call.output.iter().find_map(|block| match block {
        Content::Text { text, .. } => Some(text.as_str()),
        _ => None,
    })?;
    if call.name == "read" {
        return Some(format!("{} lines", said.lines().count()));
    }
    said.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.chars().take(ASKED).collect())
}

/// Builds the steps of one turn: every offset is from the turn's start.
struct Build {
    origin: Millis,
    now: Millis,
    steps: Vec<Step>,
    edited: BTreeSet<String>,
}

impl Build {
    fn push(&mut self, draft: Draft, cost: f64, tokens: u64) {
        let from = draft.from;
        let end = draft.to.unwrap_or(self.now).max(from);
        self.steps.push(Step {
            id: draft.id,
            parent: Some(draft.parent),
            name: draft.name,
            sort: draft.sort,
            start: Millis(from.0.saturating_sub(self.origin.0)),
            took: Millis(end.0.saturating_sub(from.0)),
            open: draft.to.is_none(),
            cost,
            tokens,
            facts: draft.facts,
            actions: draft.actions,
        });
    }

    /// The model calls and tool calls `messages` of one agent's transcript hold, under `owner`.
    fn transcript(&mut self, owner: &str, t: &Transcript, messages: Range<usize>) {
        let kept = t.messages.get(messages).unwrap_or_default();
        for (index, message) in kept.iter().enumerate() {
            let Body::Message(body) = &message.body else {
                continue;
            };
            let AgentMessage::Assistant {
                content,
                model,
                usage,
                ..
            } = body.as_ref()
            else {
                continue;
            };
            let calls: Vec<&ToolCall> = content
                .iter()
                .filter_map(|block| match block {
                    Content::ToolCall { id, .. } => t.tools.get(id.as_str()),
                    _ => None,
                })
                .collect();
            let (read, wrote) = (
                tokens(usage.input).saturating_add(tokens(usage.cache_read)),
                tokens(usage.output),
            );
            let cost = usage.cost.total.as_f64().unwrap_or(0.0);
            let took = message
                .done
                .map_or(0, |done| done.0.saturating_sub(message.at.0));
            let mut facts = vec![
                ("model", model.clone()),
                ("read", format!("{} tokens", short(read))),
                ("wrote", format!("{} tokens", short(wrote))),
            ];
            if took > 0 && wrote > 0 {
                let speed = wrote.saturating_mul(1_000) / took;
                facts.push(("speed", format!("{speed} tokens a second")));
            }
            facts.push(("cost", money(cost)));
            if !calls.is_empty() {
                let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
                facts.push(("decided", names.join(", ")));
            }
            let draft = Draft {
                id: format!("model-{owner}-{index}"),
                parent: owner.to_owned(),
                name: "model".to_owned(),
                sort: Sort::Model,
                from: message.at,
                to: message.done,
                facts,
                actions: &["Show what it wrote", "Show its logs", "Attach"],
            };
            self.push(draft, cost, read.saturating_add(wrote));
            for call in calls {
                self.call(owner, call);
            }
        }
    }

    /// A tool call as a step, and the ask it waited on as another.
    fn call(&mut self, owner: &str, call: &ToolCall) {
        let said = target(&call.args).unwrap_or_default();
        let name = format!("{} {}", call.name, leaf(said))
            .trim_end()
            .to_owned();
        let id = call.id.as_str().replace('/', ":");
        if is_edit(&call.name) && !said.is_empty() {
            self.edited.insert(said.to_owned());
        }
        // One command is one span: an answered ask is a fact of the tool's step, and only an ask
        // still waiting stands alone, because the tool has no step yet.
        let asked = call
            .gate
            .filter(|_| call.started.is_none() || call.gate.is_some_and(|g| g.answered.is_none()));
        if let Some(gate) = asked {
            let mut facts = vec![("command", said.to_owned()), ("asked", clock(gate.asked))];
            let waiting = self.now.0.saturating_sub(gate.asked.0);
            facts.push(("waiting", waited(Millis(waiting))));
            let draft = Draft {
                id: format!("wait-{id}"),
                parent: owner.to_owned(),
                name: format!("{name} · waiting on you"),
                sort: Sort::Wait,
                from: gate.asked,
                to: None,
                facts,
                actions: &["Run it ↵", "Edit", "Deny", "Attach"],
            };
            self.push(draft, 0.0, 0);
        }
        let Some(started) = call.started else {
            return;
        };
        let mut facts = vec![("tool", call.name.clone())];
        if let Some(gate) = call.gate.filter(|g| g.answered.is_some()) {
            facts.push(("asked", clock(gate.asked)));
            let answered = gate
                .answered
                .map_or(0, |at| at.0.saturating_sub(gate.asked.0));
            facts.push(("waited", waited(Millis(answered))));
        }
        if !said.is_empty() {
            let field = match call.name.as_str() {
                "grep" | "search" | "find" | "glob" => "looked_for",
                "bash" => "command",
                _ => "file",
            };
            facts.push((field, said.to_owned()));
        }
        facts.extend(got(call).map(|said| ("got", said)));
        // The server's own duration, where the result gives one, beats the two stamps.
        let to = call.ended.map(|_| {
            let took = call.duration().map_or(0, |took| took.0);
            Millis(started.0.saturating_add(took))
        });
        let draft = Draft {
            id: format!("tool-{id}"),
            parent: owner.to_owned(),
            name,
            sort: Sort::Tool,
            from: started,
            to,
            facts,
            actions: actions(&call.name),
        };
        self.push(draft, 0.0, 0);
    }

    /// A child agent and, under it, what it did.
    fn child(&mut self, root: &str, agent: &Agent) {
        let update = &agent.update;
        let named = |id: &str| format!("child-{}", id.replace('/', ":"));
        let id = named(agent.id.as_str());
        let done = update.exit.is_some() || update.status != ChildStatus::Running;
        let to = done.then(|| match update.elapsed_ms {
            Some(took) => Millis(agent.first_seen.0.saturating_add(took)),
            None => agent.last_seen,
        });
        let state = match &update.flag {
            Some(ChildFlag::NeedsYou { .. }) => "waiting on you",
            Some(ChildFlag::Stuck { .. } | ChildFlag::Other) => "stuck",
            None => update.status.as_str(),
        };
        let cost = update.cost.as_ref().and_then(|cost| cost.as_f64());
        let mut facts = vec![
            ("agent", update.name.clone()),
            ("steps", update.tool_use_count.to_string()),
        ];
        facts.extend(cost.map(|cost| ("cost", money(cost))));
        facts.push(("state", state.to_owned()));
        let draft = Draft {
            id: id.clone(),
            parent: agent
                .parent
                .as_ref()
                .map_or_else(|| root.to_owned(), |parent| named(parent.as_str())),
            name: format!("child · {}", update.name),
            sort: Sort::Child,
            from: agent.first_seen,
            to,
            facts,
            actions: &["Open its thread", "Show its trace", "Attach"],
        };
        let before = self.steps.len();
        self.push(draft, 0.0, 0);
        let kept = agent.transcript.messages.len();
        self.transcript(&id, &agent.transcript, 0..kept);
        // Its own replies are counted; the server's figure stands in only when it sent none.
        let seen = self.steps.get(before..).unwrap_or_default();
        if !seen.iter().any(|step| step.sort == Sort::Model)
            && let Some(step) = self.steps.get_mut(before)
        {
            step.cost = cost.unwrap_or(0.0);
            step.tokens = update.token_count;
        }
    }
}

impl Trace {
    /// The steps of turn `number` of the thread, or of its last turn. `now` is the injected
    /// clock: a step that has not ended runs to it. None when the thread has no such turn.
    pub fn of(thread: &Thread, number: Option<u32>, now: Millis) -> Option<Self> {
        let turns = thread.transcript.turns();
        let at = match number {
            Some(number) => turns.iter().position(|turn| turn.number == number)?,
            None => turns.len().checked_sub(1)?,
        };
        let turn = turns.get(at)?;
        let next = turns.get(at.saturating_add(1)).map(|next| next.started);
        let live = next.is_none() && matches!(thread.state, Run::Running | Run::NeedsYou);
        let root = format!("turn-{}", turn.number);
        let mut build = Build {
            origin: turn.started,
            now,
            steps: Vec::new(),
            edited: BTreeSet::new(),
        };
        let first = turn.first;
        let own = first..first.saturating_add(turn.len);
        build.transcript(&root, &thread.transcript, own);
        let mine = |agent: &&Agent| {
            agent.first_seen >= turn.started && next.is_none_or(|next| agent.first_seen < next)
        };
        for agent in thread.agents.iter().filter(mine) {
            build.child(&root, agent);
        }
        let asked = thread
            .transcript
            .messages
            .get(first)
            .and_then(|message| match &message.body {
                Body::Message(body) => match body.as_ref() {
                    AgentMessage::User { content, .. } => Some(user_text(content)),
                    _ => None,
                },
                Body::Raw(_) => None,
            })
            .unwrap_or_default();
        let ends = build
            .steps
            .iter()
            .map(|step| step.start.0.saturating_add(step.took.0));
        let finished = turn
            .ended
            .map(|ended| ended.0.saturating_sub(turn.started.0));
        let running = live.then(|| now.0.saturating_sub(turn.started.0));
        let total = ends.chain(finished).chain(running).max().unwrap_or(0);
        let mut trace = Self {
            number: turn.number,
            asked,
            live,
            steps: build.steps,
        };
        trace
            .steps
            .insert(0, trace.turn_step(root, Millis(total), build.edited.len()));
        Some(trace)
    }

    /// The turn's own step: what was asked, and what the turn took and spent in all.
    fn turn_step(&self, id: String, took: Millis, edited: usize) -> Step {
        let cost: f64 = self.steps.iter().map(|step| step.cost).sum();
        let spent: u64 = self.steps.iter().map(|step| step.tokens).sum();
        let agents = self
            .steps
            .iter()
            .filter(|step| step.sort == Sort::Child)
            .count();
        let mut facts = vec![
            ("asked", self.asked.clone()),
            ("steps", self.steps.len().to_string()),
            ("agents", agents.to_string()),
            ("cost", money(cost)),
            ("tokens", short(spent)),
        ];
        if edited > 0 {
            let plural = if edited == 1 { "" } else { "s" };
            facts.push(("changed", format!("{edited} file{plural}")));
        }
        let asked: String = self.asked.chars().take(ASKED).collect();
        Step {
            id,
            parent: None,
            name: format!("Turn {} · {asked}", self.number),
            sort: Sort::Turn,
            start: Millis(0),
            took,
            open: self.live,
            cost,
            tokens: spent,
            facts,
            actions: &["Show its logs", "Open the changes", "Attach"],
        }
    }

    /// The rows the list draws, depth first: each step's bar is a share of the lane.
    pub fn rows(&self) -> Vec<Row> {
        let Some(total) = self.steps.first().map(|root| root.took) else {
            return Vec::new();
        };
        let lane = span(total).0 as f32;
        let mut under: BTreeMap<Option<&str>, Vec<&Step>> = BTreeMap::new();
        for step in &self.steps {
            under.entry(step.parent.as_deref()).or_default().push(step);
        }
        for kids in under.values_mut() {
            kids.sort_by_key(|step| step.start);
        }
        let mut rows = Vec::with_capacity(self.steps.len());
        let mut stack: Vec<&Step> = under
            .get(&None)
            .map(|roots| roots.iter().rev().copied().collect())
            .unwrap_or_default();
        while let Some(step) = stack.pop() {
            rows.push(self.row(step, lane));
            if let Some(kids) = under.get(&Some(step.id.as_str())) {
                stack.extend(kids.iter().rev());
            }
        }
        rows
    }

    fn row(&self, step: &Step, lane: f32) -> Row {
        let start = step.start.0 as f32 / lane;
        let len = (step.took.0 as f32 / lane).min(1.0 - start);
        let took = match (step.open, step.sort) {
            (true, Sort::Wait) => format!("{}…", waited(step.took)),
            (true, _) => format!("{}…", length(step.took)),
            (false, _) => length(step.took),
        };
        let bar = Cell::Bar {
            start,
            len,
            tone: step.sort.tone(),
        };
        let titles = step.actions.iter().map(|title| (*title).to_owned());
        let mut row = Row::new(step.id.as_str())
            .text("step", step.name.as_str())
            .text("start", starts(step.start))
            .text("took", took)
            .cell("timeline", bar)
            .cell("actions", Cell::Tags(titles.collect()));
        row.parent = step.parent.as_deref().map(RowId::new);
        for (field, said) in &step.facts {
            row = row.text(field, said.as_str());
        }
        if step.sort == Sort::Turn && self.live {
            row = row.cell("live", Cell::Flag(true));
        }
        row
    }
}

/// The words of a user message: its text, or the text blocks of its content.
fn user_text(content: &yi_types::message::UserContent) -> String {
    match content {
        yi_types::message::UserContent::Text(said) => said.clone(),
        yi_types::message::UserContent::Blocks(blocks) => yi_types::message::join_text(blocks, ""),
    }
}

/// What the pane's header bar and ruler say, read from the rows the list draws.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// `Turn 2`.
    pub title: String,
    /// `14.8 s so far · $0.31 · 2 in parallel`.
    pub summary: String,
    /// The tab's meta: `11 spans · 2 parallel`.
    pub meta: String,
    pub steps: u32,
    /// The ruler's labels, left to right.
    pub ticks: [String; 4],
}

impl Header {
    /// None for an empty list. The turn is the row with no parent.
    pub fn of(table: &impl Rows) -> Option<Self> {
        let rows: Vec<&Row> = (0..table.len()).filter_map(|at| table.get(at)).collect();
        let root = rows.iter().find(|row| row.parent.is_none())?;
        let title = root.words("step")?.split(" · ").next()?.to_owned();
        let total = root.words("took")?.trim_end_matches('…');
        let live = rows
            .iter()
            .any(|row| row.words("took").is_some_and(|took| took.ends_with('…')))
            || root.get("live").is_some();
        let parallel = parallel(&rows);
        let mut summary = format!("{total}{}", if live { " so far" } else { "" });
        summary.extend(root.words("cost").map(|cost| format!(" · {cost}")));
        let mut meta = format!("{} spans", rows.len());
        if parallel > 1 {
            summary.push_str(&format!(" · {parallel} in parallel"));
            meta.push_str(&format!(" · {parallel} parallel"));
        }
        let lane = match root.get("timeline") {
            Some(Cell::Bar { len, .. }) if *len > 0.0 => seconds(total)? / f64::from(*len),
            _ => 3.0,
        };
        let tick = |third: f64| {
            let at = lane * third / 3.0;
            if (at - at.round()).abs() < 0.05 {
                format!("{at:.0}")
            } else {
                format!("{at:.1}")
            }
        };
        Some(Self {
            title,
            summary,
            meta,
            steps: u32::try_from(rows.len()).unwrap_or(u32::MAX),
            ticks: ["0 s".to_owned(), tick(1.0), tick(2.0), tick(3.0)],
        })
    }
}

/// `14.8 s` or `250 ms` as seconds.
fn seconds(said: &str) -> Option<f64> {
    let said = said.trim_end_matches('…');
    match said.split_once(' ')? {
        (count, "s") => count.parse().ok(),
        (count, "ms") => Some(count.parse::<f64>().ok()? / 1_000.0),
        _ => None,
    }
}

/// The most child agents running at once.
fn parallel(rows: &[&Row]) -> usize {
    let bars: Vec<(f32, f32)> = rows
        .iter()
        .filter_map(|row| match row.get("timeline") {
            Some(Cell::Bar {
                start,
                len,
                tone: Tone::Accent,
            }) => Some((*start, start + len)),
            _ => None,
        })
        .collect();
    bars.iter()
        .map(|(at, _)| {
            bars.iter()
                .filter(|(from, to)| from <= at && at < to)
                .count()
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
