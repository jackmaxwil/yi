//! Settling a lane (plan section 6.6): quiescent, committed as the candidate, slot released
//! with the branch kept; plus the staging merge and the generation-checked publication.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
pub use yi_types::plan::acceptance::{Conflict, Published, Quiescence};
use yi_types::plan::op::Choice;

use super::{
    BranchName, ClaimBase, GIT_TIMEOUT_MS, Head, Lane, LaneError, Pool, Sha, SlotView,
    capture_named,
};
use crate::subagent::{ChildRecord, SubagentHost};

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

fn quiet(quiescence: &Quiescence) -> Result<(), LaneError> {
    if quiescence.is_quiet() {
        return Ok(());
    }
    Err(LaneError::Busy {
        running: quiescence.running.clone(),
    })
}

/// The immutable output references a settled lane hands on: the candidate commit and the
/// branch that holds it, and the base the branch was cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub branch: BranchName,
    pub commit: Sha,
    pub base: Option<Sha>,
}

impl Candidate {
    pub fn as_reply(&self) -> Map<String, Value> {
        let mut reply = Map::new();
        reply.insert(
            "branch".to_owned(),
            Value::String(self.branch.as_str().to_owned()),
        );
        reply.insert(
            "candidate".to_owned(),
            Value::String(self.commit.as_str().to_owned()),
        );
        reply
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
        if self.branch_kept() {
            git_by(
                &self.pool.repo,
                &["branch", "-q", "-D", self.candidate.branch.as_str()],
                None,
            )?;
        }
        Ok(self.candidate)
    }

    /// False once the release deleted the branch, its tip already on main.
    fn branch_kept(&self) -> bool {
        let refname = format!("refs/heads/{}", self.candidate.branch.as_str());
        let verify = ["rev-parse", "--verify", "-q", refname.as_str()];
        git_by(&self.pool.repo, &verify, None).is_ok()
    }
}

fn past(deadline: Option<Instant>) -> Result<(), LaneError> {
    match deadline {
        Some(at) if Instant::now() >= at => Err(LaneError::Deadline),
        _ => Ok(()),
    }
}

/// `git` under the caller's deadline as well as the lane's own: a settle step cannot outlive
/// the run it belongs to (D177).
fn git_by(cwd: &Path, args: &[&str], deadline: Option<Instant>) -> Result<String, LaneError> {
    git_capped(cwd, args, deadline, yi_tools::OUTPUT_CAP)
}

fn git_capped(
    cwd: &Path,
    args: &[&str],
    deadline: Option<Instant>,
    cap: usize,
) -> Result<String, LaneError> {
    past(deadline)?;
    let own = Duration::from_millis(GIT_TIMEOUT_MS);
    let budget = deadline.map_or(own, |at| {
        own.min(at.saturating_duration_since(Instant::now()))
    });
    let label = format!("git {}", args.join(" "));
    capture_named(cwd, &label, "git", args, budget, cap, None)
        .map(|capture| capture.stdout)
        .map_err(|output| LaneError::Git {
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            exit_code: None,
            output,
        })
}

fn sha_of(text: &str) -> Result<Sha, LaneError> {
    Sha::parse(text).ok_or_else(|| LaneError::Git {
        args: vec!["rev-parse".to_owned()],
        exit_code: None,
        output: format!("not a commit id: {text:?}"),
    })
}

const UNTRACKED_NOISE: [&str; 3] = [
    ":(exclude,glob)**/__pycache__/**",
    ":(exclude,glob)**/*.pyc",
    ":(exclude,glob)**/.pytest_cache/**",
];

const KEPT_YOURS: &str = "Your uncommitted edits to the kept_yours paths stay in the working copy without the child's change to them, so `git diff HEAD` shows that change as removed and committing them would undo it; `git show HEAD:<path>` is the merged version to fold them into; a kept path missing from HEAD is one the child deleted, and it is untracked here.";

/// Incident: uncommitted child work merged as nothing, so it is committed first. Free of the
/// `Lane`, so a host reads it with the path cloned and no lock held across the git calls.
pub fn candidate_of(
    path: &Path,
    session: &str,
    branch: &BranchName,
    base: Option<Sha>,
    quiescence: &Quiescence,
    deadline: Option<Instant>,
) -> Result<Candidate, LaneError> {
    quiet(quiescence)?;
    let status = git_by(path, &super::STATUS, deadline)?;
    if !status.trim().is_empty() {
        git_by(
            path,
            &[&["add", "-A", "--", "."][..], &UNTRACKED_NOISE].concat(),
            deadline,
        )?;
        let staged = git_by(path, &["diff", "--cached", "--name-only"], deadline)?;
        let message = format!("session {session} work");
        if !staged.trim().is_empty() {
            git_by(path, &["commit", "-q", "-m", &message], deadline)?;
        }
    }
    let commit = sha_of(&git_by(path, &["rev-parse", "HEAD"], deadline)?)?;
    Ok(Candidate {
        branch: branch.clone(),
        commit,
        base,
    })
}

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
        self.release_inner(false).map_err(Unsettled::Lost)?;
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
            lane.release_inner(false)?;
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
    let refname = format!("refs/heads/{branch}");
    if git_by(
        pool.repo(),
        &["rev-parse", "--verify", "-q", &refname],
        None,
    )
    .is_ok()
    {
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
    let message = format!(
        "Integrate {} onto {}",
        candidate.short(),
        generation.base.short()
    );
    let merged = git_by(
        lane.path(),
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            &message,
            candidate.as_str(),
        ],
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
fn changed_files(
    parent: &Path,
    candidate: &Candidate,
    deadline: Option<Instant>,
) -> Result<Value, LaneError> {
    let from = candidate.base.as_ref().map_or("HEAD", Sha::as_str);
    let range = format!("{from}...{}", candidate.commit.as_str());
    let args = ["diff", "--numstat", "-z", "--no-renames", range.as_str()];
    let numstat = git_capped(parent, &args, deadline, usize::MAX)?;
    let files = numstat.split('\0').filter_map(|record| {
        let mut parts = record.splitn(3, '\t');
        let added = parts.next()?.parse().unwrap_or(0u64);
        let removed = parts.next()?.parse().unwrap_or(0u64);
        Some(json!({"path": parts.next()?, "added": added, "removed": removed}))
    });
    Ok(Value::Array(files.collect()))
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
        let Some(path) = record.get(3..) else {
            continue;
        };
        out.push(path.to_owned());
        if record.starts_with('R') || record.starts_with('C') {
            out.extend(records.next().map(str::to_owned));
        }
    }
    out
}

/// What a host reads off a child's lane for the engine: the candidate committed while quiescent,
/// the quiescence it read, and the checkout it was read in, where a `local://` output resolves.
#[derive(Debug, Clone)]
pub struct Held {
    pub candidate: Candidate,
    pub quiescence: Quiescence,
    pub path: PathBuf,
}

impl SubagentHost {
    fn record_mut<'a>(
        children: &'a mut std::collections::HashMap<String, ChildRecord>,
        target: &str,
    ) -> Result<(String, &'a mut ChildRecord), String> {
        let key = Self::key_of(children, target)?;
        let record = children
            .get_mut(&key)
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
        Ok((key, record))
    }

    /// The delegate's read: the child's lane committed as its candidate while quiescent, with
    /// the checkout cloned and no lock held, and nothing marked on the record; `None` has no lane.
    pub fn candidate_of(&self, target: &str) -> Result<Option<Held>, String> {
        let checkout = {
            let mut children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            let (_, record) = Self::record_mut(&mut children, target)?;
            match record.worktree.as_ref() {
                Some(lane) => lane.checkout(),
                None => return Ok(None),
            }
        };
        let (path, session, branch, base) = checkout;
        let quiescence = quiescence_of_jobs(&path);
        let candidate = candidate_of(&path, &session, &branch, base, &quiescence, self.deadline())
            .map_err(|error| error.to_string())?;
        Ok(Some(Held {
            candidate,
            quiescence,
            path,
        }))
    }

    /// Invariant: the choice is set on the record only once its disposition record is committed,
    /// so `delete` and the reap never delete a branch on a choice the journal does not carry.
    pub fn mark_disposed(&self, target: &str, choice: Choice) -> Result<(), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let (_, record) = Self::record_mut(&mut children, target)?;
        record.disposition = Some(choice);
        Ok(())
    }

    /// A child the plan engine dispatched: the kernel's merge and discard are refused for it,
    /// since its branch goes only through `submit` and a journaled disposition (section 6.6).
    pub fn mark_managed(&self, target: &str) -> Result<(), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let (_, record) = Self::record_mut(&mut children, target)?;
        record.managed = true;
        Ok(())
    }

    /// The one settle a reap or a delete runs: the branch kept, or deleted once its pin is the
    /// record's. A settle refused before the release puts the lane back, so the slot stays held.
    pub fn settle_lane(
        lane: &mut Option<Lane>,
        choice: Option<Choice>,
        deadline: Option<Instant>,
    ) -> Result<Option<(Choice, Candidate)>, String> {
        let Some(held) = lane.take() else {
            return Ok(None);
        };
        let choice = choice.unwrap_or(Choice::Retained);
        let quiescence = quiescence_of_jobs(held.path());
        let settled = match held.settle(&quiescence, deadline) {
            Ok(settled) => settled,
            Err(Unsettled::Kept(kept, error)) => {
                *lane = Some(*kept);
                return Err(error.to_string());
            }
            Err(Unsettled::Lost(error)) => return Err(error.to_string()),
        };
        let candidate = match choice {
            Choice::Retained => settled.candidate,
            Choice::Discarded => settled.discard().map_err(|error| error.to_string())?,
        };
        Ok(Some((choice, candidate)))
    }

    /// `settle_lane` for a record taken off `children`: a settle that fails puts the record back
    /// under `key` with its lane, permit and the refusal as its error, and the call fails.
    pub(crate) fn settle_or_restore(
        &self,
        key: &str,
        mut record: ChildRecord,
    ) -> Result<(ChildRecord, Option<(Choice, Candidate)>), String> {
        match Self::settle_lane(&mut record.worktree, record.disposition, self.deadline()) {
            Ok(settled) => Ok((record, settled)),
            Err(reason) => {
                self.restore(key, record, &format!("its lane is held: {reason}"));
                Err(reason)
            }
        }
    }

    /// A removal that could not finish puts the record back under its key, the cause on it.
    pub(crate) fn restore(&self, key: &str, mut record: ChildRecord, cause: &str) {
        record.step(crate::subagent::Step::Held(cause.to_owned()));
        if let Ok(mut children) = self.children.lock() {
            children.insert(key.to_owned(), record);
            children.touch(key, crate::family::Cause::Held);
        }
    }

    /// Worktree hand-back through the acceptance mechanics: settled, staged, published under the
    /// generation check (one retry on a moved parent); a conflict answers `merged: false`.
    pub fn merge_worktree(
        &self,
        target: &str,
        check: Option<&str>,
    ) -> Result<Map<String, Value>, String> {
        let gate = self.check_gate.get().map(|gates| &gates.0);
        if let Some(refused) = check.and_then(|check| gate?(check)) {
            return Err(format!(
                "the check is refused before it runs on the host: {refused}"
            ));
        }
        let (lane, name) = self.take_settled_worktree(target)?;
        let pool = lane.pool().clone();
        let deadline = self.deadline();
        let budget = Duration::from_millis(crate::goal::DEFAULT_CHECK_TIMEOUT_MS);
        let red = check
            .map(|check| red_check(lane.path(), check, deadline, budget))
            .transpose()?
            .flatten();
        let quiescence = quiescence_of_jobs(lane.path());
        // Invariant: a cut check spent the deadline, and the settle after it still keeps the work.
        let deadline = deadline.filter(|_| !red.as_ref().is_some_and(|(red, _)| red.cancelled));
        let settled = lane
            .settle(&quiescence, deadline)
            .map_err(|unsettled| unsettled.error().to_string())?;
        let mut reply = settled.candidate.as_reply();
        let parent = self.options.cwd.clone();
        let files = changed_files(&parent, &settled.candidate, deadline)
            .map_err(|error| error.to_string())?;
        reply.insert("files".to_owned(), files);
        if let Some((capture, cap)) = red {
            let kept = settled.candidate.branch.as_str();
            let commit = settled.candidate.commit.as_str();
            let rerun = format!(
                "`git worktree add --detach <dir> {commit}` and run the check there for the rest"
            );
            reply.insert("merged".to_owned(), Value::Bool(false));
            reply.insert("check".to_owned(), verdict(&capture, &cap, &rerun));
            let next = if settled.branch_kept() {
                format!(
                    "The check failed, so this checkout is unchanged and the child's lane is released; its work is kept on branch {kept}. `git merge {kept}` lands it anyway, and `git branch -D {kept}` drops it."
                )
            } else {
                "The check failed, so this checkout is unchanged and the child's lane is released; the child left no commit main lacks, so no branch was kept.".to_owned()
            };
            reply.insert("next".to_owned(), json!(next));
            return Ok(reply);
        }
        let mut moved: Option<(Sha, Sha)> = None;
        for _attempt in 0..2 {
            let _integration = INTEGRATION.lock().unwrap_or_else(PoisonError::into_inner);
            let generation = generation_of(&parent, deadline).map_err(|error| error.to_string())?;
            let session = format!("stage-{name}");
            let prepared = prepare(
                &pool,
                &session,
                &generation,
                &settled.candidate.commit,
                deadline,
            )
            .map_err(|error| error.to_string())?;
            let staging = match prepared {
                Prepared::Conflict { base, conflicts } => {
                    reply.insert("merged".to_owned(), Value::Bool(false));
                    reply.insert(
                        "parent_base".to_owned(),
                        Value::String(base.as_str().to_owned()),
                    );
                    reply.insert(
                        "conflict_provenance".to_owned(),
                        serde_json::to_value(conflicts).map_err(|error| error.to_string())?,
                    );
                    // Invariant: the lane is already off the record, so `rlm.discard_worktree`
                    // would answer "no worktree"; the branch is dropped with git.
                    let kept = settled.candidate.branch.as_str();
                    reply.insert(
                        "next".to_owned(),
                        json!(format!(
                            "This checkout is unchanged and the child's lane is released; its work is kept on branch {kept}. `git merge {kept}` here stops at the conflicts listed, to resolve and commit, and `git branch -D {kept}` drops the work."
                        )),
                    );
                    return Ok(reply);
                }
                Prepared::Merged(staging) => staging,
            };
            let branch = generation.head.branch().map(BranchName::as_str);
            let published = publish(
                &parent,
                &generation.base,
                branch,
                &staging.integrated,
                deadline,
            )
            .map_err(|error| error.to_string())?;
            staging.release().map_err(|error| error.to_string())?;
            match published {
                Publish::Published { at, how, kept } => {
                    reply.insert("merged".to_owned(), Value::Bool(true));
                    reply.insert(
                        "integrated".to_owned(),
                        Value::String(at.as_str().to_owned()),
                    );
                    reply.insert(
                        "published".to_owned(),
                        serde_json::to_value(how).map_err(|error| error.to_string())?,
                    );
                    if how == Published::RefOnly {
                        reply.insert("kept_yours".to_owned(), json!(kept));
                        reply.insert("next".to_owned(), json!(KEPT_YOURS));
                    }
                    let _merged_branch_goes_with_the_slot = git_by(
                        pool.repo(),
                        &["branch", "-q", "-d", settled.candidate.branch.as_str()],
                        deadline,
                    );
                    return Ok(reply);
                }
                Publish::Moved { expected, found } => moved = Some((expected, found)),
            }
        }
        let (expected, found) = moved.unwrap_or_else(|| {
            (
                settled.candidate.commit.clone(),
                settled.candidate.commit.clone(),
            )
        });
        Err(format!(
            "the parent moved from {} to {} while the merge was prepared, twice; the candidate {} is retained on branch {}: merge that branch by hand, the child's lane is released",
            expected.short(),
            found.short(),
            settled.candidate.commit.short(),
            settled.candidate.branch.as_str()
        ))
    }

    /// The `Discarded` disposition for a kernel-driven child: settled, then the branch deleted.
    pub fn discard_worktree(&self, target: &str) -> Result<Map<String, Value>, String> {
        let (lane, _) = self.take_settled_worktree(target)?;
        let quiescence = quiescence_of_jobs(lane.path());
        let settled = lane
            .settle(&quiescence, self.deadline())
            .map_err(|unsettled| unsettled.error().to_string())?;
        let candidate = settled.discard().map_err(|error| error.to_string())?;
        let mut reply = candidate.as_reply();
        reply.insert("discarded".to_owned(), Value::Bool(true));
        reply.insert("merged".to_owned(), Value::Bool(false));
        Ok(reply)
    }

    /// Invariant: taken off the record, so a tree is never handed back twice; never for a
    /// child the plan engine dispatched, whose branch goes through `submit` or a disposition.
    fn take_settled_worktree(&self, target: &str) -> Result<(Lane, String), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let (_, record) = Self::record_mut(&mut children, target)?;
        if record.managed {
            return Err(format!(
                "child \"{target}\" was dispatched by the plan: its worktree is accepted through plan.op submit and done, or disposed by fail or drop, never merged here"
            ));
        }
        if record.exit.is_none() {
            return Err(format!(
                "child \"{target}\" is still running; wait for it before touching its worktree"
            ));
        }
        let lane = record
            .worktree
            .take()
            .ok_or_else(|| format!("child \"{target}\" has no worktree (isolation was none)"))?;
        drop(record.lane_permit.take());
        Ok((lane, record.session_name.clone()))
    }
}

/// A hand-back's `check` in the lane, under the session deadline or `budget` with none; `None`
/// is a pass, otherwise the capture and the cap that bounded it.
fn red_check(
    path: &Path,
    check: &str,
    deadline: Option<Instant>,
    budget: Duration,
) -> Result<Option<(yi_tools::CommandCapture, String)>, String> {
    let until = deadline.or_else(|| Instant::now().checked_add(budget));
    let until = until.unwrap_or_else(Instant::now);
    let capture = crate::goal::run_check_in(path, check, &[], None, until, None)?;
    if capture.exit_code == Some(0) && !capture.cancelled {
        return Ok(None);
    }
    let cap = deadline.map_or_else(
        || format!("the {} ms check timeout", budget.as_millis()),
        |_| "the session deadline".to_owned(),
    );
    Ok(Some((capture, cap)))
}

/// A red check's `{exit, tail}`, each cut a row naming its cap and `rerun`, the call for the rest.
fn verdict(capture: &yi_tools::CommandCapture, cap: &str, rerun: &str) -> Value {
    let tail = crate::goal::output_tail(capture);
    let total = [&capture.stdout, &capture.stderr]
        .iter()
        .filter(|out| !out.trim().is_empty())
        .map(|out| out.trim_end().chars().count().saturating_add(1))
        .sum::<usize>()
        .saturating_sub(1);
    let kept = tail.chars().count();
    let limit = crate::goal::CHECK_TAIL_CHARS;
    let timed_out = capture
        .cancelled
        .then(|| format!("[… cut at {cap}; {rerun}]\n"));
    let captured = capture.cut_row().map(|row| format!("{row}\n"));
    let of = if capture.truncated {
        " the capture kept"
    } else {
        ""
    };
    let trimmed = (kept < total).then(|| {
        format!("[… last {kept} of {total} chars{of} (CHECK_TAIL_CHARS {limit}); {rerun}]\n")
    });
    let cut: String = timed_out
        .into_iter()
        .chain(captured)
        .chain(trimmed)
        .collect();
    let exit = capture.exit_code.filter(|_| !capture.cancelled);
    json!({"exit": exit, "tail": format!("{cut}{tail}")})
}

/// A slot path under the pool, so a staging lane can be named in a record without its host path.
pub fn slot_url(slot: super::SlotIndex) -> String {
    format!("lane://{slot}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn a_check_with_no_session_deadline_is_cut_at_its_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let budget = Duration::from_millis(300);
        let red = super::red_check(&std::env::temp_dir(), "sleep 30", None, budget)?;
        let (capture, cap) = red.ok_or("a cut check passed")?;
        let verdict = super::verdict(&capture, &cap, "rerun");
        assert!(verdict["exit"].is_null(), "{verdict}");
        let tail = verdict["tail"].as_str().unwrap_or_default();
        assert!(
            tail.starts_with("[… cut at the 300 ms check timeout; rerun]"),
            "{tail}"
        );
        Ok(())
    }
}
