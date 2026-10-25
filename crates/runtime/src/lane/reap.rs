//! The only two wipes of a slot's tree, a reap and a discard, and the count of what each drops.

use std::path::Path;

use super::{ClaimBase, Head, LaneError, Pool, STATUS, SlotIndex, git, git_capture, head};

/// Uncommitted paths one status read named; past the capture cap the count is a floor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Uncommitted {
    paths: usize,
    cut: Option<(usize, usize)>,
}

impl Uncommitted {
    pub(crate) fn read(tree: &Path) -> Result<Self, LaneError> {
        let status = git_capture(tree, &STATUS)?;
        let (rows, cut) = (status.stdout.lines().count(), status.cut);
        // Invariant: a cut list is a head and a tail around a marker row; three lines may be no path.
        let paths = rows.saturating_sub(if cut.is_some() { 3 } else { 0 });
        Ok(Self { paths, cut })
    }

    pub fn is_empty(&self) -> bool {
        self.paths == 0 && self.cut.is_none()
    }

    pub(crate) fn discarded(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!("; {self} discarded")
    }
}

impl std::fmt::Display for Uncommitted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.cut, self.paths) {
            (Some((kept, total)), n) => write!(
                formatter,
                "at least {n} uncommitted paths (status read {kept} of {total} bytes, OUTPUT_CAP {})",
                yi_tools::OUTPUT_CAP
            ),
            (None, 1) => formatter.write_str("1 uncommitted path"),
            (None, n) => write!(formatter, "{n} uncommitted paths"),
        }
    }
}

impl Pool {
    /// Incident: three lanes left by dead `pid-` drives held nothing and still filled the pool.
    /// An orphan with a clean tree and a branch `main` already contains has nothing to lose.
    pub(super) fn abandoned(&self, slot: SlotIndex) -> Result<bool, LaneError> {
        let path = self.slot_path(slot);
        if self.dirty(slot) {
            return Ok(false);
        }
        // Invariant: an unreadable HEAD is not a free slot; only a detached one is.
        let branch = match head(&path) {
            Ok(Head::Detached(_)) => return Ok(true),
            Ok(Head::Branch(branch)) => branch,
            Err(_) => return Ok(false),
        };
        let branch = branch.as_str();
        let base = self.resolve(&ClaimBase::Main, false)?;
        Ok(git(&self.repo, &["merge-base", "--is-ancestor", branch, &base]).is_ok())
    }

    /// Invariant: an orphan's branch is the only copy of its work; it goes only once `main` has it.
    pub fn reap(&self, slot: SlotIndex) -> Result<String, LaneError> {
        self.reap_if(slot, None)
    }

    /// The prompt's answer: the slot is freed only if the session named at the
    /// prompt is still the one on it, so an answer never lands on a slot that moved.
    pub fn reap_left_by(&self, slot: SlotIndex, session: &str) -> Result<String, LaneError> {
        self.reap_if(slot, Some(session))
    }

    fn reap_if(&self, slot: SlotIndex, expected: Option<&str>) -> Result<String, LaneError> {
        let _pool = self.lock()?;
        let (guard, holder) = self.probe(slot)?;
        if let Some(session) = holder {
            return Err(LaneError::SlotBusy { slot, session });
        }
        if let Some(expected) = expected {
            let found = self.read_state(slot)?.session.unwrap_or_default();
            if found != expected {
                return Err(LaneError::SlotChanged {
                    slot,
                    expected: expected.to_owned(),
                    found,
                });
            }
        }
        let branch = self.branch_of(slot);
        let kept = self.detach(slot, branch.as_deref())?;
        let lost = self.wipe(slot)?.discarded();
        let mut state = self.read_state(slot)?;
        state.session = None;
        self.write_state(slot, &state)?;
        drop(guard);
        Ok(match (branch, kept) {
            (Some(branch), true) => {
                format!("lane {slot} freed{lost}; branch {branch} kept (unmerged)")
            }
            (Some(branch), false) => {
                format!("lane {slot} freed{lost}; branch {branch} deleted (merged)")
            }
            (None, _) => format!("lane {slot} freed{lost}"),
        })
    }

    /// Whether `git status` names a path in a slot's tree; an unreadable status counts as dirty.
    pub(super) fn dirty(&self, slot: SlotIndex) -> bool {
        !Uncommitted::read(&self.slot_path(slot)).is_ok_and(|tree| tree.is_empty())
    }

    /// Invariant: the only wipe of a session's work; a claim checks out only over a clean tree.
    pub(super) fn wipe(&self, slot: SlotIndex) -> Result<Uncommitted, LaneError> {
        let path = self.slot_path(slot);
        let lost = Uncommitted::read(&path)?;
        if !lost.is_empty() {
            git(&path, &["checkout", "-q", "-f"])?;
            git(&path, &["clean", "-qfd"])?;
        }
        Ok(lost)
    }
}
