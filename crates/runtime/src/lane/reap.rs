//! The only two wipes of a slot's tree, a reap and a discard, the count of what each drops, and
//! `yi gc`, which removes linked worktrees `main` already holds.

use std::path::{Path, PathBuf};

use super::{
    ClaimBase, Head, LIST_BUDGET, LaneError, Pool, STATUS, Sha, SlotIndex, git, git_capture, head,
};

/// Uncommitted paths one status read named; past the capture cap the count is a floor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Uncommitted {
    paths: usize,
    cut: Option<(usize, usize)>,
}

impl Uncommitted {
    pub(crate) fn read(tree: &Path) -> Result<Self, LaneError> {
        Ok(Self::counted(&git_capture(
            tree,
            &STATUS,
            None,
            yi_tools::OUTPUT_CAP,
        )?))
    }

    /// One status read bounded by [`LIST_BUDGET`]; the error names why the paths went uncounted.
    pub(crate) fn read_briefly(tree: &Path) -> Result<Self, String> {
        let started = std::time::Instant::now();
        let capture = super::capture_named(
            tree,
            "git status",
            "git",
            &STATUS,
            LIST_BUDGET,
            yi_tools::OUTPUT_CAP,
            None,
        );
        capture
            .map(|capture| Self::counted(&capture))
            .map_err(|error| {
                if started.elapsed() >= LIST_BUDGET {
                    format!("git status past LIST_BUDGET ({} s)", LIST_BUDGET.as_secs())
                } else {
                    briefly(&error)
                }
            })
    }
    fn counted(status: &yi_tools::CommandCapture) -> Self {
        let (rows, cut) = (status.stdout.lines().count(), status.cut);
        // Invariant: a cut list is a head and a tail around a marker row; three lines may be no path.
        let paths = rows.saturating_sub(if cut.is_some() { 3 } else { 0 });
        Self { paths, cut }
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

/// One failed read says why: the diagnostics past capture_named's label, and any cut it makes.
fn briefly(error: &str) -> String {
    let mut lines = error.lines();
    let label = lines.next().unwrap_or_default();
    let mut body = lines.filter(|line| !line.trim().is_empty());
    let kept: Vec<&str> = body.by_ref().take(2).collect();
    let more = body.count();
    if kept.is_empty() {
        return label.to_owned();
    }
    let joined = kept.join(" ");
    if more == 0 {
        format!("{label} {joined}")
    } else {
        format!("{label} {joined} [… {more} more]")
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

    pub(super) fn clear_session(&self, slot: SlotIndex) -> Result<(), LaneError> {
        let mut state = self.read_state(slot)?;
        state.session = None;
        self.write_state(slot, &state)
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
        self.clear_session(slot)?;
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

struct Tree {
    path: PathBuf,
    locked: bool,
}

fn parse(listing: &str) -> Vec<Tree> {
    let mut trees = Vec::new();
    for block in listing.split("\n\n") {
        let mut tree = Tree {
            path: PathBuf::new(),
            locked: false,
        };
        for line in block.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                tree.path = PathBuf::from(path);
            } else if line == "locked" || line.starts_with("locked ") {
                tree.locked = true;
            }
        }
        if !tree.path.as_os_str().is_empty() {
            trees.push(tree);
        }
    }
    trees
}

impl Pool {
    /// Invariant: nothing is removed that holds work; a live lane is flocked, an outside tree is
    /// locked, a commit `main` lacks or a dirty tree stays, and every kept tree says why. Dry run is read-only.
    pub fn gc(&self, cwd: &Path, dry_run: bool) -> Result<Vec<String>, LaneError> {
        let listing = git_capture(
            &self.repo,
            &["worktree", "list", "--porcelain"],
            None,
            super::PLUMBING_CAP,
        )?;
        let deadline = std::time::Instant::now()
            .checked_add(LIST_BUDGET)
            .unwrap_or_else(std::time::Instant::now);
        let base = self.local_main(deadline);
        let base = base.as_ref().map(Sha::as_str);
        let here = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let slots = (0..self.slots)
            .map(SlotIndex)
            .filter_map(|slot| {
                let path = self.slot_path(slot).canonicalize().ok()?;
                Some((path, slot))
            })
            .collect::<Vec<_>>();
        let mut lines = Vec::new();
        if let Some((kept, total)) = listing.cut {
            lines.push(format!(
                "[worktree list cut: read {kept} of {total} bytes, lane.plumbing_cap {}; later worktrees not judged]",
                super::PLUMBING_CAP
            ));
        }
        for tree in parse(&listing.stdout).into_iter().skip(1) {
            let shown = tree.path.display();
            let resolved = tree
                .path
                .canonicalize()
                .unwrap_or_else(|_| tree.path.clone());
            let slot = slots
                .iter()
                .find(|(path, _)| path == &resolved)
                .map(|(_, slot)| *slot);
            let judge = |status| {
                base.ok_or("no main or origin/main to judge against".to_owned())
                    .and_then(|base| self.judge(&tree, base, &here, slot.is_some(), status))
            };
            if let Err(why) = judge(true) {
                lines.push(format!("kept {shown}: {why}"));
                continue;
            }
            let _pool = self.lock()?;
            let _held = match slot.map(|slot| self.probe(slot)) {
                Some(Ok((_, Some(_)))) => {
                    lines.push(format!("kept {shown}: a live session holds the slot"));
                    continue;
                }
                Some(Err(why)) => {
                    lines.push(format!("kept {shown}: slot unreadable: {why}"));
                    continue;
                }
                Some(Ok((guard, None))) => Some(guard),
                None => None,
            };
            if dry_run {
                lines.push(format!("would remove {shown}"));
                continue;
            }
            lines.push(match judge(false) {
                Err(why) => format!("kept {shown}: {why}"),
                // Incident: sixty worktrees' own `target/` directories filled a 1.8 TB disk. Git
                // removes ignored files with a clean tree and refuses the rest.
                Ok(()) => match (|| {
                    if let Some(slot) = slot {
                        self.stop_warmer(slot)?;
                    }
                    let path = tree.path.to_string_lossy();
                    if tree.locked && slot.is_some() {
                        git(&self.repo, &["worktree", "unlock", &path])?;
                    }
                    git(&self.repo, &["worktree", "remove", &path])?;
                    if let Some(slot) = slot {
                        self.clear_session(slot)?;
                    }
                    Ok::<(), LaneError>(())
                })()
                .map(drop)
                .map_err(|error| error.to_string())
                {
                    Ok(()) => format!("removed {shown}"),
                    Err(why) => format!("kept {shown}: {why}"),
                },
            });
        }
        Ok(lines)
    }

    fn judge(
        &self,
        tree: &Tree,
        base: &str,
        here: &Path,
        pool_slot: bool,
        status: bool,
    ) -> Result<(), String> {
        let current = head(&tree.path).map_err(|_| "HEAD missing".to_owned())?;
        let name = current
            .branch()
            .map(|branch| branch.as_str())
            .unwrap_or("detached HEAD");
        if tree.locked && !pool_slot {
            return Err("locked, a session holds it".to_owned());
        }
        if here.starts_with(&tree.path) {
            return Err("the current directory is inside it".to_owned());
        }
        let tip = match &current {
            Head::Branch(branch) => git(&self.repo, &["rev-parse", branch.as_str()])
                .map_err(|e| e.to_string())?
                .trim()
                .to_owned(),
            Head::Detached(sha) => sha.as_str().to_owned(),
        };
        if current.branch().is_some() && tip == base {
            return Err(format!("{name} is at {base}, no commits of its own"));
        }
        git(&self.repo, &["merge-base", "--is-ancestor", &tip, base])
            .map_err(|_| format!("{name} is not merged into {base}"))?;
        if !status {
            return Ok(());
        }
        let dirty = Uncommitted::read(&tree.path).map_err(|e| format!("status unreadable: {e}"))?;
        match dirty.is_empty() {
            true => Ok(()),
            false => Err(dirty.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Dies when a failed status read drops git's own cause and keeps only capture_named's label.
    #[test]
    fn a_failed_read_keeps_the_diagnostic_and_names_its_cut() {
        let two = briefly(
            "git status failed:\nerror: bad signature 0x00000000\nfatal: index file corrupt\n",
        );
        assert_eq!(
            two,
            "git status failed: error: bad signature 0x00000000 fatal: index file corrupt"
        );
        let many = briefly("git status failed:\nwarning: a\nfatal: b\nhint: c\nhint: d\n");
        assert_eq!(many, "git status failed: warning: a fatal: b [… 2 more]");
        let bare = briefly("git status: No such file or directory (os error 2)");
        assert_eq!(bare, "git status: No such file or directory (os error 2)");
    }

    #[test]
    fn judgment_reads_a_detached_head_newer_than_the_listing()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("yi-gc-live-head-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo)?;
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "lanes@test"][..],
            &["config", "user.name", "lanes"][..],
        ] {
            git(&repo, args)?;
        }
        std::fs::write(repo.join("README.md"), "base\n")?;
        git(&repo, &["add", "."])?;
        git(&repo, &["commit", "-qm", "base"])?;
        let path = root.join("detached");
        git(
            &repo,
            &["worktree", "add", "-q", "--detach", &path.to_string_lossy()],
        )?;
        let listing = git(&repo, &["worktree", "list", "--porcelain"])?;
        let tree = parse(&listing)
            .into_iter()
            .nth(1)
            .ok_or("no detached tree")?;
        let base = git(&repo, &["rev-parse", "main"])?.trim().to_owned();
        let pool = Pool::open(&root.join("home"), &repo, 1)?;
        pool.judge(&tree, &base, &repo, false, true)?;

        std::fs::write(path.join("new.txt"), "unreferenced\n")?;
        git(&path, &["add", "."])?;
        git(&path, &["commit", "-qm", "new detached commit"])?;
        let verdict = pool.judge(&tree, &base, &repo, false, true);
        assert!(
            verdict.is_err(),
            "stale listing HEAD authorized removing new work"
        );
        Ok(())
    }
}
