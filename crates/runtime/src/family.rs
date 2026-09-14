//! a family member's state as its own records show it, and the lines that carry it (D165).

use serde_json::Value;
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content};
use yi_types::subagent::ChildStatus;

/// A running member with no new record for this long is `stuck` with note `idle Ns`.
pub const STUCK_IDLE_MS: u64 = 300_000;
const RECENT_ENTRIES: usize = 3;
const NOTE_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberState {
    Running,
    Finished,
    Failed,
    NeedsYou,
    Stuck,
}

impl MemberState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::NeedsYou => "needs_you",
            Self::Stuck => "stuck",
        }
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

/// The question a member ended its turn on, when its last assistant message called `ask_user`.
pub fn pending_question(messages: &[AgentMessage]) -> Option<String> {
    let last = messages
        .iter()
        .rev()
        .find(|message| matches!(message, AgentMessage::Assistant { .. }))?;
    let AgentMessage::Assistant { content, .. } = last else {
        return None;
    };
    content.iter().find_map(|block| match block {
        Content::ToolCall {
            name, arguments, ..
        } if name == "ask_user" => Some(cut(arguments
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or("asked a question"))),
        _ => None,
    })
}

/// A stuck or waiting signal in one recent record, as the loop and the coupling wrote it.
fn signal_of(entry: &Entry) -> Option<(MemberState, String)> {
    match entry {
        Entry::Message {
            message:
                AgentMessage::Custom {
                    custom_type,
                    details,
                    ..
                },
            ..
        } => match custom_type.as_str() {
            "repeat_break" => Some((MemberState::Stuck, "repeat_break".to_owned())),
            "length_redrive" => {
                let rung = details
                    .as_ref()
                    .and_then(|d| d.get("rung"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                (rung >= 2).then(|| (MemberState::Stuck, format!("length_redrive rung {rung}")))
            }
            _ => None,
        },
        Entry::Custom {
            custom_type, data, ..
        } => {
            let field = |key: &str| {
                data.as_ref()
                    .and_then(|d| d.get(key))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            match custom_type.as_str() {
                "todo_intercept" if field("reason") == "let go" => {
                    Some((MemberState::Stuck, "todo_intercept let go".to_owned()))
                }
                "todo" if field("op") == "block" && field("on") == "user" => {
                    Some((MemberState::NeedsYou, "blocked on user".to_owned()))
                }
                _ => None,
            }
        }
        _ => None,
    }
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

/// The state a member's own newest records show: `needs_you` when it ended on `ask_user` or
/// blocked a todo on the user, `stuck` when the loop or the coupling re-drove it in its last
/// records or nothing moved for [`STUCK_IDLE_MS`]; the note names which.
pub fn state_from_records(
    status: ChildStatus,
    error: Option<&str>,
    messages: &[AgentMessage],
    recent: &[Entry],
    now_ms: u64,
) -> (MemberState, Option<String>, u64) {
    let newest = recent.iter().map(timestamp_of).max().unwrap_or(now_ms);
    let idle_s = now_ms.saturating_sub(newest) / 1000;
    match status {
        ChildStatus::Error => (MemberState::Failed, error.map(cut), idle_s),
        ChildStatus::Completed => match pending_question(messages) {
            Some(question) => (MemberState::NeedsYou, Some(question), idle_s),
            None => (MemberState::Finished, None, idle_s),
        },
        ChildStatus::Running => {
            if let Some((state, note)) = recent.iter().find_map(signal_of) {
                return (state, Some(note), idle_s);
            }
            if now_ms.saturating_sub(newest) >= STUCK_IDLE_MS {
                return (MemberState::Stuck, Some(format!("idle {idle_s}s")), idle_s);
            }
            (MemberState::Running, None, idle_s)
        }
    }
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
        (MemberState::Running, "running"),
        (MemberState::Finished, "finished"),
        (MemberState::Failed, "failed"),
        (MemberState::NeedsYou, "needs you"),
        (MemberState::Stuck, "stuck"),
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
