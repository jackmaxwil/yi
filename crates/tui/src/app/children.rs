use std::collections::{HashMap, HashSet};

use yi_runtime::{ChildFeed, ChildStatus, ChildUpdate};
use yi_types::message::{AgentMessage, Content};
use yi_types::subagent::ChildExit;

use super::App;
use crate::cell::{Cell, TaskCell, TaskStatus, ToolStatus};
use crate::motion::elapsed_ms;
use crate::transcript::{arg_summary, text_of};

pub struct TaskState {
    pub(crate) cell: TaskCell,
    pub(crate) streaming: Option<AgentMessage>,
    pub(crate) started: std::time::Instant,
    pub(crate) finished: Option<std::time::Instant>,
    pub(crate) born_under: Option<String>,
    pub(crate) session: Option<yi_runtime::ChildFeed>,
    pub(crate) rows: crate::cell::AnswerRows,
}

#[derive(Default)]
pub(crate) struct ChildProjection {
    pub(crate) rows: HashMap<String, ChildRow>,
    pub(crate) order: Vec<String>,
    pub(crate) settled_calls: HashSet<String>,
    pub(crate) task_keys: HashMap<String, String>,
}

/// The cause a card carries when the roster stopped listing a child it still showed running.
const GONE: &str = "gone";

/// The last tool a child called, read from its own transcript: a client that subscribed after
/// the event still names it.
fn last_tool_of(session: &ChildFeed) -> Option<String> {
    crate::port::branch_in(session.store())
        .iter()
        .rev()
        .find_map(|entry| match entry {
            yi_types::entry::Entry::Message {
                message: AgentMessage::Assistant { content, .. },
                ..
            } => content.iter().rev().find_map(|block| match block {
                Content::ToolCall {
                    name, arguments, ..
                } => {
                    let args = serde_json::Value::Object(arguments.clone());
                    let summary = arg_summary(name, &args);
                    Some(if summary.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} {summary}")
                    })
                }
                _ => None,
            }),
            _ => None,
        })
}

pub(crate) struct ChildRow {
    id: String,
    start: Cell,
    end: Option<Cell>,
    born_under: Option<String>,
    deferred: bool,
    terminal: bool,
    start_committed: bool,
    end_committed: bool,
}

impl ChildRow {
    fn new(update: &ChildUpdate, born_under: Option<String>) -> Self {
        let mut text = format!("subagent {} ({})", update.name, update.id.as_str());
        if let Some(role) = &update.role {
            text.push_str(&format!(" · {role}"));
        }
        if let Some(task) = &update.task {
            text.push_str(&format!(" · {task}"));
        }
        Self {
            id: update.id.as_str().to_owned(),
            start: Cell::Rule {
                text,
                accent_name: update.id.as_str().to_owned(),
            },
            end: None,
            born_under: born_under.clone(),
            deferred: born_under.is_some(),
            terminal: false,
            start_committed: false,
            end_committed: false,
        }
    }

    fn take_ready(&mut self) -> Vec<Cell> {
        let mut cells = Vec::new();
        if !self.start_committed {
            self.start_committed = true;
            cells.push(self.start.clone());
        }
        if self.terminal
            && !self.end_committed
            && let Some(end) = &self.end
        {
            self.end_committed = true;
            cells.push(end.clone());
        }
        cells
    }
}

fn end_rule(update: &ChildUpdate) -> Cell {
    let completed = update.exit == Some(ChildExit::Completed)
        || (update.exit.is_none() && update.status == ChildStatus::Completed);
    let parent_closed = update.error.as_deref() == Some("parent closed");
    let outcome = if parent_closed {
        "parent closed"
    } else {
        match update.exit {
            Some(ChildExit::Completed) => "done",
            Some(ChildExit::Failed { .. } | ChildExit::Other) => "failed",
            Some(ChildExit::Interrupted) => "cancelled",
            Some(ChildExit::Reaped) => "reaped",
            Some(ChildExit::Repossessed) => "revoked",
            None if completed => "done",
            None => "failed",
        }
    };
    let mut text = format!(
        "subagent {} ({}) · {outcome}",
        update.name,
        update.id.as_str()
    );
    if let Some(ms) = update.elapsed_ms {
        text.push_str(&format!(" · {}", crate::cell::elapsed_label(ms)));
    }
    text.push_str(&format!(" · {} tokens", update.token_count));
    if let Some(cost) = update.cost.as_ref().and_then(serde_json::Number::as_f64) {
        text.push_str(&format!(" · {}", yi_types::message::fmt_cost(cost, false)));
    }
    text.push_str(&format!(" · start {}", update.id.as_str()));
    let detail = if completed {
        update.answer_preview.as_deref()
    } else {
        update.error.as_deref().filter(|_| !parent_closed)
    };
    if let Some(line) = detail.and_then(|text| text.lines().next()) {
        text.push_str(&format!(" · {line}"));
    }
    Cell::Rule {
        text,
        accent_name: update.id.as_str().to_owned(),
    }
}

impl App {
    /// A `/new` or a rewind drops every card with the transcript it belonged to.
    pub(crate) fn reset_transcript(&mut self) {
        self.history.clear();
        self.pending_commit.clear();
        self.reset_live();
        self.live_tools.clear();
        self.user_turns = 0;
        self.retired_tasks.extend(self.task_order.drain(..));
        self.tasks.clear();
        self.committed_tasks.clear();
        self.projection.rows.clear();
        self.projection.order.clear();
        self.projection.settled_calls.clear();
        self.projection.task_keys.clear();
        self.dropped_tasks = 0;
        self.focused = None;
    }

    /// Invariant: the roster is the truth a card is reconciled against. A card still running
    /// that the roster no longer lists ends as `gone`, which heals every lost update.
    pub fn sync_children(&mut self, children: &[yi_runtime::ChildView]) {
        for child in children {
            let spawned_by = child.spawned_by.as_deref();
            self.adopt(&child.update, Some(child.session.clone()), spawned_by);
            let Some(state) = self.tasks.get_mut(child.update.id.as_str()) else {
                continue;
            };
            // Invariant: only a real call is stored, or the previous-step row names an activity.
            if state.cell.last_tool.is_none() {
                state.cell.last_tool = (child.update.tool_use_count > 0)
                    .then(|| last_tool_of(&child.session))
                    .flatten();
            }
        }
        let gone: Vec<ChildUpdate> = self
            .tasks
            .iter()
            .filter(|(id, state)| {
                state.cell.status == TaskStatus::Running
                    && state.session.is_some()
                    && !children.iter().any(|child| child.update.id.as_str() == *id)
            })
            .map(|(id, state)| ChildUpdate {
                id: yi_types::subagent::ChildId(id.clone()),
                name: state.cell.description.clone(),
                status: ChildStatus::Error,
                activity: yi_types::subagent::ChildActivity::Waiting,
                tool_use_count: u64::from(state.cell.toolcalls),
                token_count: state.cell.tokens,
                cost: None,
                answer_preview: None,
                error: Some(GONE.to_owned()),
                exit: None,
                flag: None,
                session: None,
                role: None,
                task: None,
                elapsed_ms: None,
            })
            .collect();
        for update in &gone {
            self.reduce_child_update(update);
        }
    }

    /// A child seen for the first time gets its task cell; every sighting updates it.
    /// `spawned_by` is the spawning cell's call id when the host recorded one.
    pub fn adopt(
        &mut self,
        update: &ChildUpdate,
        session: Option<ChildFeed>,
        spawned_by: Option<&str>,
    ) {
        let id = update.id.as_str().to_owned();
        let key = update.session.as_deref().unwrap_or(&id).to_owned();
        let running = matches!(update.status, ChildStatus::Running | ChildStatus::Other(_));
        if !running
            && (self.retired_tasks.contains(&id)
                || self
                    .projection
                    .rows
                    .get(&key)
                    .is_some_and(|row| row.end_committed))
        {
            return;
        }
        self.retired_tasks.remove(&id);
        if self.tasks.get(&id).is_some_and(|state| {
            self.projection.task_keys.get(&id) != Some(&key)
                && (state.cell.status != TaskStatus::Running || self.committed_tasks.contains(&id))
        }) {
            self.tasks.remove(&id);
            self.task_order.retain(|held| held != &id);
            self.committed_tasks.remove(&id);
        }
        if !self.tasks.contains_key(&id) {
            // Invariant: a feed comes only with the roster's record, where no cell means none ran.
            let (spawn, born_under) = self.spawned_under(spawned_by, session.is_some());
            self.task_order.push(id.clone());
            self.tasks.insert(
                id.clone(),
                TaskState {
                    streaming: None,
                    cell: TaskCell {
                        child_id: id.clone(),
                        description: update.name.clone(),
                        status: TaskStatus::Running,
                        last_tool: None,
                        prev_tool: None,
                        toolcalls: 0,
                        tokens: 0,
                        elapsed_ms: 0,
                        error: None,
                        spawn,
                        answer: None,
                        answer_dropped: 0,
                        activity: update.activity,
                        flag: None,
                    },
                    started: self.clock.now(),
                    finished: None,
                    born_under: born_under.clone(),
                    session,
                    rows: crate::cell::AnswerRows::default(),
                },
            );
            self.scheduler.request();
        }
        self.projection.task_keys.insert(id.clone(), key.clone());
        if !self.projection.rows.contains_key(&key) {
            let born_under = self
                .tasks
                .get(&id)
                .and_then(|state| state.born_under.clone());
            self.projection.order.push(key.clone());
            self.projection
                .rows
                .insert(key, ChildRow::new(update, born_under));
        }
        self.reduce_child_update(update);
    }

    /// Invariant: the sole source of a child's status and counters; a task cell
    /// commits after the tool cell it was born under, never before.
    pub fn reduce_child_update(&mut self, update: &ChildUpdate) {
        let id = update.id.as_str();
        let Some(state) = self.tasks.get_mut(id) else {
            return;
        };
        let status = match update.status {
            ChildStatus::Running | ChildStatus::Other(_) => TaskStatus::Running,
            ChildStatus::Completed => TaskStatus::Done,
            ChildStatus::Error => TaskStatus::Failed,
        };
        let toolcalls = u32::try_from(update.tool_use_count).unwrap_or(u32::MAX);
        let row_key = self
            .projection
            .task_keys
            .get(id)
            .cloned()
            .unwrap_or_else(|| update.session.as_deref().unwrap_or(id).to_owned());
        if state.cell.status != status
            || state.cell.toolcalls != toolcalls
            || state.cell.tokens != update.token_count
            || state.cell.activity != update.activity
            || state.cell.flag != update.flag
        {
            if status == TaskStatus::Running {
                if let Some(row) = self.projection.rows.get_mut(&row_key) {
                    row.terminal = false;
                    row.end = None;
                    row.end_committed = false;
                }
                self.committed_tasks.remove(id);
                if state.finished.is_some() {
                    state.finished = None;
                    state.started = self.clock.now();
                    state.cell.answer = None;
                    state.cell.answer_dropped = 0;
                }
            }
            state.cell.flag = update.flag.clone();
            state.cell.status = status;
            state.cell.activity = update.activity;
            state.cell.error = update.error.clone();
            state.cell.toolcalls = toolcalls;
            state.cell.tokens = update.token_count;
            state.cell.elapsed_ms = elapsed_ms(state.started, self.clock.now());
            // The record's preview outranks a folded stream that lost events to a gap, unless the
            // card cut that stream's head itself: a tail never holds the head the preview is.
            if let Some(preview) = &update.answer_preview {
                let folded = state.cell.answer.as_deref().unwrap_or_default();
                let folded = folded.split_whitespace().collect::<Vec<_>>().join(" ");
                // Invariant: a cut preview ends in the runtime's cut row; the answer's head precedes it.
                let head = preview
                    .rsplit_once(yi_runtime::mailbox::CUT_ROW)
                    .map_or(preview.as_str(), |cut| cut.0);
                if state.cell.answer_dropped == 0 && !folded.contains(head) {
                    state.cell.answer = Some(preview.clone());
                }
            }
            if status != TaskStatus::Running && state.finished.is_none() {
                state.finished = Some(self.clock.now());
                let answer = state
                    .session
                    .as_ref()
                    .map(|session| session.messages())
                    .unwrap_or_default()
                    .iter()
                    .rev()
                    .find_map(|m| match m {
                        AgentMessage::Assistant { content, .. } => Some(text_of(content)),
                        _ => None,
                    });
                if let Some(text) = answer.filter(|t| !t.trim().is_empty()) {
                    state.cell.keep_answer(text);
                }
            }
            self.scheduler.request();
        }
        if status != TaskStatus::Running
            && let Some(row) = self.projection.rows.get_mut(&row_key)
        {
            row.end = Some(end_rule(update));
            row.terminal = true;
        }
        self.commit_finished_tasks();
    }

    /// The spawning cell is an ordering hint, not a gate: a missing cell end is released at turn settlement.
    pub(super) fn commit_finished_tasks(&mut self) {
        for row in self.projection.rows.values_mut() {
            if row
                .born_under
                .as_ref()
                .is_some_and(|call| self.projection.settled_calls.contains(call))
            {
                row.deferred = false;
            }
        }
        loop {
            let order = self.projection.order.clone();
            let mut moved = false;
            for key in order {
                let ready = self.projection.rows.get(&key).is_some_and(|row| {
                    !row.deferred
                        && !row.born_under.as_deref().is_some_and(|call| {
                            self.live_tools
                                .iter()
                                .any(|tool| tool.call_id == call && tool.status != ToolStatus::Done)
                        })
                        && (!row.start_committed
                            || (row.terminal && !row.end_committed && row.end.is_some()))
                });
                if !ready {
                    continue;
                }
                let Some((cells, id)) = self.projection.rows.get_mut(&key).map(|row| {
                    let cells = row.take_ready();
                    (cells, row.end_committed.then(|| row.id.clone()))
                }) else {
                    continue;
                };
                if let Some(id) = id {
                    self.committed_tasks.insert(id);
                }
                for cell in cells {
                    self.commit_cell(&cell);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        while self.committed_tasks.len() > crate::agents::FINISHED_CARDS {
            let focused = self.focused.as_ref();
            let Some(at) = (self.task_order.iter())
                .position(|id| self.committed_tasks.contains(id) && focused != Some(id))
            else {
                break;
            };
            let id = self.task_order.remove(at);
            self.tasks.remove(&id);
            self.committed_tasks.remove(&id);
            self.retired_tasks.insert(id);
            self.dropped_tasks = self.dropped_tasks.saturating_add(1);
        }
    }

    pub(crate) fn release_child_rows(&mut self) {
        for row in self.projection.rows.values_mut() {
            row.deferred = false;
        }
        self.commit_finished_tasks();
    }
}
