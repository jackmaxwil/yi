use serde::{Deserialize, Serialize};

/// A session over wall time: when the model and the tools held it, and its marks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tape {
    pub start: u64,
    pub end: u64,
    pub model: Vec<[u64; 2]>,
    pub tools: Vec<[u64; 2]>,
    pub marks: Vec<Mark>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub at: u64,
    pub kind: MarkKind,
    pub entry: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkKind {
    /// A turn the user typed; rewinding to it forks the conversation there.
    User,
    Checkpoint,
    Failed,
    Compaction,
    #[serde(untagged)]
    Other(String),
}
