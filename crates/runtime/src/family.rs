//! a family member's state as its own records show it, and the lines that carry it (D165).

use serde_json::Value;
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content};
use yi_types::plan::doc::TodoStateName;
use yi_types::subagent::{ChildExit, ChildFlag, ChildStatus, ChildUpdate, LoopSignal};
use yi_types::todo::{BlockedOn, TodoRecord};

/// A running member with no new record for this long is `stuck` with note `idle Ns`.
pub const STUCK_IDLE_MS: u64 = 300_000;
const RECENT_ENTRIES: usize = 3;
const NOTE_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberState {
    /// Admitted and not yet polled.
    Queued,
    Running,
    Finished,
    Failed,
    NeedsYou,
    Stuck,
    /// A revoked child whose stop, settle or record failed; everything it held is kept.
    RepossessionPending,
}

impl MemberState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::NeedsYou => "needs_you",
            Self::Stuck => "stuck",
            Self::RepossessionPending => "repossession_pending",
        }
    }
}

/// Why a member's record last moved, which `wait` names beside the member's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    Spawned,
    Started,
    Mail,
    Progress,
    Asked,
    Finished,
    Failed,
    Interrupted,
    Reaped,
    Respawned,
    Revoked,
    Held,
    Settled,
    Stuck,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spawned => "spawned",
            Self::Started => "started",
            Self::Mail => "mail",
            Self::Progress => "progress",
            Self::Asked => "asked",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Reaped => "reaped",
            Self::Respawned => "respawned",
            Self::Revoked => "revoked",
            Self::Held => "held",
            Self::Settled => "settled",
            Self::Stuck => "stuck",
        }
    }

    pub fn ended(exit: ChildExit) -> Self {
        match exit {
            ChildExit::Completed => Self::Finished,
            ChildExit::Interrupted => Self::Interrupted,
            ChildExit::Reaped => Self::Reaped,
            ChildExit::Repossessed => Self::Revoked,
            ChildExit::Failed { .. } | ChildExit::Other => Self::Failed,
        }
    }
}

/// Where a record with no exit stands: admitted and not yet polled, live, told by the child
/// itself that its work failed, or held by a repossession that has not finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Live,
    Failed,
    Repossessing,
    Pending,
}

/// One exit read three ways: the wire status, the member state and the notice's verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub status: ChildStatus,
    pub state: MemberState,
    pub verb: &'static str,
}

/// Invariant: every surface reads a child's ending through here, so the TUI's status, the
/// model's `wait` state and the parent's notice cannot disagree. `None` is a live run.
pub fn read_exit(exit: Option<ChildExit>) -> Reading {
    let (status, state, verb) = match exit {
        None => (ChildStatus::Running, MemberState::Running, "running"),
        Some(ChildExit::Completed) => (ChildStatus::Completed, MemberState::Finished, "finished"),
        Some(ChildExit::Failed { .. }) => (ChildStatus::Error, MemberState::Failed, "failed"),
        Some(ChildExit::Interrupted) => (ChildStatus::Error, MemberState::Failed, "interrupted"),
        Some(ChildExit::Reaped) => (ChildStatus::Error, MemberState::Failed, "reaped"),
        Some(ChildExit::Repossessed) => (ChildStatus::Error, MemberState::Failed, "repossessed"),
        Some(ChildExit::Other) => (ChildStatus::Error, MemberState::Failed, "ended"),
    };
    Reading {
        status,
        state,
        verb,
    }
}

#[derive(Debug, Clone)]
pub struct MemberView {
    pub name: String,
    pub state: MemberState,
    pub note: Option<String>,
    pub tools: u64,
    pub tokens: u64,
    pub idle_s: u64,
    pub worktree: Option<String>,
}

fn cut(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() > NOTE_CHARS {
        format!("{}…", line.chars().take(NOTE_CHARS).collect::<String>())
    } else {
        line.to_owned()
    }
}

/// A stuck or waiting signal in one recent record. `stuck` is the loop's typed `signal`,
/// never a record's name: a renamed or look-alike record cannot make or hide a stuck child.
fn signal_of(entry: &Entry) -> Option<(MemberState, String)> {
    let (data, todo) = match entry {
        Entry::Message {
            message: AgentMessage::Custom { details, .. },
            ..
        } => (details.as_ref()?, false),
        Entry::Custom {
            custom_type, data, ..
        } => (data.as_ref()?, custom_type == "todo"),
        _ => return None,
    };
    if todo {
        let record = serde_json::from_value::<TodoRecord>(data.clone()).ok()?;
        let asked = record.list.items().find(|item| {
            item.state == TodoStateName::Blocked && item.on == Some(BlockedOn::User)
        })?;
        let (suffix, options) = (
            crate::todo::text::suffix(asked),
            crate::todo::text::asked(asked),
        );
        let note = format!("{}{suffix}{options}", asked.label);
        return Some((MemberState::NeedsYou, note));
    }
    let signal = serde_json::from_value::<LoopSignal>(data.get("signal")?.clone()).ok()?;
    let rung = data.get("rung").and_then(Value::as_u64).unwrap_or(0);
    let note = match signal {
        LoopSignal::RepeatBreak => "repeat_break".to_owned(),
        LoopSignal::LengthRedrive if rung >= 2 => format!("length_redrive rung {rung}"),
        LoopSignal::LengthRedrive => return None,
        LoopSignal::LetGo => "todo_intercept let go".to_owned(),
    };
    Some((MemberState::Stuck, note))
}

fn timestamp_of(entry: &Entry) -> u64 {
    match entry {
        Entry::Message { timestamp, .. }
        | Entry::ModelChange { timestamp, .. }
        | Entry::ThinkingLevelChange { timestamp, .. }
        | Entry::ActiveToolsChange { timestamp, .. }
        | Entry::Compaction { timestamp, .. }
        | Entry::BranchSummary { timestamp, .. }
        | Entry::Custom { timestamp, .. } => *timestamp,
    }
}

/// The state a member's newest records show: `needs_you` after a todo blocked on the user,
/// `stuck` after a re-drive in them or [`STUCK_IDLE_MS`] idle; the note says which.
pub fn state_from_records(
    (exit, phase): (Option<ChildExit>, Phase),
    error: Option<&str>,
    recent: &[Entry],
    now_ms: u64,
) -> (MemberState, Option<String>, u64) {
    let newest = recent.iter().map(timestamp_of).max().unwrap_or(now_ms);
    let idle_s = now_ms.saturating_sub(newest) / 1000;
    match read_exit(exit).state {
        MemberState::Running if phase == Phase::Queued => (MemberState::Queued, None, idle_s),
        MemberState::Running if phase == Phase::Failed => {
            (MemberState::Failed, error.map(cut), idle_s)
        }
        MemberState::Running if phase == Phase::Pending => {
            (MemberState::RepossessionPending, error.map(cut), idle_s)
        }
        MemberState::Running => {
            if let Some((state, note)) = recent.iter().find_map(signal_of) {
                return (state, Some(note), idle_s);
            }
            if now_ms.saturating_sub(newest)
                >= crate::levers::get()
                    .family_stuck_idle_s
                    .saturating_mul(1000)
            {
                return (MemberState::Stuck, Some(format!("idle {idle_s}s")), idle_s);
            }
            (MemberState::Running, None, idle_s)
        }
        state => (state, error.map(cut), idle_s),
    }
}

pub fn flagged(mut update: ChildUpdate, views: &[MemberView]) -> ChildUpdate {
    let view = views.iter().find(|view| view.name == update.name);
    let note = || view.and_then(|view| view.note.clone()).unwrap_or_default();
    update.flag = match view.map(|view| view.state) {
        Some(MemberState::NeedsYou) => Some(ChildFlag::NeedsYou { note: note() }),
        Some(MemberState::Stuck) => Some(ChildFlag::Stuck { note: note() }),
        _ => None,
    };
    update
}

/// One stuck notice per episode (plan section 7.5): a member is latched by name when first
/// seen `Stuck` and released when its records move it off `Stuck`, or when it is gone.
#[derive(Debug, Default)]
pub struct StuckLatch(std::collections::HashSet<String>);

impl StuckLatch {
    /// The `[child <name> stuck: <note>]` notices the members newly stuck this tick earn.
    pub fn notices(&mut self, views: &[MemberView]) -> Vec<String> {
        let mut notices = Vec::new();
        for view in views {
            if view.state != MemberState::Stuck {
                self.0.remove(&view.name);
            } else if self.0.insert(view.name.clone()) {
                let note = view.note.as_deref().unwrap_or("no note");
                notices.push(format!("[child {} stuck: {note}]", view.name));
            }
        }
        self.0
            .retain(|name| views.iter().any(|view| view.name == *name));
        notices
    }
}

pub fn recent_entries(session: &yi_session::SharedSession) -> Vec<Entry> {
    yi_session::lock_session(session)
        .find_entries(&yi_session::EntryQuery {
            order: yi_session::EntryOrder::NewestFirst,
            limit: Some(RECENT_ENTRIES),
            ..yi_session::EntryQuery::default()
        })
        .unwrap_or_default()
}

/// `children: 2 running (a, b) · 1 needs you (d: asked a question)`; `None` with no members.
pub fn children_line(views: &[MemberView]) -> Option<String> {
    if views.is_empty() {
        return None;
    }
    let groups = [
        (MemberState::Queued, "queued"),
        (MemberState::Running, "running"),
        (MemberState::Finished, "finished"),
        (MemberState::Failed, "failed"),
        (MemberState::NeedsYou, "needs you"),
        (MemberState::Stuck, "stuck"),
        (MemberState::RepossessionPending, "repossession pending"),
    ];
    let parts: Vec<String> = groups
        .iter()
        .filter_map(|(state, label)| {
            let members: Vec<String> = views
                .iter()
                .filter(|view| view.state == *state)
                .map(|view| match &view.note {
                    Some(note) => format!("{}: {note}", view.name),
                    None => view.name.clone(),
                })
                .collect();
            (!members.is_empty())
                .then(|| format!("{} {label} ({})", members.len(), members.join(", ")))
        })
        .collect();
    Some(format!("children: {}", parts.join(" · ")))
}

/// One line per entry for `history://<agent>/tail/N`: the sequence, who, and the first line.
pub fn compact_entry(entry: &Entry) -> String {
    let seq = match entry {
        Entry::Message { seq, .. }
        | Entry::ModelChange { seq, .. }
        | Entry::ThinkingLevelChange { seq, .. }
        | Entry::ActiveToolsChange { seq, .. }
        | Entry::Compaction { seq, .. }
        | Entry::BranchSummary { seq, .. }
        | Entry::Custom { seq, .. } => *seq,
    };
    let body = match entry {
        Entry::Message { message, .. } => match message {
            AgentMessage::Assistant { content, .. } => {
                let calls: Vec<String> = content
                    .iter()
                    .filter_map(|block| match block {
                        Content::ToolCall {
                            name, arguments, ..
                        } => Some(format!(
                            "{name} {}",
                            cut(&Value::Object(arguments.clone()).to_string())
                        )),
                        _ => None,
                    })
                    .collect();
                let text: String = content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text { text, .. } => Some(cut(text)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("assistant: {}", [text, calls.join("; ")].join(" ").trim())
            }
            AgentMessage::ToolResult {
                tool_name, content, ..
            } => {
                let first = content
                    .iter()
                    .find_map(|block| match block {
                        Content::Text { text, .. } => Some(cut(text)),
                        _ => None,
                    })
                    .unwrap_or_default();
                format!("result {tool_name}: {first}")
            }
            AgentMessage::User { content, .. } => match content {
                yi_types::message::UserContent::Text(text) => format!("user: {}", cut(text)),
                yi_types::message::UserContent::Blocks(_) => "user: [blocks]".to_owned(),
            },
            AgentMessage::Custom {
                custom_type,
                content,
                ..
            } => match content {
                yi_types::message::UserContent::Text(text) => {
                    format!("{custom_type}: {}", cut(text))
                }
                yi_types::message::UserContent::Blocks(_) => custom_type.clone(),
            },
            AgentMessage::BashExecution { .. } => "bash execution".to_owned(),
            AgentMessage::BranchSummary { .. } => "branch summary".to_owned(),
            AgentMessage::CompactionSummary { .. } => "compaction summary".to_owned(),
        },
        Entry::BranchSummary { .. } => "branch summary".to_owned(),
        Entry::Custom { custom_type, .. } => format!("custom {custom_type}"),
        Entry::ModelChange { model_id, .. } => format!("model {model_id}"),
        Entry::ThinkingLevelChange { thinking_level, .. } => format!("thinking {thinking_level}"),
        Entry::ActiveToolsChange { .. } => "tools changed".to_owned(),
        Entry::Compaction { .. } => "compaction".to_owned(),
    };
    format!("#{seq} {body}")
}
