use std::sync::{Arc, Mutex};

use yi_types::message::{AgentMessage, Attribution, Content, StopReason, UserContent};
use yi_types::model::{ForcedTool, ToolChoice};
use yi_types::plan::doc::TodoStateName;
use yi_types::todo::PhaseName;
use yi_types::todo::{
    BlockedOn, TODO_INTERCEPT_ENTRY_TYPE, TodoInterceptRecord, TodoItem, TodoList,
};

use super::{DEFAULT_PHASE, Op, TodoStore, text, tool};
use crate::goal::StoreHandle;
use crate::plan::loop_coupling::gate::{eager_init, enumerated};
use crate::session::{AgentSession, InterceptStopFn, PromptChoiceFn, TurnCoupling, TurnObserveFn};

pub const NUDGE_CUSTOM_TYPE: &str = "todo_nudge";
pub const PRELUDE_CUSTOM_TYPE: &str = "todo_prelude";
pub const INTERCEPT_CUSTOM_TYPE: &str = "todo_intercept";
pub const SEED_ACTOR: &str = "prompt";

pub mod gate {
    pub const NUDGE_WORK: u32 = 12;
    pub const QUIET_TURNS: u32 = 3;
    pub const FIRST_LIST_WORK: u32 = 3;
    pub const NUDGE_CAP_PER_CYCLE: u32 = 2;
    pub const INTERCEPT_CAP_PER_CYCLE: u32 = 6;
    pub const EMPTY_STOP_CAP: u32 = 3;
    pub const LADDER_TOP: u8 = 3;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eager {
    Off,
    Prelude,
    Force,
}

#[derive(Debug, Default, Clone)]
pub struct Cycle {
    pub work: u32,
    pub nudges: u32,
    pub rung: u8,
    pub last_fingerprint: Option<String>,
    pub intercepts: u32,
    pub empties: u32,
    pub first_listed: bool,
    pub unsourced: bool,
    pub quiet_turns: u32,
    pub closed_nudged: Option<String>,
    pub prompt_claims_impossible: bool,
    pub impossible: bool,
    pub artifact: bool,
}

impl Cycle {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn work(&mut self, landed: u32) -> bool {
        let levers = crate::levers::get();
        self.work = self.work.saturating_add(landed);
        if self.work >= levers.todo_nudge_work && self.nudges < levers.todo_nudge_cap {
            self.nudges = self.nudges.saturating_add(1);
            self.work = 0;
            return true;
        }
        false
    }

    pub fn touched(&mut self) {
        self.work = 0;
    }

    /// A closed list and three turns that changed nothing earn one nudge per closed set.
    pub fn quiet(&mut self, quiet: bool, closed_key: &str) -> bool {
        self.quiet_turns = if quiet {
            self.quiet_turns.saturating_add(1)
        } else {
            0
        };
        if self.quiet_turns < crate::levers::get().todo_quiet_turns
            || self.closed_nudged.as_deref() == Some(closed_key)
        {
            return false;
        }
        self.closed_nudged = Some(closed_key.to_owned());
        self.quiet_turns = 0;
        true
    }

    pub fn first_list(&mut self, landed: u32) -> bool {
        self.work = self.work.saturating_add(landed);
        if self.first_listed || self.work < crate::levers::get().todo_first_list_work {
            return false;
        }
        self.first_listed = true;
        self.work = 0;
        true
    }

    pub fn intercept(&mut self, fingerprint: &str) -> Option<u8> {
        if self.intercepts >= crate::levers::get().todo_intercept_cap {
            return None;
        }
        let unchanged = self.last_fingerprint.as_deref() == Some(fingerprint);
        let next = if unchanged {
            self.rung.saturating_add(1)
        } else {
            1
        };
        if next > gate::LADDER_TOP {
            return None;
        }
        self.rung = next;
        self.last_fingerprint = Some(fingerprint.to_owned());
        self.intercepts = self.intercepts.saturating_add(1);
        Some(self.rung)
    }

    pub fn empty_stop(&mut self) -> bool {
        if self.empties >= crate::levers::get().todo_empty_stop_cap {
            return false;
        }
        self.empties = self.empties.saturating_add(1);
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopPosture {
    Continue,
    Ask,
    Cadence,
    Quiet,
}

pub fn stop_posture(list: &TodoList, children_running: bool) -> StopPosture {
    if children_running || super::mirror::plan_of(list).is_some() {
        return StopPosture::Quiet;
    }
    let mut open = false;
    let mut cadence = false;
    for item in list.items() {
        match item.state {
            TodoStateName::Blocked => match item.on {
                Some(BlockedOn::User) => return StopPosture::Ask,
                Some(BlockedOn::Child) => return StopPosture::Quiet,
                Some(BlockedOn::External) | Some(BlockedOn::Other(_)) | None => cadence = true,
            },
            TodoStateName::Pending | TodoStateName::Running => open = true,
            TodoStateName::Done
            | TodoStateName::Failed
            | TodoStateName::Abandoned
            | TodoStateName::Other(_) => {}
        }
    }
    if open {
        StopPosture::Continue
    } else if cadence {
        StopPosture::Cadence
    } else {
        StopPosture::Quiet
    }
}

pub fn custom(custom_type: &str, text: String, display: bool) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: custom_type.to_owned(),
        content: UserContent::Text(text),
        display,
        details: None,
        timestamp: yi_session::now_ms(),
    }
}

fn open_moves(list: &TodoList) -> String {
    let mut lines = Vec::new();
    for item in list.items().filter(|item| !item.is_closed()) {
        let name = text::name(item);
        let moves = match item.state {
            TodoStateName::Running => format!(
                "done {name} evidence=`<command>` <output line> · block {name} on user note=<what would unblock it> · drop {name} reason=<why>"
            ),
            TodoStateName::Pending => format!("start {name} · drop {name} reason=<why>"),
            _ => format!("unblock {name} · drop {name} reason=<why>"),
        };
        lines.push(format!("- [{}] {}: {moves}", item.state, item.label));
    }
    lines.join("\n")
}

pub fn prelude_text(list: &TodoList) -> String {
    let open = list.progress().open.saturating_add(list.progress().blocked);
    if open == 0 {
        return "Before substantive work, put the whole request in the todo tool: one `init` (phases and items) or `set` (a checklist) covering every item the user named plus investigation and verification, in the same message as your first reads. Then continue.".to_owned();
    }
    format!(
        "The todo list still holds {open} open item(s) from before. Reconcile first: `drop` with a reason what no longer applies, keep what does, and `append` this request's items; then continue.\n{}",
        text::checklist(list).join("\n")
    )
}

pub fn first_list_text() -> String {
    format!(
        "{} changes have landed with no todo list. `init` the list naming what remains, batched with your next call.",
        crate::levers::get().todo_first_list_work
    )
}

/// A numbered request is the list: one pending item per enumerated line, cut to the label
/// max with the line as its note, under the default phase.
pub fn seed(todos: &TodoStore, prompt: &str) -> bool {
    let mut items: Vec<TodoItem> = Vec::new();
    for line in prompt.lines().filter_map(enumerated) {
        let Ok(item) = TodoItem::from_text(line) else {
            continue;
        };
        if !items.iter().any(|seen| seen.label == item.label) {
            items.push(item);
        }
    }
    if items.len() < crate::levers::get().plan_enumerated_min {
        return false;
    }
    let Ok(phase) = PhaseName::new(DEFAULT_PHASE) else {
        return false;
    };
    todos
        .apply_as(
            Op::Init {
                phases: vec![(phase, items)],
            },
            None,
            SEED_ACTOR,
        )
        .is_ok()
}

pub fn seeded_text(list: &TodoList) -> String {
    format!(
        "The todo list was seeded from the request's numbered lines; a long line is cut to its label with the line kept as the note. `append` investigation and verification items, `start` the first, and batch each op with real work.\n{}",
        text::checklist(list).join("\n")
    )
}

pub const CLOSED_LIST_TEXT: &str = "Every item is done and nothing has changed for three turns. Either `append` what remains and `start` it, or write the final answer; call no other tools.";

pub const IMPOSSIBLE_WORDS: [&str; 7] = [
    "not feasible",
    "infeasible",
    "cannot be done",
    "impossible",
    "no valid",
    "no route",
    "no solution",
];

pub const IMPOSSIBLE_TEXT: &str = "Name each constraint you assumed that the task did not state, and relax each one once before reporting that it cannot be done.";

pub fn claims_impossible(text: &str) -> bool {
    let lower = text.to_lowercase();
    IMPOSSIBLE_WORDS.iter().any(|word| lower.contains(word))
}

const QUIET_TOOLS: [&str; 5] = ["read", "grep", "glob", "todo", "get_context"];

/// A turn is quiet when every call reads: the read tools, or bash the gate proves safe.
pub fn quiet_turn(message: &AgentMessage, results: &[AgentMessage]) -> bool {
    let commands = commands_of(message);
    let mut any = false;
    for result in results {
        let AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            ..
        } = result
        else {
            continue;
        };
        any = true;
        let quiet = match tool_name.as_str() {
            "bash" => commands
                .iter()
                .any(|(id, command)| id == tool_call_id && !mutating_command(command)),
            name => QUIET_TOOLS.contains(&name),
        };
        if !quiet {
            return false;
        }
    }
    any
}

const DATA_EXTENSIONS: [&str; 6] = ["txt", "json", "csv", "md", "yaml", "xml"];

fn data_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| DATA_EXTENSIONS.contains(&extension))
}

/// Numerals with three or more significant digits, decimals included, not inside a word.
pub fn figures_of(text: &str) -> Vec<String> {
    let bound = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.'));
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let start = at;
        let mut digits = 0_usize;
        let mut dots = 0_usize;
        while let Some(&c) = chars.get(at) {
            if c.is_ascii_digit() {
                digits = digits.saturating_add(1);
            } else if c == '.'
                && dots == 0
                && chars
                    .get(at.saturating_add(1))
                    .is_some_and(char::is_ascii_digit)
            {
                dots = 1;
            } else {
                break;
            }
            at = at.saturating_add(1);
        }
        if digits >= 3
            && at > start
            && bound(start.checked_sub(1).and_then(|i| chars.get(i).copied()))
            && bound(chars.get(at).copied())
        {
            let figure: String = chars.get(start..at).unwrap_or_default().iter().collect();
            if !out.contains(&figure) {
                out.push(figure);
            }
        }
        at = at.saturating_add(1);
    }
    out
}

/// Figures written to data files this prompt cycle that no tool result or user message shows.
fn artifact_figures(store: &StoreHandle) -> Vec<(String, Vec<String>)> {
    let Some(session) = store() else {
        return Vec::new();
    };
    let entries = yi_session::lock_session(&session)
        .find_entries(&yi_session::EntryQuery {
            entry_type: Some("message"),
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default();
    let mut seen = String::new();
    let mut written: Vec<(String, String)> = Vec::new();
    for entry in entries {
        let yi_types::entry::Entry::Message { message, .. } = entry else {
            continue;
        };
        match message {
            AgentMessage::User {
                content: UserContent::Text(text),
                attribution: Attribution::User,
                ..
            } => {
                written.clear();
                seen.push_str(&text);
                seen.push('\n');
            }
            AgentMessage::User {
                content: UserContent::Text(text),
                ..
            } => {
                seen.push_str(&text);
                seen.push('\n');
            }
            AgentMessage::ToolResult { content, .. } => {
                for block in content {
                    if let Content::Text { text, .. } = block {
                        seen.push_str(&text);
                        seen.push('\n');
                    }
                }
            }
            AgentMessage::Assistant { content, .. } => {
                for block in content {
                    let Content::ToolCall {
                        name, arguments, ..
                    } = block
                    else {
                        continue;
                    };
                    if name != "write" {
                        continue;
                    }
                    let path = arguments.get("path").and_then(serde_json::Value::as_str);
                    let body = arguments.get("content").and_then(serde_json::Value::as_str);
                    if let (Some(path), Some(body)) = (path, body)
                        && data_path(path)
                    {
                        written.push((path.to_owned(), body.to_owned()));
                    }
                }
            }
            _ => {}
        }
    }
    written
        .into_iter()
        .filter_map(|(path, body)| {
            let figures: Vec<String> = figures_of(&body)
                .into_iter()
                .filter(|figure| !seen.contains(figure.as_str()))
                .collect();
            (!figures.is_empty()).then_some((path, figures))
        })
        .collect()
}

pub fn artifact_text(path: &str, figures: &[String]) -> String {
    format!(
        "The file {path} carries numbers no tool result produced: {}. Compute each in a tool and paste its output, or remove it.",
        figures.join(", ")
    )
}

pub fn nudge_text(list: &TodoList) -> String {
    format!(
        "Work has landed since the todo list last moved. Step what you finished (`done` with its evidence), `start` what you are on, and batch the op with your next real call.\n{}",
        open_moves(list)
    )
}

pub fn intercept_text(rung: u8, list: &TodoList) -> String {
    let moves = open_moves(list);
    match rung {
        1 => format!(
            "The turn ended with open todos. Continue the running item, or move each open item to the state that is true: done with its evidence, blocked with what would unblock it, dropped with a reason. Do not restate your answer.\n{moves}"
        ),
        2 => format!(
            "No todo moved since the last stop. A clean stop needs every open item done, blocked on someone else with the blocker named, or dropped with a reason. If you are waiting on the user, `block` the item on user and ask in the same message.\n{moves}"
        ),
        _ => format!(
            "The turn ends now. Write the closing message naming each open item and why it is not done; call no tools.\n{moves}"
        ),
    }
}

/// Digit runs of three or more with no word character or dot on either side: the same
/// numerals the session miner's `count_claim` reads, so the two never disagree.
pub fn numbers_of(text: &str) -> Vec<String> {
    let bound = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.'));
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let start = at;
        while chars.get(at).is_some_and(char::is_ascii_digit) {
            at = at.saturating_add(1);
        }
        let run = at.saturating_sub(start);
        if run >= 3
            && bound(start.checked_sub(1).and_then(|i| chars.get(i).copied()))
            && bound(chars.get(at).copied())
        {
            let number: String = chars.get(start..at).unwrap_or_default().iter().collect();
            if !out.contains(&number) {
                out.push(number);
            }
        }
        at = at.saturating_add(1);
    }
    out
}

/// Every tool result and user message the session has persisted, joined for a substring test.
fn seen_text(store: &StoreHandle) -> String {
    let Some(session) = store() else {
        return String::new();
    };
    let entries = yi_session::lock_session(&session)
        .find_entries(&yi_session::EntryQuery {
            entry_type: Some("message"),
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default();
    let mut seen = String::new();
    for entry in entries {
        let yi_types::entry::Entry::Message { message, .. } = entry else {
            continue;
        };
        match message {
            AgentMessage::ToolResult { content, .. } => {
                for block in content {
                    if let Content::Text { text, .. } = block {
                        seen.push_str(&text);
                        seen.push('\n');
                    }
                }
            }
            AgentMessage::User {
                content: UserContent::Text(text),
                ..
            } => {
                seen.push_str(&text);
                seen.push('\n');
            }
            _ => {}
        }
    }
    seen
}

pub fn unsourced(text: &str, seen: &str) -> Vec<String> {
    numbers_of(text)
        .into_iter()
        .filter(|number| !seen.contains(number.as_str()))
        .collect()
}

pub fn unsourced_text(numbers: &[String]) -> String {
    format!(
        "Each number in the answer needs its source: quote the tool result it came from, or remove it. Numbers without a source: {}.",
        numbers.join(", ")
    )
}

pub const EMPTY_STOP_TEXT: &str =
    "The turn produced no output. Give the final answer, or make the next tool call.";

fn text_of(message: &AgentMessage) -> String {
    let AgentMessage::Assistant { content, .. } = message else {
        return String::new();
    };
    content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_terminal(message: &AgentMessage) -> bool {
    let AgentMessage::Assistant { stop_reason, .. } = message else {
        return true;
    };
    matches!(
        stop_reason,
        StopReason::Error | StopReason::Aborted | StopReason::Length | StopReason::Deferred
    )
}

fn called(results: &[AgentMessage], name: &str) -> bool {
    results.iter().any(
        |result| matches!(result, AgentMessage::ToolResult { tool_name, .. } if tool_name == name),
    )
}

fn commands_of(message: &AgentMessage) -> Vec<(String, String)> {
    let AgentMessage::Assistant { content, .. } = message else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|block| match block {
            Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } if name == "bash" => arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .map(|command| (id.clone(), command.to_owned())),
            _ => None,
        })
        .collect()
}

fn mutating_command(command: &str) -> bool {
    !matches!(
        yi_permission::verdict(command),
        yi_permission::Verdict::Allow
    )
}

pub fn landed(message: &AgentMessage, results: &[AgentMessage]) -> (u32, bool) {
    let commands = commands_of(message);
    let mut landed = 0_u32;
    let mut todo_touched = false;
    for result in results {
        let AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            is_error,
            ..
        } = result
        else {
            continue;
        };
        if *is_error {
            continue;
        }
        match tool_name.as_str() {
            tool::NAME => todo_touched = true,
            "edit" | "write" => landed = landed.saturating_add(1),
            "bash" => {
                if commands
                    .iter()
                    .any(|(id, command)| id == tool_call_id && mutating_command(command))
                {
                    landed = landed.saturating_add(1);
                }
            }
            _ => {}
        }
    }
    (landed, todo_touched)
}

pub struct Options {
    pub eager: Eager,
    pub children_running: Arc<dyn Fn() -> bool + Send + Sync>,
    pub inner: Option<TurnCoupling>,
}

/// The ladder's last rung: the family reads its typed signal as `stuck`.
const LET_GO: &str = "let go";

fn record_intercept(store: &StoreHandle, rung: u8, reason: &str, fingerprint: &str, total: u32) {
    let Some(session) = store() else {
        return;
    };
    let mut extra = serde_json::Map::new();
    if reason == LET_GO {
        let signal = yi_types::subagent::LoopSignal::LetGo;
        extra.insert("signal".to_owned(), serde_json::json!(signal));
    }
    let record = TodoInterceptRecord {
        at: yi_session::now_ms(),
        rung,
        reason: reason.to_owned(),
        fingerprint: fingerprint.to_owned(),
        cycle_total: total,
        extra,
    };
    let Ok(payload) = serde_json::to_value(&record) else {
        return;
    };
    let _a_ledger_write_never_fails_a_turn = yi_session::lock_session(&session).append_custom(
        "main",
        TODO_INTERCEPT_ENTRY_TYPE,
        Some(payload),
    );
}

fn rehydrate(store: &StoreHandle) -> Cycle {
    let mut cycle = Cycle::default();
    let Some(session) = store() else {
        return cycle;
    };
    let entries = yi_session::lock_session(&session)
        .find_entries(&yi_session::EntryQuery {
            custom_type: Some(TODO_INTERCEPT_ENTRY_TYPE.to_owned()),
            order: yi_session::EntryOrder::NewestFirst,
            limit: Some(1),
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default();
    if let Some(yi_types::entry::Entry::Custom {
        data: Some(data), ..
    }) = entries.into_iter().next()
        && let Ok(record) = serde_json::from_value::<TodoInterceptRecord>(data)
    {
        cycle.intercepts = record.cycle_total;
        cycle.rung = record.rung;
        cycle.last_fingerprint = Some(record.fingerprint);
    }
    cycle
}

/// The three claims a clean stop can make that the record does not support, each sent
/// back once per prompt: impossibility the task never named, a figure in an answer file
/// no tool produced, a number in the text no result shows.
fn claim_redrive(
    cycle: &mut Cycle,
    todos: &TodoStore,
    store: &StoreHandle,
    text: &str,
) -> Option<AgentMessage> {
    let fingerprint = todos.list().fingerprint();
    if !cycle.impossible && !cycle.prompt_claims_impossible && claims_impossible(text) {
        cycle.impossible = true;
        record_intercept(store, 0, "impossible", &fingerprint, cycle.intercepts);
        return Some(custom(
            INTERCEPT_CUSTOM_TYPE,
            IMPOSSIBLE_TEXT.to_owned(),
            false,
        ));
    }
    if !cycle.artifact
        && let Some((path, figures)) = artifact_figures(store).into_iter().next()
    {
        cycle.artifact = true;
        record_intercept(store, 0, "artifact", &fingerprint, cycle.intercepts);
        return Some(custom(
            INTERCEPT_CUSTOM_TYPE,
            artifact_text(&path, &figures),
            false,
        ));
    }
    if !cycle.unsourced && !numbers_of(text).is_empty() {
        let numbers = unsourced(text, &seen_text(store));
        if !numbers.is_empty() {
            cycle.unsourced = true;
            record_intercept(store, 0, "unsourced", &fingerprint, cycle.intercepts);
            return Some(custom(
                INTERCEPT_CUSTOM_TYPE,
                unsourced_text(&numbers),
                false,
            ));
        }
    }
    None
}

fn prompt_hook(
    cycle: Arc<Mutex<Cycle>>,
    todos: Arc<TodoStore>,
    deliver: Arc<dyn Fn(AgentMessage) + Send + Sync>,
    inner: Option<Arc<PromptChoiceFn>>,
    eager: Eager,
) -> Arc<PromptChoiceFn> {
    Arc::new(move |prompt: &AgentMessage| {
        if let Ok(mut cycle) = cycle.lock() {
            cycle.reset();
        }
        let AgentMessage::User {
            content: UserContent::Text(text),
            attribution: Attribution::User,
            ..
        } = prompt
        else {
            return inner.as_ref().and_then(|inner| inner(prompt));
        };
        if let Ok(mut cycle) = cycle.lock() {
            cycle.prompt_claims_impossible = claims_impossible(text);
        }
        let list = todos.list();
        let open = list.progress().open.saturating_add(list.progress().blocked) > 0;
        let mirrored = super::mirror::plan_of(&list).is_some();
        if eager == Eager::Off || mirrored || (!open && !eager_init(text)) {
            return inner.as_ref().and_then(|inner| inner(prompt));
        }
        let seeded = !open && seed(&todos, text);
        let list = todos.list();
        let prelude = if seeded {
            seeded_text(&list)
        } else {
            prelude_text(&list)
        };
        deliver(custom(PRELUDE_CUSTOM_TYPE, prelude, false));
        if eager == Eager::Force && !open {
            return ForcedTool::new(tool::NAME).ok().map(ToolChoice::Tool);
        }
        inner.as_ref().and_then(|inner| inner(prompt))
    })
}

pub fn coupling(session: &AgentSession, todos: Arc<TodoStore>, options: Options) -> TurnCoupling {
    let Options {
        eager,
        children_running,
        inner,
    } = options;
    let store = session.store_handle();
    let cycle = Arc::new(Mutex::new(rehydrate(&store)));
    let deliver = session.advisory_hook();

    let on_prompt: Arc<PromptChoiceFn> = prompt_hook(
        Arc::clone(&cycle),
        Arc::clone(&todos),
        Arc::clone(&deliver),
        inner.as_ref().map(|inner| Arc::clone(&inner.on_prompt)),
        eager,
    );

    let on_turn: Arc<TurnObserveFn> = {
        let cycle = Arc::clone(&cycle);
        let todos = Arc::clone(&todos);
        let deliver = Arc::clone(&deliver);
        let inner = inner.as_ref().map(|inner| Arc::clone(&inner.on_turn));
        Arc::new(move |snapshot: &yi_loop::TurnSnapshot| {
            if let Some(inner) = &inner {
                inner(snapshot);
            }
            let (landed, touched) = landed(snapshot.message, snapshot.tool_results);
            let Ok(mut cycle) = cycle.lock() else {
                return;
            };
            if touched {
                cycle.touched();
                return;
            }
            let list = todos.list();
            if super::mirror::plan_of(&list).is_some() {
                return;
            }
            let progress = list.progress();
            if progress.total > 0 && progress.open.saturating_add(progress.blocked) == 0 {
                let quiet = quiet_turn(snapshot.message, snapshot.tool_results);
                let key = format!("{}/{}", progress.done, progress.total);
                if cycle.quiet(quiet, &key) {
                    deliver(custom(
                        NUDGE_CUSTOM_TYPE,
                        CLOSED_LIST_TEXT.to_owned(),
                        false,
                    ));
                }
                return;
            }
            if landed == 0 {
                return;
            }
            if list.progress().total == 0 {
                if cycle.first_list(landed) {
                    deliver(custom(NUDGE_CUSTOM_TYPE, first_list_text(), false));
                }
            } else if list.progress().open > 0 && cycle.work(landed) {
                deliver(custom(NUDGE_CUSTOM_TYPE, nudge_text(&list), false));
            }
        })
    };

    let intercept_stop: Arc<InterceptStopFn> = {
        let cycle = Arc::clone(&cycle);
        let todos = Arc::clone(&todos);
        let inner = inner
            .as_ref()
            .map(|inner| Arc::clone(&inner.intercept_stop));
        Arc::new(move |snapshot: &yi_loop::TurnSnapshot| {
            if is_terminal(snapshot.message) {
                return None;
            }
            let Ok(mut cycle) = cycle.lock() else {
                return None;
            };
            let text = text_of(snapshot.message);
            if text.trim().is_empty() {
                if cycle.empty_stop() {
                    return Some(custom(
                        INTERCEPT_CUSTOM_TYPE,
                        EMPTY_STOP_TEXT.to_owned(),
                        false,
                    ));
                }
                return None;
            }
            if let Some(message) = claim_redrive(&mut cycle, &todos, &store, &text) {
                return Some(message);
            }
            let list = todos.list();
            let posture = stop_posture(&list, children_running());
            if posture != StopPosture::Continue || called(snapshot.tool_results, "ask_user") {
                drop(cycle);
                return inner.as_ref().and_then(|inner| inner(snapshot));
            }
            let fingerprint = list.fingerprint();
            let Some(rung) = cycle.intercept(&fingerprint) else {
                record_intercept(&store, cycle.rung, LET_GO, &fingerprint, cycle.intercepts);
                return None;
            };
            record_intercept(&store, rung, "open", &fingerprint, cycle.intercepts);
            Some(custom(
                INTERCEPT_CUSTOM_TYPE,
                intercept_text(rung, &list),
                rung == 1,
            ))
        })
    };

    TurnCoupling {
        on_prompt,
        on_turn,
        intercept_stop,
    }
}

pub fn install(session: &AgentSession, todos: Arc<TodoStore>, options: Options) {
    session.set_turn_coupling(coupling(session, todos, options));
}
