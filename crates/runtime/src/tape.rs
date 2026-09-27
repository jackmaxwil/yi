use yi_types::checkpoint::CHECKPOINT_ENTRY_TYPE;
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;
use yi_types::tape::{Mark, MarkKind, Tape};

use crate::AgentSession;

pub fn tape(entries: &[Entry]) -> Tape {
    let mut tape = Tape::default();
    let mut previous: Option<u64> = None;
    let mut asked_at: Option<u64> = None;
    for entry in entries {
        let (id, at) = match entry {
            Entry::Message { id, timestamp, .. } | Entry::Custom { id, timestamp, .. } => {
                (id.clone(), *timestamp)
            }
            Entry::Compaction { id, timestamp, .. } => (id.clone(), *timestamp),
            _ => continue,
        };
        if tape.start == 0 {
            tape.start = at;
        }
        tape.end = tape.end.max(at);
        let mark = |kind: MarkKind, label: String| Mark {
            at,
            kind,
            entry: id.clone(),
            label,
        };
        match entry {
            Entry::Message { message, .. } => match message {
                AgentMessage::User { .. } if message.attribution().reads_as_typed(at) => {
                    let label: String = message.plain_text().chars().take(60).collect();
                    tape.marks.push(mark(MarkKind::User, label));
                }
                AgentMessage::Assistant { .. } => {
                    if let Some(from) = previous {
                        tape.model.push([from, at]);
                    }
                    asked_at = Some(at);
                }
                AgentMessage::ToolResult {
                    tool_name,
                    is_error,
                    ..
                } => {
                    // Invariant: parallel calls share one ask, so their spans merge into one.
                    match (asked_at, tape.tools.last_mut()) {
                        (Some(from), Some(last)) if last[1] >= from => last[1] = last[1].max(at),
                        (Some(from), _) => tape.tools.push([from, at]),
                        (None, _) => {}
                    }
                    if *is_error {
                        tape.marks
                            .push(mark(MarkKind::Failed, format!("{tool_name} failed")));
                    }
                }
                _ => {}
            },
            Entry::Custom { custom_type, .. } if custom_type == CHECKPOINT_ENTRY_TYPE => {
                tape.marks
                    .push(mark(MarkKind::Checkpoint, "checkpoint".to_owned()));
            }
            Entry::Compaction { .. } => {
                tape.marks
                    .push(mark(MarkKind::Compaction, "compaction".to_owned()));
            }
            _ => {}
        }
        previous = Some(at);
    }
    tape
}

pub fn session_tape(session: &AgentSession) -> Tape {
    let Some(store) = session.store() else {
        return Tape::default();
    };
    let entries = yi_session::lock_session(&store)
        .find_entries_on_branch(
            "main",
            &yi_session::EntryQuery {
                order: yi_session::EntryOrder::OldestFirst,
                ..yi_session::EntryQuery::default()
            },
            &yi_session::BranchBounds::default(),
        )
        .unwrap_or_default();
    tape(&entries)
}
