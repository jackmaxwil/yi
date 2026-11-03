//! The only two wipes of a slot's tree, a reap and a discard, the count of what each drops,
//! `yi gc`, and registrations of lanes trees a deleted lanes directory took.

use std::path::{Path, PathBuf};

use super::{
    ClaimBase, Head, LIST_BUDGET, LaneError, Pool, STATUS, Sha, SlotIndex, git, git_capture, head,
};

/// Uncommitted paths one status read named; past the capture cap the count is a floor, and
/// `submodules` is how many of them a gitlink read attributed, so a survivor can name its cause.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Uncommitted {
    paths: usize,
    cut: Option<(usize, usize)>,
    submodules: usize,
}

impl Uncommitted {
    pub(crate) fn read(tree: &Path, home: Option<&Path>) -> Result<Self, LaneError> {
        let status = git_capture(tree, &STATUS, &[], None, yi_tools::OUTPUT_CAP)?;
        Ok(Self::counted(&status).with_submodules(tree, home, None))
    }

    /// Invariant: host git never enters a submodule, whose config it cannot trust, so `status` is
    /// blind to work inside one. A listing reads the same count, so no lane is clean in one place.
    fn with_submodules(
        mut self,
        tree: &Path,
        home: Option<&Path>,
        until: Option<std::time::Instant>,
    ) -> Self {
        // One budget for the whole read: a hung git in one submodule must not stall every spawn.
        let until = until.or_else(|| std::time::Instant::now().checked_add(LIST_BUDGET));
        self.submodules = dirty_submodules(tree, until, home).unwrap_or(1);
        self.paths = self.paths.saturating_add(self.submodules);
        self
    }

    fn counted(status: &yi_tools::CommandCapture) -> Self {
        let (rows, cut) = (status.stdout.lines().count(), status.cut);
        // Invariant: a cut list is a head and a tail around a marker row; three lines may be no path.
        let paths = rows.saturating_sub(if cut.is_some() { 3 } else { 0 });
        Self {
            paths,
            cut,
            submodules: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.paths == 0 && self.cut.is_none()
    }

    /// What a wipe removed: the read before it less what the same read says survived it.
    fn without(&self, after: &Self) -> Self {
        Self {
            paths: self.paths.saturating_sub(after.paths),
            cut: self.cut,
            submodules: after.submodules.min(self.submodules),
        }
    }

    /// A survivor says what held it, never a cause nothing read: a gitlink count is what a
    /// checkout cannot reach, and the rest is in place because `clean -fd` left it there.
    fn described(&self) -> String {
        let inside = self.submodules.min(self.paths);
        let outside = self.paths.saturating_sub(inside);
        match (inside, outside) {
            (0, _) => format!("{self} kept in place, outside git's reach"),
            (_, 0) => format!("{self} kept inside a submodule"),
            (_, _) => format!("{self} kept: {inside} inside a submodule, {outside} in place"),
        }
    }

    fn discarded(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!("; {self} discarded")
    }
}

/// What a wipe removed and what it could not reach: git never enters a submodule without being
/// asked, so its worktree holds edits the parent's checkout and clean leave in place.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wiped {
    lost: Uncommitted,
    kept: Uncommitted,
}

impl Wiped {
    /// The sentence a reap or a discard adds: the paths it dropped, and the ones that survived.
    pub fn discarded(&self) -> String {
        let kept = if self.kept.is_empty() {
            String::new()
        } else {
            format!("; {}", self.kept.described())
        };
        format!("{}{kept}", self.lost.discarded())
    }
}

/// The count a status read adds for the gitlinks holding work, one path for an unreadable index:
/// the claim decision and a listing take it from here, so one budget and one default decide both.
pub(crate) fn submodule_paths(
    paths: usize,
    tree: &Path,
    until: Option<std::time::Instant>,
    home: Option<&Path>,
) -> usize {
    paths.saturating_add(dirty_submodules(tree, until, home).unwrap_or(1))
}

/// The populated gitlinks with work inside them, `None` when the index cannot be read; the claim
/// decision and a listing both read it, so one answer exists for whether a lane is dirty.
pub(crate) fn dirty_submodules(
    tree: &Path,
    until: Option<std::time::Instant>,
    home: Option<&Path>,
) -> Option<usize> {
    let stage = super::hostgit::git_capped(
        tree,
        &["ls-files", "--stage", "-z"],
        until,
        super::PLUMBING_CAP,
    )
    .ok()?;
    let links: Vec<PathBuf> = (stage.split('\0'))
        .filter_map(|row| row.split_once('\t'))
        .filter(|(meta, _)| meta.starts_with("160000 "))
        .filter_map(|(_, path)| gitlink(tree, path))
        .filter(|dir| std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some()))
        .collect();
    // Invariant: a tree with no populated gitlink pays one index read, never a profile build.
    if links.is_empty() {
        return Some(0);
    }
    // Invariant: one builder for a lane's profile, from the home the pool holds; a caller with no
    // pool reads the host's own, so a seal added to `profile` reaches a submodule read too.
    let home = home
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    let sandbox = home
        .as_deref()
        .and_then(|home| super::toolchain::profile(tree, home));
    let links =
        (links.iter()).filter(|dir| !submodule_is_clean(tree, dir, until, sandbox.as_ref()));
    Some(links.count())
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
        !Uncommitted::read(&self.slot_path(slot), Some(&self.home))
            .is_ok_and(|tree| tree.is_empty())
    }

    /// Invariant: the only wipe of a session's work; a claim checks out only over a clean tree.
    pub(super) fn wipe(&self, slot: SlotIndex) -> Result<Wiped, LaneError> {
        let path = self.slot_path(slot);
        let lost = Uncommitted::read(&path, Some(&self.home))?;
        // Nothing to drop: the answer is the same, and a clean hand-back pays no confirming read.
        if lost.is_empty() {
            return Ok(Wiped {
                lost,
                kept: Uncommitted::default(),
            });
        }
        git(&path, &["checkout", "-q", "-f"])?;
        git(&path, &["clean", "-qfd"])?;
        // What the same read says now: the paths a wipe leaves are the ones it did not drop, and a
        // tree it cannot read is not one it emptied, so the failure is the caller's, never a zero.
        let kept = Uncommitted::read(&path, Some(&self.home))?;
        Ok(Wiped {
            lost: lost.without(&kept),
            kept,
        })
    }

    /// Incident: a deleted lanes directory left its slots registered, and `worktree add` refused
    /// a branch no live tree holds.
    pub(super) fn rebuild(&self, add: &[&str]) -> Result<(), LaneError> {
        let listing = git_capture(&self.repo, &["worktree", "list", "--porcelain"])?;
        for tree in parse(&listing.stdout) {
            if tree.path.starts_with(&self.dir) && !tree.path.exists() {
                let text = tree.path.to_string_lossy();
                let _ = git(&self.repo, &["worktree", "remove", "-f", "-f", &text]);
            }
        }
        git(&self.repo, add).map(|_| ())
    }
}

/// A gitlink's working tree, never a path out of it: the index sits in a lane's contained-writable
/// admin dir, so an entry naming `..` or an absolute directory would point host code at another tree.
fn gitlink(tree: &Path, path: &str) -> Option<PathBuf> {
    let rel = Path::new(path);
    let normal = |part: std::path::Component<'_>| matches!(part, std::path::Component::Normal(_));
    if rel.is_absolute() || !rel.components().all(normal) {
        return None;
    }
    Some(tree.join(rel))
}

/// `status` under the tree's own profile, so a clean filter a child planted runs contained, and
/// without Seatbelt still with hooks and fsmonitor off. A dir with no `.git` is never clean.
fn submodule_is_clean(
    tree: &Path,
    dir: &Path,
    until: Option<std::time::Instant>,
    sandbox: Option<&yi_tools::Sandbox>,
) -> bool {
    let Some(until) = until.filter(|at| std::time::Instant::now() < *at) else {
        return false;
    };
    let Ok(path) = dir.strip_prefix(tree) else {
        return false;
    };
    if !dir.join(".git").exists() {
        return false;
    }
    // The flags the parent's own read carries, from the constant that holds them.
    let argv: Vec<String> = [
        "git".to_owned(),
        "-C".to_owned(),
        path.to_string_lossy().into_owned(),
    ]
    .into_iter()
    .chain(STATUS.iter().map(|arg| (*arg).to_owned()))
    .collect();
    let named: Vec<&str> = argv.iter().map(String::as_str).collect();
    let Some((program, args)) = super::toolchain::wrapped(&named, sandbox) else {
        return false;
    };
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    super::capture_named(
        tree,
        "git status in a submodule",
        &program,
        &borrowed,
        until.saturating_duration_since(std::time::Instant::now()),
        yi_tools::OUTPUT_CAP,
        sandbox,
    )
    .is_ok_and(|capture| capture.stdout.trim().is_empty())
}

pub(super) struct Tree {
    pub(super) path: PathBuf,
    locked: bool,
    pub(super) branch: Option<String>,
}

pub(super) fn parse(listing: &str) -> Vec<Tree> {
    let mut trees = Vec::new();
    for block in listing.split("\n\n") {
        let mut tree = Tree {
            path: PathBuf::new(),
            locked: false,
            branch: None,
        };
        for line in block.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                tree.path = PathBuf::from(path);
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                tree.branch = Some(branch.to_owned());
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
            &[],
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
        let dirty = Uncommitted::read(&tree.path, Some(&self.home))
            .map_err(|e| format!("status unreadable: {e}"))?;
        match dirty.is_empty() {
            true => Ok(()),
            false => Err(dirty.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Dies with an index entry naming a path out of the tree: the admin dir that holds the index is
    /// contained-writable, so `..` or an absolute path would point host code at another tree.
    #[test]
    fn a_gitlink_outside_the_tree_is_no_link() {
        let tree = Path::new("/lane");
        assert_eq!(gitlink(tree, "sub"), Some(PathBuf::from("/lane/sub")));
        assert_eq!(gitlink(tree, "../other"), None);
        assert_eq!(gitlink(tree, "/etc"), None);
        assert_eq!(gitlink(tree, "a/../b"), None);
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

    /// Dies with a populated submodule counted dirty wherever no sandbox profile exists: the read
    /// falls back to the lane's own runner, one directory down, and must still see work inside it.
    #[test]
    fn a_submodule_read_with_no_profile_sees_the_work_inside_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let until = || Some(std::time::Instant::now() + LIST_BUDGET);
        let root = std::env::temp_dir().join(format!("yi-submodule-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (tree, sub) = (root.join("tree"), root.join("tree/sub"));
        std::fs::create_dir_all(&sub)?;
        for (dir, args) in [
            (&tree, &["init", "-q", "-b", "main"][..]),
            (&sub, &["init", "-q", "-b", "main"][..]),
        ] {
            git(dir, args)?;
            git(dir, &["config", "user.email", "lanes@test"])?;
            git(dir, &["config", "user.name", "lanes"])?;
        }
        std::fs::write(sub.join("file.txt"), "one\n")?;
        git(&sub, &["add", "."])?;
        git(&sub, &["commit", "-qm", "first"])?;

        assert!(
            submodule_is_clean(&tree, &tree.join("sub"), until(), None),
            "clean reads clean"
        );
        assert!(
            !submodule_is_clean(&tree, &tree.join("no-such-link"), until(), None),
            "no .git is never clean"
        );
        git(&sub, &["config", "status.showUntrackedFiles", "no"])?;
        std::fs::write(sub.join("new.txt"), "work\n")?;
        assert!(
            !submodule_is_clean(&tree, &tree.join("sub"), until(), None),
            "a config cannot hide work"
        );
        Ok(())
    }
}
