//! Lane shapes: a pooled worktree slot's state on disk and the landing event.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A forge pull request number; the identity a landing is polled and merged by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrNumber(pub u32);

impl std::fmt::Display for PrNumber {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "#{}", self.0)
    }
}

/// `<pool>/<n>.json`: what a slot remembers between the processes that use it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotState {
    /// The commit the tree was last reset to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Hash of the toolchain lockfile the slot last synced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockfile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm: Option<WarmReceipt>,
    /// The session id holding the slot; `None` once released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The warmer's receipt: which base and lockfile it built, and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarmReceipt {
    pub base: String,
    pub lockfile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// `None` while the warmer runs or after a claim killed it mid-run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub started_ms: u64,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `<pool>/seen.json`: lockfile hashes a session synced on claim, the only ones
/// the idle warmer may build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenLockfiles {
    #[serde(default)]
    pub hashes: Vec<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Green,
    Red,
    #[serde(untagged)]
    Other(String),
}

impl JobState {
    pub fn glyph(&self) -> &'static str {
        match self {
            Self::Queued => "○",
            Self::Running => "⟳",
            Self::Green => "●",
            Self::Red => "✗",
            Self::Other(_) => "?",
        }
    }
}

/// One gate job as the forge reports it; the name is bounded by the parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LandingJob {
    pub name: String,
    pub state: JobState,
}

/// Where a lane's work is on its way to `main`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Landing {
    Unlanded,
    Pushed {
        branch: String,
    },
    Open {
        pr: PrNumber,
        jobs: Vec<LandingJob>,
        /// Commits `main` gained under the branch since it was pushed.
        behind: u32,
    },
    Merged {
        pr: PrNumber,
    },
}

/// `lanes` in config: worktree-first sessions, the slot count, and the land command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanesConfig {
    pub enabled: Option<bool>,
    pub slots: Option<u8>,
    /// A program plus arguments run in the lane with the title appended, in place of the
    /// built-in push-and-open; a repo whose landing has its own verbs names them here.
    pub land: Option<Vec<String>>,
}

/// What a lane's branch would land: the diff against its merge base with main.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchDiff {
    pub base: String,
    pub files: Vec<(String, u64, u64)>,
    pub patch: String,
    pub untracked: usize,
}
