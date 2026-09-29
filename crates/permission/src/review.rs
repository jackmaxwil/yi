use std::path::PathBuf;

use sha2::{Digest, Sha256};
use yi_types::permission::RuleKind;

/// How many actions the ledger remembers. Eviction is by generation, so a long
/// session forgets its oldest answers rather than refusing new ones.
pub const LEDGER_CAP: usize = 256;

/// Invariant: the digest of the same canonical string the session rules key on, so a mutated
/// argument, path, command or cwd is a different action and cannot reuse its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionId([u8; 32]);

impl ActionId {
    #[must_use]
    pub fn of(canonical: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        Self(hasher.finalize().into())
    }
}

/// Monotonic within one session; what a denial tells the model to quote back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionState {
    /// The reviewer refused; nobody has put it to the user yet.
    DeniedPendingUser,
    /// The user said yes once. Consumed by the next identical call.
    UserApproved,
    UserDenied,
}

/// Everything `ask_user` needs to replay one denied call to the human, kept
/// beside the action id the answer binds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedAsk {
    pub title: String,
    pub description: String,
    pub patch: Option<String>,
    pub targets: Vec<PathBuf>,
    pub display: String,
    pub canonical: String,
    pub kind: RuleKind,
    pub grants: Vec<crate::Grant>,
    /// The reviewer's own words. Stored because a re-issue has to quote the
    /// same refusal the first denial did, not the policy line underneath it.
    pub evidence: String,
}

struct Entry {
    action: ActionId,
    request: RequestId,
    state: ActionState,
    generation: u64,
    ask: ReviewedAsk,
}

/// Once-grade memory for actions an auto reviewer refused: what was asked, who
/// answered, and which action the answer is bound to.
#[derive(Default)]
pub struct ActionLedger {
    entries: Vec<Entry>,
    next_request: u64,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserVerdict {
    Approved,
    Denied,
}

impl ActionLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn state_of(&self, action: ActionId) -> Option<(RequestId, ActionState)> {
        self.entries
            .iter()
            .find(|entry| entry.action == action)
            .map(|entry| (entry.request, entry.state))
    }

    /// Idempotent by action: an identical denied call returns the request that exists, so a
    /// retry loop cannot spend a second review or open a second question.
    pub fn open(&mut self, action: ActionId, ask: ReviewedAsk) -> RequestId {
        if let Some(entry) = self.entries.iter().find(|entry| entry.action == action) {
            return entry.request;
        }
        self.generation = self.generation.saturating_add(1);
        self.next_request = self.next_request.saturating_add(1);
        let request = RequestId(self.next_request);
        if self.entries.len() >= LEDGER_CAP {
            self.evict_oldest();
        }
        self.entries.push(Entry {
            action,
            request,
            state: ActionState::DeniedPendingUser,
            generation: self.generation,
            ask,
        });
        request
    }

    fn evict_oldest(&mut self) {
        if let Some(position) = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.generation)
            .map(|(position, _)| position)
        {
            self.entries.remove(position);
        }
    }

    #[must_use]
    pub fn ask_of(&self, request: RequestId) -> Option<&ReviewedAsk> {
        self.entry_of(request).map(|(ask, _)| ask)
    }

    #[must_use]
    pub fn entry_of(&self, request: RequestId) -> Option<(&ReviewedAsk, ActionState)> {
        self.entries
            .iter()
            .find(|entry| entry.request == request)
            .map(|entry| (&entry.ask, entry.state))
    }

    pub fn resolve(&mut self, request: RequestId, verdict: UserVerdict) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.request == request)
        else {
            return false;
        };
        entry.state = match verdict {
            UserVerdict::Approved => ActionState::UserApproved,
            UserVerdict::Denied => ActionState::UserDenied,
        };
        true
    }

    pub fn forget(&mut self, action: ActionId) {
        self.entries.retain(|entry| entry.action != action);
    }

    /// Single use: an approval is spent by the first identical call, so one yes never becomes
    /// a standing grant. `allow always` goes to the session rules, where those belong.
    pub fn take_approval(&mut self, action: ActionId) -> bool {
        let Some(position) = self
            .entries
            .iter()
            .position(|entry| entry.action == action && entry.state == ActionState::UserApproved)
        else {
            return false;
        };
        self.entries.remove(position);
        true
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
