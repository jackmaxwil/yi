use std::sync::{Arc, Mutex};

use yi_types::message::{AgentMessage, Attribution, Content, StopReason, UserContent};
use yi_types::model::{ForcedTool, ToolChoice};
use yi_types::plan::doc::{TODO_LABEL_MAX, TodoLabel, TodoStateName};
use yi_types::todo::PhaseName;
use yi_types::todo::{BlockedOn, TODO_INTERCEPT_ENTRY_TYPE, TodoInterceptRecord, TodoList};

use super::{DEFAULT_PHASE, Op, TodoStore, text, tool};
use crate::goal::StoreHandle;
use crate::plan::loop_coupling::gate::{ENUMERATED_ITEMS_MIN, eager_init, enumerated};
use crate::session::{AgentSession, InterceptStopFn, PromptChoiceFn, TurnCoupling, TurnObserveFn};

pub const NUDGE_CUSTOM_TYPE: &str = "todo_nudge";
pub const PRELUDE_CUSTOM_TYPE: &str = "todo_prelude";
pub const INTERCEPT_CUSTOM_TYPE: &str = "todo_intercept";
pub const SEED_ACTOR: &str = "prompt";

pub mod gate {
    pub const NUDGE_WORK: u32 = 12;
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
}

impl Cycle {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn work(&mut self, landed: u32) -> bool {
        self.work = self.work.saturating_add(landed);
        if self.work >= gate::NUDGE_WORK && self.nudges < gate::NUDGE_CAP_PER_CYCLE {
            self.nudges = self.nudges.saturating_add(1);
            self.work = 0;
            return true;
        }
        false
    }

    pub fn touched(&mut self) {
        self.work = 0;
    }

    pub fn first_list(&mut self, landed: u32) -> bool {
        self.work = self.work.saturating_add(landed);
        if self.first_listed || self.work < gate::FIRST_LIST_WORK {
            return false;
        }
        self.first_listed = true;
        self.work = 0;
        true
    }

    pub fn intercept(&mut self, fingerprint: &str) -> Option<u8> {
        if self.intercepts >= gate::INTERCEPT_CAP_PER_CYCLE {
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
        if self.empties >= gate::EMPTY_STOP_CAP {
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
    if children_running {
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
        let label = item.label.as_str();
        let moves = match item.state {
            TodoStateName::Running => format!(
                "done {label:?} evidence=<the check that passed> · block {label:?} on user note=<what would unblock it> · drop {label:?} reason=<why>"
            ),
            TodoStateName::Pending => format!("start {label:?} · drop {label:?} reason=<why>"),
            _ => format!("unblock {label:?} · drop {label:?} reason=<why>"),
        };
        lines.push(format!("- [{}] {label}: {moves}", item.state));
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
        gate::FIRST_LIST_WORK
    )
}

/// A numbered request is the list: one pending item per enumerated line, cut to the label
/// max, under the default phase, so the model starts from the user's own items.
pub fn seed(todos: &TodoStore, prompt: &str) -> bool {
    let mut labels: Vec<TodoLabel> = Vec::new();
    for line in prompt.lines().filter_map(enumerated) {
        let text: String = line.trim().chars().take(TODO_LABEL_MAX).collect();
        let Ok(label) = TodoLabel::new(text.trim_end()) else {
            continue;
        };
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    if labels.len() < ENUMERATED_ITEMS_MIN {
        return false;
    }
    let Ok(phase) = PhaseName::new(DEFAULT_PHASE) else {
        return false;
    };
    todos
        .apply_as(
            Op::Init {
                phases: vec![(phase, labels)],
            },
            None,
            SEED_ACTOR,
        )
        .is_ok()
}

pub fn seeded_text(list: &TodoList) -> String {
    format!(
        "The todo list was seeded from the request's numbered lines. `append` investigation and verification items, `start` the first, and batch each op with real work.\n{}",
        text::checklist(list).join("\n")
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

fn record_intercept(store: &StoreHandle, rung: u8, reason: &str, fingerprint: &str, total: u32) {
    let Some(session) = store() else {
        return;
    };
    let record = TodoInterceptRecord {
        at: yi_session::now_ms(),
        rung,
        reason: reason.to_owned(),
        fingerprint: fingerprint.to_owned(),
        cycle_total: total,
        extra: serde_json::Map::new(),
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

pub fn coupling(session: &AgentSession, todos: Arc<TodoStore>, options: Options) -> TurnCoupling {
    let Options {
        eager,
        children_running,
        inner,
    } = options;
    let store = session.store_handle();
    let cycle = Arc::new(Mutex::new(rehydrate(&store)));
    let deliver = session.advisory_hook();

    let on_prompt: Arc<PromptChoiceFn> = {
        let cycle = Arc::clone(&cycle);
        let todos = Arc::clone(&todos);
        let deliver = Arc::clone(&deliver);
        let inner = inner.as_ref().map(|inner| Arc::clone(&inner.on_prompt));
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
            let list = todos.list();
            let open = list.progress().open.saturating_add(list.progress().blocked) > 0;
            if eager == Eager::Off || (!open && !eager_init(text)) {
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
    };

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
            if landed == 0 {
                return;
            }
            let list = todos.list();
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
            if text_of(snapshot.message).trim().is_empty() {
                if cycle.empty_stop() {
                    return Some(custom(
                        INTERCEPT_CUSTOM_TYPE,
                        EMPTY_STOP_TEXT.to_owned(),
                        false,
                    ));
                }
                return None;
            }
            let list = todos.list();
            let posture = stop_posture(&list, children_running());
            if posture != StopPosture::Continue || called(snapshot.tool_results, "ask_user") {
                drop(cycle);
                return inner.as_ref().and_then(|inner| inner(snapshot));
            }
            let fingerprint = list.fingerprint();
            let Some(rung) = cycle.intercept(&fingerprint) else {
                record_intercept(&store, cycle.rung, "let go", &fingerprint, cycle.intercepts);
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
