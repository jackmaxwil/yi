//! A kernel child's hand-back: its worktree settled, merged into the parent, and its check read.

use super::fork::ChildTree;
use super::hostgit::git_by;
use super::settle::*;
use super::{BranchName, Sha};
use crate::subagent::{ChildRecord, SubagentHost};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::{Duration, Instant};
use yi_types::plan::op::Choice;

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

    /// A result's check on the child's tree as it stands: a fork's in a warm directory, so what it
    /// ignored stays out; a child with no tree of its own, in place.
    pub(crate) fn child_check(&self, name: &str, check: &str) -> Result<(), String> {
        let timeout = crate::goal::DEFAULT_CHECK_TIMEOUT_MS;
        let cwd = self
            .cwd_of(name)
            .unwrap_or_else(|| self.options.cwd.clone());
        if !cwd.starts_with(super::fork::forks_dir(&self.options.home)) {
            return crate::goal::run_check(check, &cwd, timeout, &self.checker());
        }
        let pool = super::Pool::open(&self.options.home, &cwd, self.options.lane_slots)
            .map_err(|error| error.to_string())?;
        let warm = pool.warm().map_err(|error| error.to_string())?;
        let commit = super::warm::snapshot(&cwd).map_err(|error| error.to_string())?;
        let queue_until = self
            .deadline()
            .or_else(|| Instant::now().checked_add(super::warm::CHECK_QUEUE))
            .unwrap_or_else(Instant::now);
        let budget = Duration::from_millis(timeout);
        let checker = self.checker();
        let run =
            super::warm::check_commit(&warm, &commit, &cwd, check, queue_until, budget, &checker);
        match run.verdict {
            super::warm::Verdict::Green => Ok(()),
            super::warm::Verdict::Red { exit } => {
                let code = exit.map_or_else(|| "signal".to_owned(), |code| code.to_string());
                Err(format!("exit {code}\n{}", run.output()))
            }
            super::warm::Verdict::Abstain { why } => Err(format!("it did not run: {why}")),
        }
    }

    /// The one settle a reap or a delete runs: the branch kept, or deleted once its pin is the
    /// record's. A settle refused before the release puts the lane back, so the slot stays held.
    pub fn settle_lane(
        lane: &mut Option<ChildTree>,
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
            Err((kept, error)) => {
                *lane = Some(*kept);
                return Err(error.to_string());
            }
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
        let tree_key = lane.path().to_string_lossy().into_owned();
        let deadline = self.deadline();
        let quiescence = quiescence_of_jobs(lane.path());
        let settled = lane
            .settle(&quiescence, deadline)
            .map_err(|(_kept, error)| error.to_string())?;
        super::warm::forget_tickets(&tree_key);
        let mut reply = settled.candidate.as_reply();
        let parent = self.options.cwd.clone();
        let files = changed_files(&parent, &settled.candidate, deadline)
            .map_err(|error| error.to_string())?;
        reply.insert("files".to_owned(), files);
        let budget = Duration::from_millis(crate::goal::DEFAULT_CHECK_TIMEOUT_MS);
        let red = check
            .map(|check| {
                red_check(
                    &pool,
                    &settled.candidate.commit,
                    &parent,
                    check,
                    deadline,
                    budget,
                    &self.checker(),
                )
            })
            .transpose()?
            .flatten();
        if let Some((capture, cap)) = red {
            let kept = settled.candidate.branch.as_str();
            let commit = settled.candidate.commit.as_str();
            reply.insert("merged".to_owned(), Value::Bool(false));
            let check = check.unwrap_or_default();
            reply.insert(
                "check".to_owned(),
                verdict(&capture, &cap, &parent, check, commit),
            );
            let kept_branch = pool.branch_exists(&settled.candidate.branch);
            let next = if kept_branch.map_err(|error| error.to_string())? {
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
            let prepared = super::stage::merged(
                &pool,
                &parent,
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
        let stopped = yi_tools::jobs::registry().kill_under(lane.path());
        let settled_jobs = |root: &Path| quiescence_of_jobs(root).running.is_empty();
        let until = Instant::now().checked_add(Duration::from_secs(5));
        while !settled_jobs(lane.path()) && until.is_some_and(|until| Instant::now() < until) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let quiescence = quiescence_of_jobs(lane.path());
        let settled = lane
            .settle(&quiescence, self.deadline())
            .map_err(|(_kept, error)| error.to_string())?;
        let candidate = settled.discard().map_err(|error| error.to_string())?;
        let mut reply = candidate.as_reply();
        reply.insert("discarded".to_owned(), Value::Bool(true));
        if !stopped.is_empty() {
            let row = format!(
                "stopped {} jobs under the tree: {}; a process that left its job keeps running",
                stopped.len(),
                stopped.join(", ")
            );
            reply.insert("stopped".to_owned(), Value::String(row));
        }
        reply.insert("merged".to_owned(), Value::Bool(false));
        Ok(reply)
    }

    /// Invariant: taken off the record, so a tree is never handed back twice; never for a
    /// child the plan engine dispatched, whose branch goes through `submit` or a disposition.
    fn take_settled_worktree(&self, target: &str) -> Result<(ChildTree, String), String> {
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
        if record
            .worktree
            .as_ref()
            .is_some_and(ChildTree::carries_dirt)
        {
            return Err(format!(
                "child \"{target}\" took a tree that carries uncommitted files, which a commit would claim as its work: merge it with h.tree.merge() from the tree it came from"
            ));
        }
        let lane = record
            .worktree
            .take()
            .ok_or_else(|| format!("child \"{target}\" has no worktree (isolation was none)"))?;
        Ok((lane, record.session_name.clone()))
    }
}

/// A hand-back's `check` on the settled commit, under the session deadline or `budget` with none;
/// `None` is a pass, otherwise the capture and what bounded it. Past a bound is no pass either.
fn red_check(
    pool: &super::Pool,
    commit: &Sha,
    parent: &Path,
    check: &str,
    deadline: Option<Instant>,
    budget: Duration,
    checker: &crate::goal::Checker,
) -> Result<Option<(yi_tools::CommandCapture, String)>, String> {
    let warm = pool.warm().map_err(|error| error.to_string())?;
    let queue_until = deadline
        .or_else(|| Instant::now().checked_add(super::warm::CHECK_QUEUE))
        .unwrap_or_else(Instant::now);
    let left = deadline.map(|at| at.saturating_duration_since(Instant::now()));
    let budget = left.map_or(budget, |left| left.min(budget));
    let run = super::warm::check_commit(&warm, commit, parent, check, queue_until, budget, checker);
    let cap = deadline.map_or_else(
        || format!("the {} ms check timeout", budget.as_millis()),
        |_| "the session deadline".to_owned(),
    );
    Ok(match (run.verdict, run.capture) {
        (super::warm::Verdict::Green, _) => None,
        (_, Some(capture)) => Some((capture, cap)),
        (super::warm::Verdict::Red { .. }, None) => Some((never_ran(), cap)),
        (super::warm::Verdict::Abstain { why }, None) => Some((never_ran(), why)),
    })
}

/// No process behind it: a verdict's reply reads this as a check that was cut, with its reason.
fn never_ran() -> yi_tools::CommandCapture {
    yi_tools::CommandCapture {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        cancelled: true,
        truncated: false,
        kill_error: None,
        spill: None,
        cut: None,
        rows: Vec::new(),
    }
}

/// A red check's `{exit, tail}` through [`crate::goal::check_view`]; its rerun is the check on
/// `commit` in a scratch worktree, since the released lane's slot is reused.
fn verdict(
    capture: &yi_tools::CommandCapture,
    cap: &str,
    parent: &Path,
    check: &str,
    commit: &str,
) -> Value {
    let rerun = format!(
        "d=$(mktemp -d) && git worktree add -q --detach \"$d\" {commit} && (cd \"$d\" && {check}); s=$?; git worktree remove --force \"$d\"; exit $s"
    );
    let view = crate::goal::check_view(&rerun, parent, None, capture);
    let cut = capture.cancelled.then(|| {
        let call = crate::goal::rerun_in(parent, &rerun);
        format!("[… cut at {cap}; the rest: bash ({call}) > view.log 2>&1, then read view.log]\n")
    });
    let exit = capture.exit_code.filter(|_| !capture.cancelled);
    json!({"exit": exit, "tail": format!("{}{view}", cut.unwrap_or_default())})
}
