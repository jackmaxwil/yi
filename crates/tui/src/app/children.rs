use std::time::Instant;

use yi_runtime::{ChildFeed, ChildStatus, ChildUpdate};
use yi_types::message::{AgentMessage, Content};

use super::{App, TaskState};
use crate::cell::{Cell, TaskCell, TaskStatus, ToolStatus};
use crate::motion::elapsed_ms;
use crate::transcript::{arg_summary, text_of};

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

impl App {
    /// Invariant: the roster is the truth a card is reconciled against. A card still running
    /// that the roster no longer lists ends as `gone`, which heals every lost update.
    pub fn sync_children(&mut self, children: &[yi_runtime::ChildView]) {
        for child in children {
            self.adopt(&child.update, Some(child.session.clone()));
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
                answer_preview: None,
                error: Some(GONE.to_owned()),
                exit: None,
                flag: None,
            })
            .collect();
        for update in &gone {
            self.reduce_child_update(update);
        }
    }

    /// A child seen for the first time gets its task cell; every sighting updates it.
    pub fn adopt(&mut self, update: &ChildUpdate, session: Option<ChildFeed>) {
        let id = update.id.as_str().to_owned();
        if !self.tasks.contains_key(&id) {
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
                        spawn: self.spawning_cell(),
                        answer: None,
                        activity: update.activity,
                        flag: None,
                    },
                    started: Instant::now(),
                    finished: None,
                    born_under: self.spawning_call(),
                    session,
                },
            );
            self.scheduler.request();
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
        if state.cell.status != status
            || state.cell.toolcalls != toolcalls
            || state.cell.tokens != update.token_count
            || state.cell.activity != update.activity
            || state.cell.flag != update.flag
        {
            if status == TaskStatus::Running && state.finished.is_some() {
                state.finished = None;
                state.started = Instant::now();
                state.cell.answer = None;
                self.committed_tasks.remove(id);
            }
            state.cell.flag = update.flag.clone();
            state.cell.status = status;
            state.cell.activity = update.activity;
            state.cell.error = update.error.clone();
            state.cell.toolcalls = toolcalls;
            state.cell.tokens = update.token_count;
            state.cell.elapsed_ms = elapsed_ms(state.started);
            // The record's preview outranks a folded stream that lost events to a gap.
            if let Some(preview) = &update.answer_preview {
                let folded = state.cell.answer.as_deref().unwrap_or_default();
                let folded = folded.split_whitespace().collect::<Vec<_>>().join(" ");
                let head = preview.trim_end_matches('…');
                if !folded.contains(head) {
                    state.cell.answer = Some(preview.clone());
                }
            }
            if status != TaskStatus::Running && state.finished.is_none() {
                state.finished = Some(Instant::now());
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
                    state.cell.answer = Some(crate::cell::tail_bounded(text));
                }
            }
            self.scheduler.request();
        }
        self.commit_finished_tasks();
    }

    /// The spawning cell is an ordering hint, not a gate: a finished card waits only on the
    /// one cell it was born under, so a lost cell end cannot hold every other card.
    pub(super) fn commit_finished_tasks(&mut self) {
        for id in self.task_order.clone() {
            let Some(state) = self.tasks.get(&id) else {
                continue;
            };
            let under_live_cell = state.born_under.as_deref().is_some_and(|call| {
                self.live_tools
                    .iter()
                    .any(|tool| tool.call_id == call && tool.status != ToolStatus::Done)
            });
            let cell = state.cell.clone();
            if under_live_cell
                || cell.status == TaskStatus::Running
                || !self.committed_tasks.insert(id)
            {
                continue;
            }
            self.commit_cell(&Cell::Task(cell));
        }
    }
}
