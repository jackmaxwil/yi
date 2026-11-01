//! The only two wipes of a slot's tree, a reap and a discard, the count of what each drops, and
//! `yi gc`, which removes linked worktrees `main` already holds.

use std::path::{Path, PathBuf};

use super::{
    ClaimBase, Head, LIST_BUDGET, LaneError, Pool, STATUS, Sha, SlotIndex, git, git_capture, head,
};

const INTEGRATED_WALK_MAX: usize = 256;
const INTEGRATED_PATHS_MAX: usize = 1024;

/// How a branch's content is in trunk, cheapest test first.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    SameCommit,
    Ancestor,
    EmptyDiff,
    SameTree,
    SameContent,
    PatchId,
}

impl std::fmt::Display for Rung {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SameCommit => "same commit",
            Self::Ancestor => "ancestor",
            Self::EmptyDiff => "no changes",
            Self::SameTree => "same tree",
            Self::SameContent => "same content",
            Self::PatchId => "patch-id",
        })
    }
}

/// Whether `tip`'s content is in `trunk` however it landed: a squash merge leaves no ancestry to read.
/// Any failed question is an `Err`, which keeps the branch; only git's own exit 1 is a no.
pub fn integrated(repo: &Path, tip: &str, trunk: &str) -> Result<Option<Rung>, LaneError> {
    integrated_within(repo, tip, trunk, INTEGRATED_WALK_MAX)
}

/// A slot handed back: its branch went with `main`, or stayed and this says what git answered.
pub(super) enum Handback {
    Deleted,
    Kept(String),
}
fn integrated_within(
    repo: &Path,
    tip: &str,
    trunk: &str,
    walk_max: usize,
) -> Result<Option<Rung>, LaneError> {
    if !super::no(git(repo, &["merge-base", "--is-ancestor", tip, trunk]))? {
        let commits = (format!("{tip}^{{commit}}"), format!("{trunk}^{{commit}}"));
        let ids = git(repo, &["rev-parse", &commits.0, &commits.1])?;
        let mut ids = ids.lines().map(str::trim);
        let rung = match (ids.next(), ids.next()) {
            (Some(tip_id), Some(trunk_id)) if tip_id == trunk_id => Rung::SameCommit,
            _ => Rung::Ancestor,
        };
        return Ok(Some(rung));
    }
    let range = format!("{trunk}...{tip}");
    if !super::no(git(repo, &["diff", "--quiet", "--no-ext-diff", &range]))? {
        return Ok(Some(Rung::EmptyDiff));
    }
    let trees = (format!("{tip}^{{tree}}"), format!("{trunk}^{{tree}}"));
    let trees = git(repo, &["rev-parse", &trees.0, &trees.1])?;
    let mut trees = trees.lines().map(str::trim);
    if let (Some(tip_tree), Some(trunk_tree)) = (trees.next(), trees.next())
        && tip_tree == trunk_tree
    {
        return Ok(Some(Rung::SameTree));
    }
    let listing = [
        "diff",
        "--name-only",
        "-z",
        "--no-ext-diff",
        "--no-renames",
        &range,
    ];
    let touched = git(repo, &listing)?;
    let touched = touched
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    // Invariant: a list past the cap skips this rung, not the ladder; the cap is loud only when
    // no later rung can answer either.
    let past_paths = touched.len() > INTEGRATED_PATHS_MAX;
    if !past_paths && !touched.is_empty() {
        let paths = touched
            .iter()
            .map(|path| format!(":(literal){path}"))
            .collect::<Vec<_>>();
        let mut scoped = vec![
            "diff",
            "--quiet",
            "--no-ext-diff",
            "--no-renames",
            tip,
            trunk,
            "--",
        ];
        scoped.extend(paths.iter().map(String::as_str));
        if !super::no(git(repo, &scoped))? {
            return Ok(Some(Rung::SameContent));
        }
    }
    let counts = git(repo, &["rev-list", "--count", "--left-right", &range])?;
    let mut counts = counts
        .split_whitespace()
        .map(|count| count.parse::<usize>().unwrap_or(usize::MAX));
    let behind = counts.next().unwrap_or(usize::MAX);
    let ahead = counts.next().unwrap_or(usize::MAX);
    if ahead > walk_max || behind > walk_max {
        let past = if past_paths {
            format!(
                "{} paths changed past the {INTEGRATED_PATHS_MAX}-path content comparison, and ",
                touched.len()
            )
        } else {
            String::new()
        };
        return Err(LaneError::Git {
            args: vec!["cherry".to_owned(), trunk.to_owned(), tip.to_owned()],
            exit_code: None,
            output: format!(
                "[not judged: {past}{ahead} commits on this branch and {behind} on {trunk} past the {walk_max}-commit patch-id walk; compare it with trunk by hand, or rebase it]"
            ),
        });
    }
    let cherry = git(repo, &["cherry", trunk, tip])?;
    let all_in = cherry.lines().count() > 0 && cherry.lines().all(|row| row.starts_with("- "));
    Ok(all_in.then_some(Rung::PatchId))
}

/// Uncommitted paths one status read named; past the capture cap the count is a floor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Uncommitted {
    paths: usize,
    cut: Option<(usize, usize)>,
}

impl Uncommitted {
    pub(crate) fn read(tree: &Path) -> Result<Self, LaneError> {
        Ok(Self::counted(&git_capture(tree, &STATUS)?))
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
    pub(super) fn abandoned(&self, slot: SlotIndex) -> Result<Option<Handback>, LaneError> {
        let path = self.slot_path(slot);
        if self.dirty(slot) {
            return Ok(None);
        }
        // Invariant: an unreadable HEAD is not a free slot; only a detached one is.
        let branch = match head(&path) {
            Ok(Head::Detached(_)) => return Ok(Some(Handback::Deleted)),
            Ok(Head::Branch(branch)) => branch,
            Err(_) => return Ok(None),
        };
        match self.land(branch.as_str())? {
            Handback::Deleted => Ok(Some(Handback::Deleted)),
            Handback::Kept(_) => Ok(None),
        }
    }

    /// Invariant: read-only, so the caller can ask before taking the lock a delete serializes on.
    pub(super) fn land(&self, branch: &str) -> Result<Handback, LaneError> {
        let base = self.resolve(&ClaimBase::Main, false)?;
        match integrated(&self.repo, branch, &base) {
            Ok(Some(_)) => Ok(Handback::Deleted),
            Ok(None) => Ok(Handback::Kept("unmerged".to_owned())),
            Err(why) => Ok(Handback::Kept(why.to_string())),
        }
    }

    /// Invariant: the worktree is detached before the branch goes, or git refuses the delete.
    pub(super) fn detach(
        &self,
        slot: SlotIndex,
        branch: Option<&str>,
        landed: Handback,
    ) -> Result<Handback, LaneError> {
        let path = self.slot_path(slot);
        let text = path.to_string_lossy().into_owned();
        let _ = git(&self.repo, &["worktree", "unlock", &text]);
        git(&path, &["checkout", "-q", "--detach"])?;
        match (branch, landed) {
            (Some(branch), Handback::Deleted) => {
                git(&self.repo, &["branch", "-q", "-D", branch])?;
                Ok(Handback::Deleted)
            }
            (Some(_), kept) | (None, kept) => Ok(kept),
        }
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
        let branch = self.branch_of(slot);
        let landed = match branch.as_deref() {
            Some(branch) => self.land(branch)?,
            None => Handback::Deleted,
        };
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
        let kept = self.detach(slot, branch.as_deref(), landed)?;
        let lost = self.wipe(slot)?.discarded();
        self.clear_session(slot)?;
        drop(guard);
        Ok(match (branch, kept) {
            (Some(branch), Handback::Kept(why)) => {
                format!("lane {slot} freed{lost}; branch {branch} kept ({why})")
            }
            (Some(branch), Handback::Deleted) => {
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
        let listing = git_capture(&self.repo, &["worktree", "list", "--porcelain"])?;
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
                "[worktree list cut: read {kept} of {total} bytes, OUTPUT_CAP {}; later worktrees not judged]",
                yi_tools::OUTPUT_CAP
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
                Ok(rung) => match (|| {
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
                    Ok(()) => format!("removed {shown} (merged: {rung})"),
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
    ) -> Result<Rung, String> {
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
        let rung = integrated(&self.repo, &tip, base)
            .map_err(|error| format!("{name}: {error}"))?
            .ok_or_else(|| format!("{name} is not merged into {base}"))?;
        if !status {
            return Ok(rung);
        }
        let dirty = Uncommitted::read(&tree.path).map_err(|e| format!("status unreadable: {e}"))?;
        match dirty.is_empty() {
            true => Ok(rung),
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
        let _rung = pool.judge(&tree, &base, &repo, false, true)?;

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

    #[test]
    fn the_walk_cap_holds_at_its_limit_and_is_loud_one_past()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("yi-gc-walk-cap-{}", std::process::id()));
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
        std::fs::write(repo.join("base.txt"), "base\n")?;
        git(&repo, &["add", "."])?;
        git(&repo, &["commit", "-qm", "base"])?;
        git(&repo, &["switch", "-qc", "work"])?;
        std::fs::write(repo.join("work.txt"), "work\n")?;
        git(&repo, &["add", "."])?;
        git(&repo, &["commit", "-qm", "work"])?;
        git(&repo, &["switch", "-q", "main"])?;
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "moved on"])?;
        git(&repo, &["cherry-pick", "work"])?;
        std::fs::write(repo.join("work.txt"), "changed since\n")?;
        git(&repo, &["commit", "-qam", "changed since"])?;
        assert_eq!(
            integrated_within(&repo, "work", "main", 3)?,
            Some(Rung::PatchId)
        );
        let past = integrated_within(&repo, "work", "main", 2)
            .err()
            .ok_or("no refusal past the cap")?
            .to_string();
        assert!(
            past.contains("1 commits on this branch and 3 on main past the 2-commit patch-id walk"),
            "{past}"
        );
        Ok(())
    }

    /// Dies with the path cap refusing the ladder: both caps are named when neither rung answers.
    #[test]
    fn a_path_list_past_its_cap_still_names_the_walk_that_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("yi-gc-path-cap-{}", std::process::id()));
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
        std::fs::write(repo.join("base.txt"), "base\n")?;
        git(&repo, &["add", "."])?;
        git(&repo, &["commit", "-qm", "base"])?;
        git(&repo, &["switch", "-qc", "wide"])?;
        for n in 0..1025 {
            std::fs::write(repo.join(format!("f{n:04}.txt")), format!("{n}\n"))?;
        }
        git(&repo, &["add", "."])?;
        git(&repo, &["commit", "-qm", "wide"])?;
        let past = integrated_within(&repo, "wide", "main", 0)
            .err()
            .ok_or("no refusal with both caps past")?
            .to_string();
        assert!(
            past.contains("1025 paths changed past the 1024-path content comparison"),
            "{past}"
        );
        assert!(past.contains("past the 0-commit patch-id walk"), "{past}");
        Ok(())
    }
}
