use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;
use yi_types::reclaim::{ANNOTATION_ENTRY_TYPE, AnnotationRecord, Reclaimed};

use crate::reclaim::{Cut, brief, calls, hash};

const NOTE_CAP: usize = 280;
const KINDS: [&str; 3] = ["pin", "discard", "finding"];

/// The model's marks on its own history: read and written through the session store, since the
/// session's message list catches up only when a run ends.
pub(crate) struct AnnotateTool {
    store: crate::goal::StoreHandle,
    cut: Arc<Mutex<Cut>>,
}

impl AnnotateTool {
    pub(crate) fn new(store: crate::goal::StoreHandle, cut: Arc<Mutex<Cut>>) -> Self {
        Self { store, cut }
    }
}

fn text<'a>(input: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

struct Target {
    item: Reclaimed,
    call: String,
    result: AgentMessage,
    /// Why no cut ever takes it, whatever a discard says.
    kept: Option<&'static str>,
}

/// The newest stored tool result whose call contains `wanted`: as a placeholder names it, or
/// any of its arguments in full, since a brief shows one, cut at 80 chars.
fn target(entries: &[Entry], wanted: &str) -> Result<Target, String> {
    let stored: Vec<(&str, &AgentMessage)> = (entries.iter())
        .filter_map(|entry| match entry {
            Entry::Message { id, message, .. } => Some((id.as_str(), message)),
            _ => None,
        })
        .collect();
    let messages: Vec<AgentMessage> = stored
        .iter()
        .map(|(_, message)| (*message).clone())
        .collect();
    let pairs = calls(&messages);
    let full = |name: &str, args: &Map<String, Value>| {
        let values = args.values().filter_map(Value::as_str);
        std::iter::once(name)
            .chain(values)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let named: Vec<(usize, String, String)> = (pairs.iter().enumerate())
        .filter_map(|(index, call)| {
            call.map(|(name, args)| (index, brief(name, args), full(name, args)))
        })
        .collect();
    // A placeholder quotes the argument in backticks; a copy without them matches in full.
    let found =
        (named.iter().rev()).find(|(_, call, all)| call.contains(wanted) || all.contains(wanted));
    let Some((index, call, _)) = found else {
        let recent: Vec<&str> = named
            .iter()
            .rev()
            .take(5)
            .map(|(_, call, _)| call.as_str())
            .collect();
        return Err(format!(
            "no stored tool result's call contains `{wanted}`; the newest {} of {}: {}",
            recent.len(),
            named.len(),
            recent.join(", ")
        ));
    };
    let Some((
        entry,
        result @ AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            ..
        },
    )) = stored.get(*index)
    else {
        return Err(format!("`{wanted}` names no tool result"));
    };
    let path = |index: usize| {
        pairs
            .get(index)
            .copied()
            .flatten()
            .filter(|(name, _)| *name == "read")
            .and_then(|(_, args)| args.get("path")?.as_str())
    };
    let reread = path(*index)
        .is_some_and(|read| (index + 1..pairs.len()).any(|later| path(later) == Some(read)));
    let kept = if crate::reclaim::NEVER.contains(&tool_name.as_str()) {
        Some(
            "plan, todo and ask_user results carry live state or the user's words and are never cut",
        )
    } else if !content
        .iter()
        .all(|part| matches!(part, yi_types::message::Content::Text { .. }))
    {
        Some("a result with an image is never cut, since its placeholder could not say so")
    } else if tool_name == "read" && path(*index).is_some() && !reread {
        Some("the newest read of a file is never cut, since an edit needs its line tags")
    } else {
        None
    };
    Ok(Target {
        item: Reclaimed {
            tool_call_id: tool_call_id.clone(),
            hash: hash(content),
            entry_id: Some((*entry).to_owned()),
        },
        call: call.clone(),
        result: (*result).clone(),
        kept,
    })
}

/// Every finding on the branch, oldest first, as the compaction view lists them.
pub(crate) fn findings(entries: &[Entry]) -> Vec<String> {
    let records = entries.iter().filter_map(|entry| match entry {
        Entry::Custom {
            custom_type,
            data: Some(data),
            ..
        } if custom_type == ANNOTATION_ENTRY_TYPE => {
            serde_json::from_value::<AnnotationRecord>(data.clone()).ok()
        }
        _ => None,
    });
    let found = records.filter(|record| record.kind == "finding");
    (found.filter_map(|record| {
        let on = record
            .call
            .map(|call| format!(" (on {call})"))
            .unwrap_or_default();
        record.note.map(|note| format!("{note}{on}"))
    }))
    .collect()
}

impl Tool for AnnotateTool {
    fn name(&self) -> &str {
        "annotate"
    }

    fn description(&self) -> &str {
        "Mark an earlier tool result, named by part of its call (`call`: e.g. `read src/lib.rs`; the newest match): `pin` keeps it whole in your view however old, `discard` lets Yi drop it from your view at the next cut (the stored result stays). Or record a `finding` (`note`, at most 280 chars) that compaction keeps."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": KINDS},
                "call": {"type": "string", "description": "pin, discard, optional on finding: part of the earlier call"},
                "note": {"type": "string", "description": "finding: the fact, at most 280 chars"}
            },
            "required": ["kind"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn validate(&self, input: &Map<String, Value>) -> Result<(), String> {
        let kind = text(input, "kind").unwrap_or_default();
        if !KINDS.contains(&kind) {
            return Err(format!("annotate `kind` is one of {}", KINDS.join(", ")));
        }
        let note = text(input, "note").map_or(0, |note| note.chars().count());
        if note > NOTE_CAP {
            return Err(format!(
                "annotate `note` is {note} chars; the cap is {NOTE_CAP}"
            ));
        }
        match (kind, text(input, "call"), note) {
            ("finding", _, 0) => Err("a finding needs a `note`".to_owned()),
            ("pin" | "discard", None, _) => {
                Err(format!("{kind} needs `call`, part of the earlier call"))
            }
            _ => Ok(()),
        }
    }

    fn execute(&self, input: Map<String, Value>, _context: &ToolContext) -> ToolOutput {
        if let Err(refusal) = self.validate(&input) {
            return error_output(refusal);
        }
        let Some(store) = (self.store)() else {
            return error_output("annotate needs a session store, and this session has none");
        };
        let kind = text(&input, "kind").unwrap_or_default().to_owned();
        let found = match text(&input, "call") {
            Some(wanted) => {
                let query = yi_session::EntryQuery {
                    order: yi_session::EntryOrder::OldestFirst,
                    ..yi_session::EntryQuery::default()
                };
                let bounds = yi_session::BranchBounds::default();
                let entries = (yi_session::lock_session(&store))
                    .find_entries_on_branch("main", &query, &bounds)
                    .unwrap_or_default();
                match target(&entries, wanted) {
                    Ok(found) => Some(found),
                    Err(refusal) => return error_output(refusal),
                }
            }
            None => None,
        };
        if let Some(found) = &found {
            // Invariant: the overlay takes the cut, then the store; never both held here.
            let cut = self
                .cut
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .cut_at(&found.result);
            let refusal = match (kind.as_str(), cut, found.kept) {
                ("finding", _, _) => None,
                (_, Some(turn), _) => Some(format!(
                    "{} was already cut at request {turn}; its placeholder names the read that restores it",
                    found.call
                )),
                ("discard", None, Some(why)) => Some(format!("{} stays: {why}", found.call)),
                _ => None,
            };
            if let Some(refusal) = refusal {
                return error_output(refusal);
            }
        }
        let (target, call) =
            found.map_or((None, None), |found| (Some(found.item), Some(found.call)));
        let record = AnnotationRecord {
            kind: kind.clone(),
            target,
            call: call.clone(),
            note: text(&input, "note").map(str::to_owned),
            extra: Map::new(),
        };
        if let Err(error) = yi_session::lock_session(&store).append_custom_record(&record) {
            return error_output(format!("annotate could not record the mark: {error}"));
        }
        self.cut
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .note(&record);
        let call = call.unwrap_or_default();
        text_output(match kind.as_str() {
            "pin" => format!("Pinned {call}: no cut takes it, however old."),
            "discard" => format!(
                "Discarded {call}: a cut may now take it however young or short; cuts come once dropping pays, and the placeholder names the read that restores it."
            ),
            _ => "Finding recorded: every compaction view lists it.".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Map, Value, json};
    use yi_tools::{Tool, ToolContext};
    use yi_types::message::{AgentMessage, Content, StopReason};
    use yi_types::reclaim::AnnotationRecord;

    use super::{AnnotateTool, findings};

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    fn context() -> ToolContext {
        ToolContext {
            cwd: std::path::PathBuf::from("."),
            cancelled: Arc::new(|| false),
            recovery_dir: None,
            transcript: None,
            auto_background: None,
            sandbox: None,
            deny_read: Vec::new(),
            deny_write: Vec::new(),
            container: None,
            call_id: String::new(),
            job_owner: None,
        }
    }

    /// A store holding a `read src/lib.rs` and a `bash ls`, each answered; the read's entry id.
    fn session() -> Result<(yi_session::SharedSession, String), Box<dyn std::error::Error>> {
        let store = Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
            yi_session::SessionMetadata {
                id: "annotate".to_owned(),
                created_at: 0,
                parent_session_id: None,
                name: None,
            },
        )));
        let mut read = String::new();
        for (id, name, args) in [
            ("c1", "read", json!({"path": "src/lib.rs"})),
            ("c2", "bash", json!({"command": "ls"})),
        ] {
            let call = yi_ai::faux::faux_tool_call(
                id,
                name,
                args.as_object().cloned().unwrap_or_default(),
            );
            let answer = AgentMessage::ToolResult {
                tool_call_id: id.to_owned(),
                tool_name: name.to_owned(),
                content: vec![Content::Text {
                    text: format!("{name} output"),
                    text_signature: None,
                }],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: false,
                timestamp: 0,
            };
            let mut store = yi_session::lock_session(&store);
            store.append_message(
                "main",
                yi_ai::faux::faux_assistant_message(vec![call], StopReason::ToolUse),
            )?;
            let entry = store.append_message("main", answer)?;
            if name == "read" {
                read = entry;
            }
        }
        Ok((store, read))
    }

    fn call(tool: &AnnotateTool, input: Value) -> (String, bool) {
        let input: Map<String, Value> = input.as_object().cloned().unwrap_or_default();
        let output = tool.execute(input, &context());
        let text = (output.result.content.iter())
            .filter_map(|part| match part {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        (text, output.is_error)
    }

    #[test]
    fn a_pin_names_the_newest_matching_result_and_its_entry() -> Fallible {
        let (store, _) = session()?;
        let reread = AgentMessage::ToolResult {
            tool_call_id: "c3".to_owned(),
            tool_name: "read".to_owned(),
            content: vec![Content::Text {
                text: "read output, edited".to_owned(),
                text_signature: None,
            }],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        };
        let args = json!({"path": "src/lib.rs"})
            .as_object()
            .cloned()
            .unwrap_or_default();
        let asked = yi_ai::faux::faux_tool_call("c3", "read", args);
        let read = {
            let mut store = yi_session::lock_session(&store);
            store.append_message(
                "main",
                yi_ai::faux::faux_assistant_message(vec![asked], StopReason::ToolUse),
            )?;
            store.append_message("main", reread)?
        };
        let held = Arc::clone(&store);
        let tool = AnnotateTool::new(Arc::new(move || Some(Arc::clone(&held))), Arc::default());
        let (text, failed) = call(&tool, json!({"kind": "pin", "call": "read src/lib.rs"}));
        assert!(
            !failed && text.starts_with("Pinned read `src/lib.rs`"),
            "{text}"
        );
        let records: Vec<AnnotationRecord> = yi_session::lock_session(&store)
            .custom_records(yi_session::EntryOrder::OldestFirst, None);
        let target = records
            .first()
            .and_then(|record| record.target.as_ref())
            .ok_or("no target")?;
        assert_eq!(
            (target.tool_call_id.as_str(), target.entry_id.as_deref()),
            ("c3", Some(read.as_str()))
        );
        Ok(())
    }

    /// A mark that names nothing says what the newest calls were, so the next try can match.
    #[test]
    fn a_call_that_matches_nothing_lists_the_newest_calls() -> Fallible {
        let (store, _) = session()?;
        let tool = AnnotateTool::new(Arc::new(move || Some(Arc::clone(&store))), Arc::default());
        let (text, failed) = call(&tool, json!({"kind": "discard", "call": "cargo test"}));
        assert!(failed, "{text}");
        assert!(
            text.ends_with("the newest 2 of 2: bash `ls`, read `src/lib.rs`"),
            "{text}"
        );
        Ok(())
    }

    /// The note cap, at the limit and one past it; a recorded finding reaches the compaction view.
    #[test]
    fn a_finding_at_the_cap_is_kept_and_one_past_it_is_refused() -> Fallible {
        let (store, _) = session()?;
        let held = Arc::clone(&store);
        let tool = AnnotateTool::new(Arc::new(move || Some(Arc::clone(&held))), Arc::default());
        let (text, failed) = call(&tool, json!({"kind": "finding", "note": "é".repeat(281)}));
        assert!(
            failed && text.contains("is 281 chars; the cap is 280"),
            "{text}"
        );
        let (text, failed) = call(
            &tool,
            json!({"kind": "finding", "note": "é".repeat(280), "call": "ls"}),
        );
        assert!(!failed, "{text}");
        let entries =
            yi_session::lock_session(&store).find_entries(&yi_session::EntryQuery::default())?;
        assert_eq!(
            findings(&entries),
            [format!("{} (on bash `ls`)", "é".repeat(280))]
        );
        Ok(())
    }

    /// Review of #1151: a discard no cut could ever honour was acknowledged anyway; a long
    /// command matched only through its brief, which stops at 80 chars.
    #[test]
    fn a_discard_no_cut_could_take_is_refused_and_a_long_call_matches_in_full() -> Fallible {
        let (store, _) = session()?;
        let long = format!(
            "cargo test -p yi-runtime --test integration -- {}::the_case",
            "deep".repeat(20)
        );
        let asked = yi_ai::faux::faux_tool_call(
            "c4",
            "bash",
            json!({"command": long})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        );
        let answer = AgentMessage::ToolResult {
            tool_call_id: "c4".to_owned(),
            tool_name: "bash".to_owned(),
            content: vec![Content::Text {
                text: "ok".to_owned(),
                text_signature: None,
            }],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        };
        {
            let mut store = yi_session::lock_session(&store);
            store.append_message(
                "main",
                yi_ai::faux::faux_assistant_message(vec![asked], StopReason::ToolUse),
            )?;
            store.append_message("main", answer)?;
        }
        let tool = AnnotateTool::new(Arc::new(move || Some(Arc::clone(&store))), Arc::default());
        let (text, failed) = call(&tool, json!({"kind": "discard", "call": "read src/lib.rs"}));
        assert!(
            failed && text.contains("the newest read of a file is never cut"),
            "{text}"
        );
        let (text, failed) = call(
            &tool,
            json!({"kind": "discard", "call": "bash `cargo test"}),
        );
        assert!(!failed, "a placeholder's own quoting matches: {text}");
        let (text, failed) = call(&tool, json!({"kind": "discard", "call": "::the_case"}));
        assert!(
            !failed && text.starts_with("Discarded bash `cargo test"),
            "{text}"
        );
        Ok(())
    }
}
