use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_tools::{Change, ChangeKind, Checkpoints, TreeId};
use yi_types::checkpoint::{CHECKPOINT_ENTRY_TYPE, CheckpointAt, CheckpointData};
use yi_types::entry::Entry;

use crate::session::AgentSession;

pub fn checkpoint_root(home: &Path) -> PathBuf {
    home.join(".yi/checkpoints")
}

struct Shadow {
    root: PathBuf,
    cwd: PathBuf,
    opened: std::sync::OnceLock<Option<Checkpoints>>,
}

impl Shadow {
    fn get(&self) -> Option<&Checkpoints> {
        self.opened
            .get_or_init(|| Checkpoints::open(&self.root, &self.cwd).ok())
            .as_ref()
    }
}

/// Best-effort: without git there is no shadow gitdir and no capture, and nothing else changes.
pub fn wire_turn_checkpoints(session: &AgentSession, home: &Path, cwd: &Path) {
    let shadow = Arc::new(Shadow {
        root: checkpoint_root(home),
        cwd: cwd.to_path_buf(),
        opened: std::sync::OnceLock::new(),
    });
    session.set_turn_start_hook(capture_hook(
        session,
        Arc::clone(&shadow),
        CheckpointAt::TurnStart,
    ));
    session.set_turn_end_hook(capture_hook(session, shadow, CheckpointAt::TurnEnd));
}

fn capture_hook(
    session: &AgentSession,
    shadow: Arc<Shadow>,
    at: CheckpointAt,
) -> Arc<crate::session::TurnHook> {
    let store = session.store_handle();
    Arc::new(move || {
        let Some(store) = store() else {
            return;
        };
        let span = yi_types::trace::span("checkpoint.capture").arg("at", format!("{at:?}"));
        let captured = shadow.get().map(Checkpoints::capture);
        drop(span);
        let Some(Ok(tree)) = captured else {
            return;
        };
        let _capture_failure_never_fails_a_turn =
            append_checkpoint(&store, &tree, at.clone(), None);
    })
}

fn append_checkpoint(
    store: &yi_session::SharedSession,
    tree: &TreeId,
    at: CheckpointAt,
    after: Option<&TreeId>,
) -> Result<(), String> {
    let data = CheckpointData {
        tree: tree.as_str().to_owned(),
        at,
        after: after.map(|tree| tree.as_str().to_owned()),
    };
    let payload = serde_json::to_value(&data).map_err(|error| error.to_string())?;
    yi_session::lock_session(store)
        .append_custom("main", CHECKPOINT_ENTRY_TYPE, Some(payload))
        .map(|_id| ())
        .map_err(|error| error.to_string())
}

pub fn undo(store: &yi_session::SharedSession, project: &Path, home: &Path) -> UndoOutcome {
    let Some((data, since)) = undo_target(store) else {
        return UndoOutcome::NoCheckpoint;
    };
    restore_tree(store, project, home, &data, since, || Ok(()))
}

/// Invariant: the replaced tree is recorded after `rewind` moves the lane, on the branch it lands
/// on; recorded before it, the record is off-branch and the next undo reverts the turn before.
pub fn undo_to(
    store: &yi_session::SharedSession,
    entry_id: &str,
    project: &Path,
    home: &Path,
    rewind: impl FnOnce() -> Result<(), String>,
) -> UndoOutcome {
    let entries = yi_session::lock_session(store)
        .find_entries_on_branch(
            "main",
            &yi_session::EntryQuery {
                order: yi_session::EntryOrder::OldestFirst,
                ..yi_session::EntryQuery::default()
            },
            &yi_session::BranchBounds::default(),
        )
        .unwrap_or_default();
    let Some(at) = entries.iter().position(|entry| entry.id() == entry_id) else {
        return UndoOutcome::NoCheckpoint;
    };
    // The turn-start capture runs beside the first request, so it may land on either side of
    // the prompt; tools wait for it, so any one with no tool result between saw the same files.
    let tool_result = |entry: &&Entry| {
        matches!(
            entry,
            Entry::Message {
                message: yi_types::message::AgentMessage::ToolResult { .. },
                ..
            }
        )
    };
    let turn_start = |entry: &Entry| match entry {
        Entry::Custom {
            custom_type,
            data: Some(data),
            ..
        } if custom_type == CHECKPOINT_ENTRY_TYPE => {
            serde_json::from_value::<CheckpointData>(data.clone())
                .ok()
                .filter(|start| start.at == CheckpointAt::TurnStart)
        }
        _ => None,
    };
    let after = entries.get(at..).unwrap_or_default().iter();
    let before = entries.get(..at).unwrap_or_default().iter().rev();
    let found = after
        .take_while(|entry| !tool_result(entry))
        .find_map(turn_start);
    let Some(start) = found.or_else(|| {
        before
            .take_while(|entry| !tool_result(entry))
            .find_map(turn_start)
    }) else {
        return UndoOutcome::NoCheckpoint;
    };
    // Incident: a conversation-only rewind leaves later turns' files on disk; paired with its
    // branch's turn end they read as the user's edits and stayed. The session's newest wins.
    let session = yi_session::lock_session(store)
        .find_entries(&checkpoint_query())
        .unwrap_or_default();
    let since = parse(session)
        .into_iter()
        .find_map(|recorded| match recorded.data.at {
            CheckpointAt::TurnEnd => Some(recorded.data.tree),
            CheckpointAt::Undo => recorded.data.after,
            CheckpointAt::TurnStart | CheckpointAt::Other(_) => None,
        });
    restore_tree(store, project, home, &start, since, rewind)
}

fn restore_tree(
    store: &yi_session::SharedSession,
    project: &Path,
    home: &Path,
    data: &CheckpointData,
    since: Option<String>,
    rewind: impl FnOnce() -> Result<(), String>,
) -> UndoOutcome {
    let checkpoints = match Checkpoints::open(&checkpoint_root(home), project) {
        Ok(checkpoints) => checkpoints,
        Err(error) => return UndoOutcome::Failed(error.to_string()),
    };
    let replaced = match checkpoints.capture() {
        Ok(tree) => tree,
        Err(error) => return UndoOutcome::Failed(error.to_string()),
    };
    let since = since.map(TreeId::new);
    let restored = match checkpoints.restore(&TreeId::new(data.tree.clone()), since.as_ref()) {
        Ok(changes) => changes,
        Err(error) => return UndoOutcome::Failed(error.to_string()),
    };
    let after = match checkpoints.capture() {
        Ok(tree) => tree,
        Err(error) => return UndoOutcome::Failed(error.to_string()),
    };
    let rewound = rewind();
    if let Err(error) = append_checkpoint(store, &replaced, CheckpointAt::Undo, Some(&after)) {
        return UndoOutcome::Failed(error);
    }
    if let Err(error) = rewound {
        return UndoOutcome::Failed(format!(
            "{}, but the conversation did not rewind: {error}",
            describe_undo(&restored, since.is_some())
        ));
    }
    UndoOutcome::Restored {
        changes: restored,
        scoped: since.is_some(),
    }
}

pub enum UndoOutcome {
    Restored { changes: Vec<Change>, scoped: bool },
    NoCheckpoint,
    Failed(String),
}

/// Every surface names each kept path: a file the restore skipped is invisible otherwise.
pub fn undo_notes(changes: &[Change], scoped: bool) -> Vec<String> {
    let kept = paths(changes, |kind| kind == ChangeKind::Kept);
    let mut notes = Vec::new();
    if !kept.is_empty() {
        notes.push(format!(
            "[kept {} of {} — changed since the turn ended, left as you have it: {}]",
            kept.len(),
            changes.len(),
            kept.join(", ")
        ));
    }
    if !scoped {
        notes.push(
            "[unscoped — no turn-end checkpoint pairs with this one, so every path changed \
             since it moved]"
                .to_owned(),
        );
    }
    notes
}

pub fn describe_undo(changes: &[Change], scoped: bool) -> String {
    let moved = paths(changes, |kind| kind != ChangeKind::Kept);
    let mut parts = vec![match moved.len() {
        0 => "nothing to restore — no file changed since the checkpoint".to_owned(),
        1 => format!("restored 1 file — {}", moved.join(", ")),
        n => format!("restored {n} files — {}", moved.join(", ")),
    }];
    parts.extend(undo_notes(changes, scoped));
    parts.join("; ")
}

fn paths(changes: &[Change], wanted: impl Fn(ChangeKind) -> bool) -> Vec<String> {
    let mut names: Vec<String> = changes
        .iter()
        .filter(|change| wanted(change.kind))
        .map(|change| change.path.display().to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

pub struct RecordedCheckpoint {
    pub data: CheckpointData,
    pub timestamp: u64,
}

fn checkpoint_query() -> yi_session::EntryQuery {
    yi_session::EntryQuery {
        custom_type: Some(CHECKPOINT_ENTRY_TYPE.to_owned()),
        order: yi_session::EntryOrder::NewestFirst,
        ..yi_session::EntryQuery::default()
    }
}

pub fn recorded(store: &yi_session::SharedSession) -> Vec<RecordedCheckpoint> {
    let entries = yi_session::lock_session(store)
        .find_entries_on_branch(
            "main",
            &checkpoint_query(),
            &yi_session::BranchBounds::default(),
        )
        .unwrap_or_default();
    parse(entries)
}

fn parse(entries: Vec<Entry>) -> Vec<RecordedCheckpoint> {
    entries
        .into_iter()
        .filter_map(|entry| {
            let Entry::Custom {
                data, timestamp, ..
            } = entry
            else {
                return None;
            };
            serde_json::from_value(data?)
                .ok()
                .map(|data| RecordedCheckpoint { data, timestamp })
        })
        .collect()
}

/// Walks to the turn's start, or to what an undo replaced (so the second is a redo), paired
/// with the newest turn end after it: a follow-up's start can land before this turn's end.
fn undo_target(store: &yi_session::SharedSession) -> Option<(CheckpointData, Option<String>)> {
    let recorded = recorded(store);
    let index = recorded.iter().position(|recorded| {
        matches!(
            recorded.data.at,
            CheckpointAt::TurnStart | CheckpointAt::Undo
        )
    })?;
    let target = recorded.get(index)?.data.clone();
    let since = match target.at {
        CheckpointAt::Undo => target.after.clone(),
        _ => recorded
            .iter()
            .take(index)
            .find(|recorded| recorded.data.at == CheckpointAt::TurnEnd)
            .map(|recorded| recorded.data.tree.clone()),
    };
    Some((target, since))
}
