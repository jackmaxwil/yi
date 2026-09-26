//! What a child holds from its parent: a deadline and tokens drawn at spawn, never minted,
//! and the records a revocation leaves behind (D215).
use serde::{Deserialize, Serialize};

use crate::url::Url;

/// `deadline_ms` is wall-clock epoch milliseconds; absent under a parent that has no clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub holder: String,
    pub parent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    pub granted_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked: Option<Revocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revocation {
    pub at: u64,
    pub grace_ms: u64,
    pub reason: String,
}

impl Revocation {
    pub fn due(&self) -> u64 {
        self.at.saturating_add(self.grace_ms)
    }
}

/// How a repossession left the child's work: settled on the references it names, or still
/// pending because a stop, a settle or this record's own write failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Disposition {
    Settled,
    Pending { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repossession {
    pub lease: Lease,
    pub at: u64,
    pub kept: Vec<Url>,
    pub disposition: Disposition,
}

/// What a lease leaves when its holder is reaped. `unspent` is absent when a turn reported no
/// usage: an unknown spend returns nothing rather than a guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Returned {
    pub lease: Lease,
    pub spent: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unspent: Option<u64>,
}

/// One line of the lease journal, kept on the parent's own transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LeaseRecord {
    Revoked(Lease),
    Repossessed(Repossession),
    Returned(Returned),
}

pub const DEFAULT_GRACE_MS: u64 = 30_000;

/// What happens to a running child when its parent closes. There is no `Abandon`: nothing
/// owns an orphan's address, budget, inbox, artifacts and deadline until a supervisor does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum ParentClose {
    Terminate {
        #[serde(default = "default_grace")]
        grace_ms: u64,
    },
    RequestCancel,
}

fn default_grace() -> u64 {
    DEFAULT_GRACE_MS
}

impl Default for ParentClose {
    fn default() -> Self {
        Self::Terminate {
            grace_ms: DEFAULT_GRACE_MS,
        }
    }
}

impl ParentClose {
    pub fn grace_ms(self) -> u64 {
        match self {
            Self::Terminate { grace_ms } => grace_ms,
            Self::RequestCancel => DEFAULT_GRACE_MS,
        }
    }
}
