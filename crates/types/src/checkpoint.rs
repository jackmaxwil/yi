use serde::{Deserialize, Serialize};

/// The `custom{checkpoint}` entry payload (design §7.7): one shadow-gitdir
/// tree id and the moment it was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointData {
    pub tree: String,
    pub at: CheckpointAt,
    /// On `Undo` only: the tree the restore left, so a redo moves only what the undo moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// `Undo` marks the state a restore replaced, which is what makes `yi undo`
/// itself undoable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckpointAt {
    TurnStart,
    TurnEnd,
    Undo,
    #[serde(untagged)]
    Other(String),
}

pub const CHECKPOINT_ENTRY_TYPE: &str = "checkpoint";
