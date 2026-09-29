use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChildId(pub String);

impl ChildId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildStatus {
    Running,
    Completed,
    Error,
}

impl ChildStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Error => "error",
        }
    }
}

/// Why a run failed, as a closed set a client can branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailClass {
    RefusedSpawn,
    Provider,
    KernelDeath,
    RedCheck,
    Deadline,
    /// A class a newer host named; the ending it rides on still reads as a failure.
    #[serde(other)]
    Other,
}

/// How a child's run ended. The wire status, the member state and the parent's notice are
/// all read from this one value, so no two surfaces tell different stories of one exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChildExit {
    Completed,
    Failed {
        class: FailClass,
    },
    Interrupted,
    Reaped,
    Repossessed,
    /// A kind a newer host named, decoded rather than failing the update that carries it.
    #[serde(other)]
    Other,
}

/// The loop's own word that it re-drove a run, carried as `signal` beside a record's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopSignal {
    RepeatBreak,
    LengthRedrive,
    LetGo,
}

/// What the child is doing right now, as opposed to how its run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildActivity {
    Waiting,
    Writing,
    Executing,
}

/// A finding the child made outside its own task. `violates_check_of` names an ancestor task
/// whose check the runtime re-runs, so a child cannot declare its own finding critical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub violates_check_of: Option<crate::plan::TaskId>,
    pub fingerprint: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The answer a checked child owes its parent. `discoveries` is required and may be empty:
/// an absent field does not decode, keeping a malformed result fatal instead of a null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildResult {
    pub value: serde_json::Value,
    pub discoveries: Vec<Discovery>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Design §11: the one typed status surface for a running child, so a client reads counts and
/// activity instead of re-deriving them from the child's event stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildUpdate {
    pub id: ChildId,
    pub name: String,
    pub status: ChildStatus,
    pub activity: ChildActivity,
    pub tool_use_count: u64,
    pub token_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Absent while the child runs, and from a host older than the typed exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<ChildExit>,
    /// Absent unless a live child waits on its parent or has stalled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<ChildFlag>,
}

/// The family state a live child's status alone does not say. `note` is `rlm.status`'s own:
/// `asks <request id>: <question>` for a question, the stall's reason for `stuck`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildFlag {
    NeedsYou {
        note: String,
    },
    Stuck {
        note: String,
    },
    /// A state a newer host named, decoded rather than failing the update that carries it.
    #[serde(other)]
    Other,
}

/// The custom entry type of a parent's child trail.
pub const CHILD_ENTRY: &str = "child";

/// One line of the trail a parent keeps on its own transcript: where each child's transcript
/// lives and how its run ended. `path` is relative to the directory of the parent's file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChildTrail {
    Spawned(ChildSpawned),
    Ended(ChildEnded),
    /// A line a newer host wrote; the scan that finds older children reads past it.
    #[serde(other)]
    Other,
}

/// `brief` is the SHA-256 of the prompt the child was spawned with, hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSpawned {
    pub name: String,
    pub id: ChildId,
    pub session: String,
    pub path: String,
    pub brief: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildEnded {
    pub name: String,
    pub id: ChildId,
    pub exit: ChildExit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub tokens: u64,
}
