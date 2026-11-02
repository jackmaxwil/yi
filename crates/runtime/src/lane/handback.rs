//! A kernel child's hand-back: its worktree settled, merged into the parent, and its check read.

use super::fork::{ChildTree, SettledTree};
use super::hostgit::git_by;
use super::settle::*;
use super::{BranchName, Pool, Sha};
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
        let deadline = self.deadline();
        let budget = Duration::from_millis(crate::goal::DEFAULT_CHECK_TIMEOUT_MS);
        let red = check
            .map(|check| red_check(lane.path(), check, deadline, budget, &self.checker()))
            .transpose();
        let red = match red {
            Ok(red) => red.flatten(),
            Err(error) => {
                self.put_back_worktree(target, lane);
                return Err(error);
            }
        };
        let quiescence = quiescence_of_jobs(lane.path());
        // Invariant: a cut check spent the deadline, and the settle after it still keeps the work.
        let deadline = deadline.filter(|_| !red.as_ref().is_some_and(|(red, _)| red.cancelled));
        let settled = match lane.settle(&quiescence, deadline) {
            Ok(settled) => settled,
            Err((kept, error)) => {
                self.put_back_worktree(target, *kept);
                return Err(error.to_string());
            }
        };
        let branch = settled.candidate.branch.clone();
        self.land_settled(settled, red, check, &name, &pool, deadline)
            .map_err(|error| match pool.branch_exists(&branch) {
                Ok(true) if !error.contains(branch.as_str()) => format!(
                    "{error}; the child's lane is released and its work is kept on branch {branch}"
                ),
                _ => error,
            })
    }

    /// Past the settle the tree is gone and the commit is on its branch, so a failure from here on
    /// has to say where the work went.
    fn land_settled(
        &self,
        settled: SettledTree,
        red: Option<(yi_tools::CommandCapture, String)>,
        check: Option<&str>,
        name: &str,
        pool: &Pool,
        deadline: Option<Instant>,
    ) -> Result<Map<String, Value>, String> {
        let mut reply = settled.candidate.as_reply();
        let parent = self.options.cwd.clone();
        let files = changed_files(&parent, &settled.candidate, deadline)
            .map_err(|error| error.to_string())?;
        reply.insert("files".to_owned(), files);
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
                pool,
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
        let settled = match lane.settle(&quiescence, self.deadline()) {
            Ok(settled) => settled,
            Err((kept, error)) => {
                self.put_back_worktree(target, *kept);
                return Err(error.to_string());
            }
        };
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

    /// A failure before the tree retired hands it back to the record, so a retry finds it and it
    /// waits on no retention sweep with nothing naming it.
    fn put_back_worktree(&self, target: &str, tree: ChildTree) {
        if let Ok(mut children) = self.children.lock()
            && let Ok((_, record)) = Self::record_mut(&mut children, target)
        {
            record.worktree = Some(tree);
        }
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
        let lane = record
            .worktree
            .take()
            .ok_or_else(|| format!("child \"{target}\" has no worktree: isolation was none, or it was already merged or discarded"))?;
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
    checker: &crate::goal::Checker,
) -> Result<Option<(yi_tools::CommandCapture, String)>, String> {
    let until = deadline.or_else(|| Instant::now().checked_add(budget));
    let until = until.unwrap_or_else(Instant::now);
    let capture = crate::goal::run_check_in(path, check, &[], None, until, None, checker)?;
    if capture.exit_code == Some(0) && !capture.cancelled {
        return Ok(None);
    }
    let cap = deadline.map_or_else(
        || format!("the {} ms check timeout", budget.as_millis()),
        |_| "the session deadline".to_owned(),
    );
    Ok(Some((capture, cap)))
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn a_check_with_no_session_deadline_is_cut_at_its_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let budget = Duration::from_millis(300);
        let red = super::red_check(
            &std::env::temp_dir(),
            "sleep 30",
            None,
            budget,
            &Default::default(),
        )?;
        let (capture, cap) = red.ok_or("a cut check passed")?;
        let parent = std::env::temp_dir();
        let verdict = super::verdict(&capture, &cap, &parent, "sleep 30", "abc");
        assert!(verdict["exit"].is_null(), "{verdict}");
        let tail = verdict["tail"].as_str().unwrap_or_default();
        assert!(
            tail.starts_with("[… cut at the 300 ms check timeout; the rest: bash ("),
            "{tail}"
        );
        Ok(())
    }
}
