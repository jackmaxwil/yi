//! Settling a lane (plan section 6.6): quiescent, committed as the candidate, slot released
//! with the branch kept; plus the staging merge and the generation-checked publication.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{Value, json};
pub use yi_types::plan::acceptance::{Conflict, Published, Quiescence};

pub use super::hostgit::{Candidate, candidate_of};
use super::hostgit::{git_by, git_capped, identity, past, sha_of};
use super::{BranchName, ClaimBase, Head, Lane, LaneError, Pool, Sha, SlotView};

/// Invariant: held from the generation read through publication, so two hand-backs in this
/// process never race on the parent checkout; across processes only the generation check guards.
pub(crate) static INTEGRATION: Mutex<()> = Mutex::new(());

/// The jobs registry's answer for `root`: every backgrounded command still running under
/// the lane path. A child's foreground command has returned by the time it can ask.
pub fn quiescence_of_jobs(root: &Path) -> Quiescence {
    Quiescence {
        at: crate::session_store::now_ms(),
        running: yi_tools::jobs::registry().running_under(root),
    }
}

/// A lane settled: the candidate on its branch, the slot handed back.
#[must_use]
#[derive(Debug)]
pub struct Settled {
    pub candidate: Candidate,
    pub slot: super::SlotIndex,
    pool: Pool,
}

impl Settled {
    /// The `Discarded` path: the branch goes, so the caller has already pinned what it keeps;
    /// one `settle` deleted already (a no-op child's tip is on main) is the outcome asked for.
    pub fn discard(self) -> Result<Candidate, LaneError> {
        if self.pool.branch_exists(&self.candidate.branch)? {
            git_by(
                &self.pool.repo,
                &["branch", "-q", "-D", self.candidate.branch.as_str()],
                None,
            )?;
        }
        Ok(self.candidate)
    }
}

pub(super) const KEPT_YOURS: &str = "Your uncommitted edits to the kept_yours paths stay in the working copy without the child's change to them, so `git diff HEAD` shows that change as removed and committing them would undo it; `git show HEAD:<path>` is the merged version to fold them into; a kept path missing from HEAD is one the child deleted, and it is untracked here.";

/// A settle that did not release: the lane comes back with the error when nothing was
/// detached yet, so the slot stays held; a release that failed midway is `Lost`.
#[derive(Debug)]
pub enum Unsettled {
    Kept(Box<Lane>, LaneError),
    Lost(LaneError),
}

impl Unsettled {
    pub fn error(&self) -> &LaneError {
        match self {
            Self::Kept(_, error) | Self::Lost(error) => error,
        }
    }
}

impl std::fmt::Display for Unsettled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kept(_, error) => write!(formatter, "{error} (the lane is kept)"),
            Self::Lost(error) => write!(formatter, "{error} (the lane was released)"),
        }
    }
}

impl std::error::Error for Unsettled {}

impl Lane {
    /// The candidate this lane holds once quiescent; see [`candidate_of`].
    pub fn candidate(
        &self,
        quiescence: &Quiescence,
        deadline: Option<Instant>,
    ) -> Result<Candidate, LaneError> {
        candidate_of(
            &self.path,
            &self.session,
            &self.branch,
            self.base().and_then(|base| Sha::parse(&base)),
            quiescence,
            deadline,
        )
    }

    /// What a host needs to read the candidate without the lane: its path, session, branch
    /// and base.
    pub fn checkout(&self) -> (PathBuf, String, BranchName, Option<Sha>) {
        (
            self.path.clone(),
            self.session.clone(),
            self.branch.clone(),
            self.base().and_then(|base| Sha::parse(&base)),
        )
    }

    /// Invariant: a refusal before the release hands the lane back (`Unsettled::Kept`), so a
    /// busy or over-deadline settle never frees the slot; `Drop` stays the best-effort detach.
    pub fn settle(
        self,
        quiescence: &Quiescence,
        deadline: Option<Instant>,
    ) -> Result<Settled, Unsettled> {
        let candidate = match self.candidate(quiescence, deadline) {
            Ok(candidate) => candidate,
            Err(error) => return Err(Unsettled::Kept(Box::new(self), error)),
        };
        if let Err(error) = past(deadline) {
            return Err(Unsettled::Kept(Box::new(self), error));
        }
        let slot = self.slot;
        let pool = self.pool.clone();
        self.take_slot(false).map_err(Unsettled::Lost)?;
        Ok(Settled {
            candidate,
            slot,
            pool,
        })
    }
}

/// The parent generation an integration is prepared against: the commit the parent's HEAD
/// names when the merge is prepared, compared again at publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub head: Head,
    pub base: Sha,
}

pub fn generation_of(parent: &Path, deadline: Option<Instant>) -> Result<Generation, LaneError> {
    let head = super::head(parent)?;
    let base = sha_of(&git_by(
        parent,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        deadline,
    )?)?;
    Ok(Generation { head, base })
}

/// A prepared merge in a staging lane from the pool, never the parent's checkout; dropped, it
/// hands the slot back and deletes the branch that keeps `integrated` reachable.
#[must_use]
#[derive(Debug)]
pub struct Staging {
    lane: Option<Lane>,
    pub base: Sha,
    pub candidate: Sha,
    pub integrated: Sha,
}

impl Staging {
    /// A merge in the object store: nothing to hand back, and `integrated` is held by its caller.
    pub(super) fn laneless(base: Sha, candidate: Sha, integrated: Sha) -> Self {
        Self {
            lane: None,
            base,
            candidate,
            integrated,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.lane.as_ref().map(Lane::path)
    }

    pub fn slot(&self) -> Option<super::SlotIndex> {
        self.lane.as_ref().map(Lane::slot)
    }

    pub fn release(mut self) -> Result<(), LaneError> {
        match self.lane.take() {
            Some(lane) => lane.discard().map(drop),
            None => Ok(()),
        }
    }

    /// The slot back with the staging branch kept at `integrated`, the pin until publication;
    /// what the checker wrote into the tree is not the integration and is never committed.
    pub fn keep(mut self) -> Result<Sha, LaneError> {
        let integrated = self.integrated.clone();
        if let Some(lane) = self.lane.take() {
            lane.take_slot(false)?;
        }
        Ok(integrated)
    }
}

/// Delete the staging branches that point at `integrated` once it is published or superseded;
/// `--points-at`, never `--contains`, so another attempt's pin on a shared base is left alone.
pub fn drop_integration_pin(pool: &Pool, integrated: &Sha) -> Result<(), LaneError> {
    let refs = git_by(
        pool.repo(),
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "--points-at",
            integrated.as_str(),
            "refs/heads/yi/stage-*",
        ],
        None,
    )?;
    for branch in refs.lines().map(str::trim).filter(|line| !line.is_empty()) {
        git_by(pool.repo(), &["branch", "-q", "-D", branch], None)?;
    }
    Ok(())
}

/// Frees the slot and deletes the branch of a staging lane whose merge no record names.
/// True when a slot or a branch of `session` was there to drop.
pub fn drop_staging(pool: &Pool, session: &str) -> Result<bool, LaneError> {
    let mut dropped = false;
    for view in pool.list()? {
        if let SlotView::Orphan { slot, holder, .. } = view
            && holder.session == session
        {
            pool.reap_left_by(slot, session)?;
            dropped = true;
        }
    }
    let branch = BranchName::for_session(session)?;
    if pool.branch_exists(&branch)? {
        git_by(pool.repo(), &["branch", "-q", "-D", branch.as_str()], None)?;
        dropped = true;
    }
    Ok(dropped)
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(lane) = self.lane.take() {
            let _best_effort_after_release = lane.discard();
        }
    }
}

/// What preparing a merge found: a merged tree, or the paths it could not choose at.
#[derive(Debug)]
pub enum Prepared {
    Merged(Staging),
    Conflict { base: Sha, conflicts: Vec<Conflict> },
}

fn conflicts_of(
    lane: &Path,
    base: &Sha,
    deadline: Option<Instant>,
) -> Result<Vec<Conflict>, LaneError> {
    let unmerged = git_by(lane, &["diff", "--name-only", "--diff-filter=U"], deadline)?;
    let mut out = Vec::new();
    for path in unmerged
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let stages = git_by(lane, &["ls-files", "-u", "--", path], deadline)?;
        let has = |stage: &str| {
            stages.lines().any(|line| {
                line.split('\t')
                    .next()
                    .is_some_and(|meta| meta.split(' ').nth(2) == Some(stage))
            })
        };
        let side = match (has("2"), has("3")) {
            (true, true) => "both",
            (true, false) => "ours",
            (false, true) => "theirs",
            (false, false) => "unknown",
        };
        out.push(Conflict {
            path: path.to_owned(),
            side: side.to_owned(),
            base: base.as_str().to_owned(),
        });
    }
    Ok(out)
}

/// Prepare `candidate` onto `generation` in a staging lane claimed at the generation's base.
/// A conflict releases the lane and reports its provenance; the candidate is untouched.
pub fn prepare(
    pool: &Pool,
    session: &str,
    generation: &Generation,
    candidate: &Sha,
    deadline: Option<Instant>,
) -> Result<Prepared, LaneError> {
    past(deadline)?;
    let lane = pool.claim(
        session,
        ClaimBase::Commit(generation.base.as_str().to_owned()),
    )?;
    // Invariant: a claim keeps an unmerged branch, but a staging branch holds no one's only copy.
    let base = ["reset", "-q", "--hard", generation.base.as_str()];
    git_by(lane.path(), &base, deadline)?;
    let message = format!(
        "Integrate {} onto {}",
        candidate.short(),
        generation.base.short()
    );
    let merge = [
        "merge",
        "--no-ff",
        "--no-edit",
        "-m",
        &message,
        candidate.as_str(),
    ];
    let merged = git_by(
        lane.path(),
        &[&identity(lane.path())[..], &merge[..]].concat(),
        deadline,
    );
    if let Err(error) = merged {
        let merge_base = git_by(
            lane.path(),
            &["merge-base", generation.base.as_str(), candidate.as_str()],
            deadline,
        )
        .ok()
        .and_then(|text| Sha::parse(&text))
        .unwrap_or_else(|| generation.base.clone());
        let conflicts = conflicts_of(lane.path(), &merge_base, deadline)?;
        let _aborted_best_effort = git_by(lane.path(), &["merge", "--abort"], deadline);
        lane.discard()?;
        if conflicts.is_empty() {
            return Err(error);
        }
        return Ok(Prepared::Conflict {
            base: generation.base.clone(),
            conflicts,
        });
    }
    let integrated = sha_of(&git_by(lane.path(), &["rev-parse", "HEAD"], deadline)?)?;
    Ok(Prepared::Merged(Staging {
        lane: Some(lane),
        base: generation.base.clone(),
        candidate: candidate.clone(),
        integrated,
    }))
}

/// What publishing found: the parent at the prepared generation, or moved past it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Publish {
    Published {
        at: Sha,
        how: Published,
        kept: Vec<String>,
    },
    Moved {
        expected: Sha,
        found: Sha,
    },
}

/// Publish `integrated` onto the parent while its HEAD still names `expected`, else `Moved`. A
/// fast-forward when git can; the ref alone on the user's dirt; any other refusal, an `Err`.
pub fn publish(
    parent: &Path,
    expected: &Sha,
    expected_branch: Option<&str>,
    integrated: &Sha,
    deadline: Option<Instant>,
) -> Result<Publish, LaneError> {
    // Every pathspec below is repository-root relative, as `diff --name-only` and `status`
    // print them, so every command runs from the toplevel whatever the session's cwd is.
    let top = PathBuf::from(git_by(parent, &["rev-parse", "--show-toplevel"], deadline)?.trim());
    let parent = top.as_path();
    let now = generation_of(parent, deadline)?;
    // The generation is the commit and the branch that names it: a checkout switched to
    // another branch at the same commit has moved too, and is prepared again on it.
    let switched = expected_branch
        .is_some_and(|expected| now.head.branch().map(BranchName::as_str) != Some(expected));
    if now.base != *expected || switched {
        return Ok(Publish::Moved {
            expected: expected.clone(),
            found: now.base,
        });
    }
    let Some(branch) = now.head.branch() else {
        return Err(LaneError::Unpublishable {
            path: parent.to_path_buf(),
        });
    };
    let forwarded = git_by(
        parent,
        &["merge", "-q", "--ff-only", integrated.as_str()],
        deadline,
    );
    let Err(refused) = forwarded else {
        return Ok(Publish::Published {
            at: integrated.clone(),
            how: Published::FastForward,
            kept: Vec::new(),
        });
    };
    let again = generation_of(parent, deadline)?;
    if again.base != *expected {
        return Ok(Publish::Moved {
            expected: expected.clone(),
            found: again.base,
        });
    }
    let changed = nul_lines(&git_by(
        parent,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            expected.as_str(),
            integrated.as_str(),
        ],
        deadline,
    )?);
    // `-uall`: an untracked directory is listed file by file, so a path the integration adds
    // inside one counts as the user's and is never restored over.
    let dirty = status_paths(&git_by(
        parent,
        &["status", "--porcelain", "-z", "-uall"],
        deadline,
    )?);
    // Only the user's overlapping dirt takes the ref-only path. An index lock, a hook, or a
    // merge or rebase the user has not concluded is git's own refusal, returned to retry.
    if in_progress(parent, deadline)? || !changed.iter().any(|path| dirty.contains(path)) {
        return Err(refused);
    }
    let staged = nul_lines(&git_by(
        parent,
        &["diff", "--cached", "--name-only", "--no-renames", "-z"],
        deadline,
    )?);
    let refname = format!("refs/heads/{}", branch.as_str());
    git_by(
        parent,
        &[
            "update-ref",
            "-m",
            "yi: accept",
            &refname,
            integrated.as_str(),
            expected.as_str(),
        ],
        deadline,
    )?;
    // The checkout follows the ref on every path the user did not touch: the tree first, while
    // the old index names the deleted paths; then the index. ponytail: argv, ARG_MAX ceiling.
    let (kept, untouched): (Vec<&String>, Vec<&String>) =
        changed.iter().partition(|path| dirty.contains(path));
    let untouched: Vec<&str> = untouched.into_iter().map(String::as_str).collect();
    if !untouched.is_empty() {
        let source = format!("--source={}", integrated.as_str());
        let mut args = vec!["restore", "-q", &source, "--worktree", "--"];
        args.extend(untouched);
        git_by(parent, &args, deadline)?;
    }
    let unstaged: Vec<&str> = changed
        .iter()
        .filter(|path| !staged.contains(path))
        .map(String::as_str)
        .collect();
    if !unstaged.is_empty() {
        let mut args = vec!["reset", "-q", integrated.as_str(), "--"];
        args.extend(unstaged);
        git_by(parent, &args, deadline)?;
    }
    Ok(Publish::Published {
        at: integrated.clone(),
        how: Published::RefOnly,
        kept: kept.into_iter().cloned().collect(),
    })
}

/// An unconcluded merge, rebase, cherry-pick or revert in `top`, or another git holding its
/// index: nothing publishes over either.
fn in_progress(top: &Path, deadline: Option<Instant>) -> Result<bool, LaneError> {
    let git_dir =
        PathBuf::from(git_by(top, &["rev-parse", "--absolute-git-dir"], deadline)?.trim());
    Ok([
        "index.lock",
        "MERGE_HEAD",
        "REBASE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ]
    .iter()
    .any(|name| git_dir.join(name).exists()))
}

/// Invariant: uncapped, since [`git_by`]'s 30 KB cut splices an omitted-bytes row into the list.
/// A `{path, added, removed}` per file against the base, else the merge base with `parent` HEAD.
pub(super) fn changed_files(
    parent: &Path,
    candidate: &Candidate,
    deadline: Option<Instant>,
) -> Result<Value, LaneError> {
    let from = candidate.base.as_ref().map_or("HEAD", Sha::as_str);
    let range = format!("{from}...{}", candidate.commit.as_str());
    let args = ["diff", "--numstat", "-z", "--no-renames", range.as_str()];
    let numstat = git_capped(parent, &args, deadline, usize::MAX)?;
    let rows = nul_lines(&numstat);
    let files = rows.iter().filter_map(|row| {
        let (path, added, removed) = numstat_row(row)?;
        Some(json!({"path": path, "added": added, "removed": removed}))
    });
    Ok(Value::Array(files.collect()))
}

/// One `diff --numstat` row as path, added, removed; a binary file's `-` counts read as 0.
pub(crate) fn numstat_row(row: &str) -> Option<(&str, u64, u64)> {
    let mut parts = row.splitn(3, '\t');
    let added = parts.next()?.parse().unwrap_or(0);
    let removed = parts.next()?.parse().unwrap_or(0);
    Some((parts.next()?, added, removed))
}

fn nul_lines(text: &str) -> Vec<String> {
    text.split('\0')
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Every path `status --porcelain -z` names, dirty or untracked; a rename names two.
fn status_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut records = text.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        let Some((code, path)) = status_row(record) else {
            continue;
        };
        out.push(path.to_owned());
        if code.starts_with('R') || code.starts_with('C') {
            out.extend(records.next().map(str::to_owned));
        }
    }
    out
}

/// One `-z` record's porcelain code and path: two columns, a space, then the name.
#[must_use]
pub fn status_row(record: &str) -> Option<(&str, &str)> {
    Some((record.get(..2)?, record.get(3..)?))
}

/// A slot path under the pool, so a staging lane can be named in a record without its host path.
pub fn slot_url(slot: super::SlotIndex) -> String {
    format!("lane://{slot}")
}
