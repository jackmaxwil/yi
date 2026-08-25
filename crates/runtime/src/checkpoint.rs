use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_tools::{Change, Checkpoints};
use yi_types::checkpoint::{CHECKPOINT_ENTRY_TYPE, CheckpointAt, CheckpointData};
use yi_types::entry::Entry;

use crate::session::AgentSession;

pub fn checkpoint_root(home: &Path) -> PathBuf {
    home.join(".yi/checkpoints")
}

/// Design 5.3: checkpoints are best-effort. Without git there is no shadow
/// gitdir and no turn-start capture; every other surface is unaffected.
pub fn wire_turn_checkpoints(session: &AgentSession, home: &Path, cwd: &Path) {
    let Ok(checkpoints) = Checkpoints::open(&checkpoint_root(home), cwd) else {
        return;
    };
    let checkpoints = Arc::new(checkpoints);
    let store = session.store_handle();
    session.set_turn_start_hook(Arc::new(move || {
        let Some(store) = store() else {
            return;
        };
        let Ok(tree) = checkpoints.capture() else {
            return;
        };
        let _capture_failure_never_fails_a_turn =
            append_checkpoint(&store, &tree, CheckpointAt::TurnStart);
    }));
}

fn append_checkpoint(
    store: &yi_session::SharedSession,
    tree: &yi_tools::TreeId,
    at: CheckpointAt,
) -> Result<(), String> {
    let data = CheckpointData {
        tree: tree.as_str().to_owned(),
        at,
    };
    let payload = serde_json::to_value(&data).map_err(|error| error.to_string())?;
    yi_session::lock_session(store)
        .append_custom("main", CHECKPOINT_ENTRY_TYPE, Some(payload))
        .map(|_id| ())
        .map_err(|error| error.to_string())
}

/// Restores the tree the newest checkpoint holds, recording the replaced state
/// as its own checkpoint first — that is what makes undo undoable (design 5.3).
pub fn undo(store: &yi_session::SharedSession, project: &Path, home: &Path) -> UndoOutcome {
    let Some(data) = newest_checkpoint(store) else {
        return UndoOutcome::NoCheckpoint;
    };
    let checkpoints = match Checkpoints::open(&checkpoint_root(home), project) {
        Ok(checkpoints) => checkpoints,
        Err(error) => return UndoOutcome::Failed(error.to_string()),
    };
    let replaced = match checkpoints.capture() {
        Ok(tree) => tree,
        Err(error) => return UndoOutcome::Failed(error.to_string()),
    };
    let restored = match checkpoints.restore(&yi_tools::TreeId::new(data.tree)) {
        Ok(changes) => changes,
        Err(error) => return UndoOutcome::Failed(error.to_string()),
    };
    if let Err(error) = append_checkpoint(store, &replaced, CheckpointAt::Undo) {
        return UndoOutcome::Failed(error);
    }
    UndoOutcome::Restored(restored)
}

pub enum UndoOutcome {
    Restored(Vec<Change>),
    NoCheckpoint,
    Failed(String),
}

fn newest_checkpoint(store: &yi_session::SharedSession) -> Option<CheckpointData> {
    let session = yi_session::lock_session(store);
    let entries = session
        .find_entries_on_branch(
            "main",
            &yi_session::EntryQuery {
                custom_type: Some(CHECKPOINT_ENTRY_TYPE.to_owned()),
                order: yi_session::EntryOrder::NewestFirst,
                limit: Some(1),
                ..yi_session::EntryQuery::default()
            },
            &yi_session::BranchBounds::default(),
        )
        .ok()?;
    let Entry::Custom { data, .. } = entries.into_iter().next()? else {
        return None;
    };
    serde_json::from_value(data?).ok()
}
