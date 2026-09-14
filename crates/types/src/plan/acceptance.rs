//! Worktree acceptance shapes (plan section 6.6): the quiescence a settle read, a merge's
//! conflicts, how a publication landed, and the disposition every other exit records first.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::url::Url;

/// What the child was doing when its lane was read (journal key `quiescent`): a settle
/// refuses while any command under the lane still runs, naming it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quiescence {
    pub at: u64,
    /// The commands still running, by their text; empty is quiet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<String>,
}

impl Quiescence {
    pub fn is_quiet(&self) -> bool {
        self.running.is_empty()
    }

    pub fn running_commands(&self) -> usize {
        self.running.len()
    }
}

/// A path the prepared merge had to choose at, which side won, and the base it chose against.
/// One shape on the acceptance record and inside `merge_failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    pub side: String,
    pub base: String,
}

/// How a publication reached the parent: its checkout fast-forwarded, or only its branch ref
/// moved because the user's uncommitted work overlaps the integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Published {
    FastForward,
    RefOnly,
}

/// How a worktree todo left `Running` without an acceptance, recorded before the lane and
/// slot are released; `branch` and `candidate` are absent when the host held no lane by then.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Retained {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        candidate: Option<String>,
        kept: Vec<Url>,
        reason: String,
    },
    Discarded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        candidate: Option<String>,
        kept: Vec<Url>,
        reason: String,
    },
    MergeFailed {
        branch: String,
        candidate: String,
        parent_base: String,
        generation: u64,
        conflict_provenance: Vec<Conflict>,
    },
    RepossessionPending {
        branch: String,
        candidate: String,
        kept: Vec<Url>,
        lease: Value,
        at: u64,
        detail: String,
    },
}

impl Disposition {
    /// The journal's `op` for every member.
    pub const KIND: &'static str = "disposition";

    pub fn kept(&self) -> &[Url] {
        match self {
            Self::Retained { kept, .. }
            | Self::Discarded { kept, .. }
            | Self::RepossessionPending { kept, .. } => kept,
            Self::MergeFailed { .. } => &[],
        }
    }

    pub fn branch(&self) -> Option<&str> {
        match self {
            Self::Retained { branch, .. } | Self::Discarded { branch, .. } => branch.as_deref(),
            Self::MergeFailed { branch, .. } | Self::RepossessionPending { branch, .. } => {
                Some(branch)
            }
        }
    }

    /// Whether this disposition frees the lane slot: a merge failure keeps it for the resolved
    /// candidate's next submit, a pending repossession until the join returns.
    pub fn releases_slot(&self) -> bool {
        !matches!(
            self,
            Self::RepossessionPending { .. } | Self::MergeFailed { .. }
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Retained { .. } => "retained",
            Self::Discarded { .. } => "discarded",
            Self::MergeFailed { .. } => "merge_failed",
            Self::RepossessionPending { .. } => "repossession_pending",
        }
    }
}
