//! A worktree child's tree: a linked worktree under `~/.yi/forks`, cut at the parent's HEAD and
//! filled by the cheapest copy this filesystem has. Lanes stay for root sessions and verification.

#[cfg(target_os = "macos")]
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use yi_types::lane::SlotState;

use super::hostgit::{candidate_of, git, git_capped};
use super::settle::{Candidate, Quiescence};
use super::{BranchName, LIST_BUDGET, LaneError, PLUMBING_CAP, Pool, Sha, io_error};

const SCRATCH_ROWS_MAX: usize = 3;
const SWEEP_QUEUE: usize = 256;

/// What git said about the parent's tree before a fork: what the child will not have.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParentState {
    /// Uncommitted tracked and untracked paths, left behind as the commit-cut says.
    pub left_out: usize,
    /// Untracked and ignored paths, a directory ending in `/`: skipped by the copy.
    pub skipped: Vec<String>,
    /// The ignored ones alone: what `.worktreeinclude` may name and a row reports.
    pub ignored: Vec<String>,
    pub tracked_dirt: bool,
}

/// `git status --porcelain -z --ignored`. `None` where a path came through lossy, since a skip list
/// with a replaced byte names nothing on disk.
#[must_use]
pub fn parent_state(status: &str) -> Option<ParentState> {
    if status.contains('\u{fffd}') {
        return None;
    }
    let mut state = ParentState::default();
    let mut fields = status.split('\0').filter(|field| !field.is_empty());
    while let Some(row) = fields.next() {
        let (code, path) = (row.get(..2)?, row.get(3..)?);
        match code {
            "!!" => {
                state.skipped.push(path.to_owned());
                state.ignored.push(path.to_owned());
            }
            "??" => {
                state.skipped.push(path.to_owned());
                state.left_out = state.left_out.saturating_add(1);
            }
            _ => {
                state.tracked_dirt = true;
                state.left_out = state.left_out.saturating_add(1);
                if code.starts_with('R') || code.starts_with('C') {
                    let _the_old_name = fields.next();
                }
            }
        }
    }
    Some(state)
}

/// What a fork brings besides the commit: ignored files, and whether the host owes first-use work.
/// A container's tree owes none, since a Mac's units are wrong in an image.
#[derive(Debug, Default)]
pub struct Carry {
    pub extra: Vec<String>,
    pub denied: Vec<PathBuf>,
    pub first_use: bool,
}

/// Before a fresh tree's first command: the cargo units that name nothing in it, the toolchain, and
/// the one setup script a repository committed. Each failure is a row: the child can still rebuild.
fn first_use(parent: &Path, repo: &Path, tree: &Path, home: &Path, id: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let built = [parent, repo]
        .into_iter()
        .find(|at| at.join("target/debug").is_dir());
    match super::carry::carry_cargo(built.unwrap_or(parent), tree, id) {
        super::carry::Carried::Units(units) => {
            rows.push(format!(
                "carried {units} third-party cargo units into the child"
            ));
        }
        super::carry::Carried::Refused(row) => rows.push(row),
        super::carry::Carried::Nothing => {}
    }
    let toolchains = super::toolchain::detect(tree);
    if let Err(error) = super::toolchain::sync_tree(tree, home, &toolchains) {
        rows.push(format!("[toolchain sync failed: {error}]"));
    }
    rows.extend(run_setup(tree, home));
    rows
}

/// Runs contained and only as committed: an edit since would be a child's code in the host's name.
fn run_setup(tree: &Path, home: &Path) -> Option<String> {
    const SETUP: &str = ".yi/setup";
    let script = tree.join(SETUP);
    if !script.is_file() {
        return None;
    }
    if !crate::ext::project::committed(tree, &script, SETUP) {
        return Some(format!("[{SETUP} skipped: it differs from HEAD]"));
    }
    if !yi_tools::Sandbox::available() {
        return Some(format!("[{SETUP} skipped: no sandbox to run it in]"));
    }
    let (program, args, sandbox) = super::toolchain::contained(tree, home, &["sh", SETUP])?;
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let timeout = Duration::from_millis(super::toolchain::SYNC_TIMEOUT_MS);
    let cap = yi_tools::OUTPUT_CAP;
    let ran = super::capture_named(
        tree,
        SETUP,
        &program,
        &borrowed,
        timeout,
        cap,
        sandbox.as_ref(),
    );
    ran.err().map(|error| format!("[{SETUP} failed: {error}]"))
}

/// A child's tree, held by the flock on `<id>.held`; `<id>.json` is the record, beside it so a
/// rename of the record never swaps the inode under the lock.
#[must_use = "dropping a child's tree keeps its work on disk for retention, unmerged"]
#[derive(Debug)]
pub struct ChildTree {
    path: PathBuf,
    branch: BranchName,
    base: Sha,
    pool: Pool,
    session: String,
    _held: File,
}

impl ChildTree {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    pub fn base(&self) -> &Sha {
        &self.base
    }

    /// What a host needs to read the candidate without the tree.
    pub fn checkout(&self) -> (PathBuf, String, BranchName, Option<Sha>) {
        (
            self.path.clone(),
            self.session.clone(),
            self.branch.clone(),
            Some(self.base.clone()),
        )
    }

    /// Cut at `parent`'s HEAD: its dirt stays there, or carrying it would commit a person's edits
    /// as the child's. Returns the tree and what the parent's model should hear.
    pub fn fork(
        pool: &Pool,
        parent: &Path,
        child_id: &str,
        plan: &Carry,
    ) -> Result<(Self, Vec<String>), LaneError> {
        let head = git(parent, &["rev-parse", "--verify", "HEAD^{commit}"])?;
        // Status paths are the repository's and a session may sit in a subdirectory of it.
        let top = git(parent, &["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(top.trim_end());
        let parent = top.as_path();
        let base =
            Sha::parse(&head).ok_or_else(|| LaneError::Forge(format!("HEAD is no sha: {head}")))?;
        let until = Instant::now().checked_add(LIST_BUDGET);
        let args = ["status", "--porcelain", "-z", "--ignored"];
        let status = git_capped(parent, &args, until, PLUMBING_CAP);
        let state = status.as_ref().ok().and_then(|status| parent_state(status));
        let forks = forks_dir(pool.home());
        std::fs::create_dir_all(&forks).map_err(io_error(&forks))?;
        let path = forks.join(child_id);
        let held_path = forks.join(format!("{child_id}.held"));
        let held = yi_session::lock_file(&held_path).map_err(io_error(&held_path))?;
        let record = SlotState {
            base: Some(base.as_str().to_owned()),
            session: Some(child_id.to_owned()),
            ..SlotState::default()
        };
        let bytes =
            serde_json::to_vec(&record).map_err(|error| LaneError::Forge(error.to_string()))?;
        let record_path = path.with_extension("json");
        yi_session::replace_file(&record_path, &bytes).map_err(io_error(&record_path))?;
        let branch = BranchName::for_session(child_id)?;
        let text = path.to_string_lossy().into_owned();
        let reason = format!("session:{child_id}");
        let add = [
            "worktree",
            "add",
            "-q",
            "--no-checkout",
            "--lock",
            "--reason",
            &reason,
            "-b",
        ];
        let mut args: Vec<&str> = add.to_vec();
        args.extend([branch.as_str(), &text, base.as_str()]);
        git(pool.repo(), &args)?;
        let tree = Self {
            path,
            branch,
            base,
            pool: pool.clone(),
            session: child_id.to_owned(),
            _held: held,
        };
        let mut rows = Vec::new();
        let cloned = state
            .as_ref()
            .filter(|_| !parent.join(".gitmodules").exists())
            .is_some_and(|state| cheap_copy(parent, &tree, state).is_ok());
        if !cloned {
            tree.plain_checkout()?;
        }
        let ignored = state
            .as_ref()
            .map_or(&[][..], |state| state.ignored.as_slice());
        rows.extend(super::carry::carry_includes(
            parent,
            tree.path(),
            ignored,
            &plan.extra,
            &plan.denied,
        ));
        rows.extend(
            state
                .as_ref()
                .map(|state| left_out_row(state, &tree))
                .unwrap_or_default(),
        );
        if plan.first_use {
            let (from, repo) = (parent.to_path_buf(), pool.repo().to_path_buf());
            let (at, home, id) = (
                tree.path().to_path_buf(),
                pool.home().to_path_buf(),
                child_id.to_owned(),
            );
            yi_tools::register_first_use(
                tree.path(),
                Box::new(move || first_use(&from, &repo, &at, &home, &id)),
            );
        }
        if state.is_none() {
            let why = status.err().map_or_else(
                || "it named a path that is not UTF-8".to_owned(),
                |error| error.to_string(),
            );
            rows.push(format!(
                "the child starts at HEAD {}; uncommitted paths not counted: {why}",
                tree.base.short()
            ));
        }
        Ok((tree, rows))
    }

    /// The rung every platform has, and what a failed clone falls back to over its leavings.
    fn plain_checkout(&self) -> Result<(), LaneError> {
        git(&self.path, &["clean", "-qfdx"])?;
        git(&self.path, &["reset", "-q", "--hard", self.base.as_str()]).map(drop)
    }

    pub fn candidate(
        &self,
        quiescence: &Quiescence,
        deadline: Option<Instant>,
    ) -> Result<Candidate, LaneError> {
        let _gone_already_is_fine =
            std::fs::remove_dir_all(super::carry::temp_of(&self.path, &self.session));
        candidate_of(
            &self.path,
            &self.session,
            &self.branch,
            Some(self.base.clone()),
            quiescence,
            deadline,
        )
    }

    /// The work committed on its branch, then the tree retired. A refusal before that hands the
    /// tree back, so a busy or late settle never loses it.
    pub fn settle(
        self,
        quiescence: &Quiescence,
        deadline: Option<Instant>,
    ) -> Result<SettledTree, (Box<Self>, LaneError)> {
        let candidate = match self.candidate(quiescence, deadline) {
            Ok(candidate) => candidate,
            Err(error) => return Err((Box::new(self), error)),
        };
        let pool = self.pool.clone();
        let unchanged = candidate.commit == self.base;
        self.retire();
        if unchanged {
            let branch = candidate.branch.as_str();
            let _gone_already_is_fine = git(pool.repo(), &["branch", "-q", "-D", branch]);
        }
        Ok(SettledTree { candidate, pool })
    }

    /// Commit point: the rename, after which nothing reads this path. `--lock` kept `Pool::gc` off
    /// until here.
    fn retire(self) {
        yi_tools::forget_first_use(&self.path);
        retire_at(&self.pool, &self.path);
    }
}

fn retire_at(pool: &Pool, path: &Path) {
    let text = path.to_string_lossy().into_owned();
    let _gone_already_is_fine = git(pool.repo(), &["worktree", "unlock", &text]);
    let tomb = path.with_extension("gone");
    if std::fs::rename(path, &tomb).is_ok() {
        let _the_next_sweep_prunes = git(pool.repo(), &["worktree", "prune", "--expire=now"]);
        sweep_later(tomb);
    }
    for leftover in [path.with_extension("json"), path.with_extension("held")] {
        let _gone_already_is_fine = std::fs::remove_file(leftover);
    }
}

/// A tree of a dead holder, work committed on its branch first so it survives, then retired.
pub fn reclaim(home: &Path, tree: &Path) -> bool {
    let Ok(pool) = Pool::open(home, tree, 1) else {
        return false;
    };
    let Ok(bytes) = std::fs::read(tree.with_extension("json")) else {
        return false;
    };
    let Ok(record) = serde_json::from_slice::<SlotState>(&bytes) else {
        return false;
    };
    let (Some(session), Some(base)) = (record.session, record.base.as_deref().and_then(Sha::parse))
    else {
        return false;
    };
    let Ok(branch) = BranchName::for_session(&session) else {
        return false;
    };
    let quiet = Quiescence {
        at: 0,
        running: Vec::new(),
    };
    let _gone_already_is_fine = std::fs::remove_dir_all(super::carry::temp_of(tree, &session));
    if candidate_of(tree, &session, &branch, Some(base), &quiet, None).is_err() {
        return false;
    }
    retire_at(&pool, tree);
    true
}

/// `<id>.held` is held by a live process.
#[must_use]
pub fn is_held(tree: &Path) -> bool {
    let held = tree.with_extension("held");
    !matches!(yi_session::try_lock_file(&held), Ok(Ok(_)))
}

#[must_use]
pub fn forks_dir(home: &Path) -> PathBuf {
    home.join(".yi/forks")
}

/// A full queue leaves the tomb for retention, so nothing waits without bound.
fn sweep_later(tomb: PathBuf) {
    static SWEEPER: OnceLock<Mutex<Option<SyncSender<PathBuf>>>> = OnceLock::new();
    let sender = SWEEPER.get_or_init(|| {
        let (send, receive) = sync_channel::<PathBuf>(SWEEP_QUEUE);
        let spawned = std::thread::Builder::new()
            .name("yi-fork-sweep".to_owned())
            .spawn(move || {
                for tomb in receive {
                    let _the_next_sweep_retries = std::fs::remove_dir_all(tomb);
                }
            });
        Mutex::new(spawned.ok().map(|_| send))
    });
    if let Ok(sender) = sender.lock()
        && let Some(sender) = &*sender
    {
        let _retention_finishes_a_full_queue = sender.try_send(tomb);
    }
}

/// `settle`'s result: the branch and what it holds. The tree is gone, the branch stays.
#[derive(Debug)]
pub struct SettledTree {
    pub candidate: Candidate,
    pool: Pool,
}

impl SettledTree {
    /// The `Discarded` path: the branch goes too.
    pub fn discard(self) -> Result<Candidate, LaneError> {
        if self.pool.branch_exists(&self.candidate.branch)? {
            git(
                self.pool.repo(),
                &["branch", "-q", "-D", self.candidate.branch.as_str()],
            )?;
        }
        Ok(self.candidate)
    }
}

/// The scratch files the child lacks, named so the model can ask for one back.
fn left_out_row(state: &ParentState, tree: &ChildTree) -> Vec<String> {
    let short = tree.base.short();
    let mut rows = Vec::new();
    if state.left_out > 0 {
        rows.push(format!(
            "the child starts at HEAD {short}, without {} uncommitted paths in your checkout: pass them as local:// addresses in its prompt",
            state.left_out
        ));
    }
    let rebuilt = [
        "target/",
        "node_modules/",
        ".venv/",
        "__pycache__/",
        "dist/",
        "build/",
    ];
    let scratch: Vec<&String> = (state.ignored.iter())
        .filter(|path| !rebuilt.iter().any(|name| path.ends_with(name)))
        .filter(|path| !tree.path().join(path.as_str()).exists())
        .collect();
    if !scratch.is_empty() {
        let named: Vec<&str> = (scratch.iter().take(SCRATCH_ROWS_MAX))
            .map(|path| path.as_str())
            .collect();
        rows.push(format!(
            "ignored and not in the child: {}",
            named.join(", ")
        ));
        if scratch.len() > SCRATCH_ROWS_MAX {
            rows.push(yi_tools::cut_row(
                SCRATCH_ROWS_MAX,
                scratch.len(),
                "ignored paths named",
                "fork.scratch_rows",
                SCRATCH_ROWS_MAX,
                "git status --ignored",
            ));
        }
        rows.push("`carry=[\".env\"]` on rlm.run copies one".to_owned());
    }
    rows
}

#[cfg(target_os = "macos")]
fn cheap_copy(parent: &Path, tree: &ChildTree, state: &ParentState) -> Result<(), LaneError> {
    let skipped: HashSet<PathBuf> = (state.skipped.iter())
        .map(|path| PathBuf::from(path.trim_end_matches('/')))
        .collect();
    clone_walk(parent, tree.path(), &skipped).map_err(io_error(tree.path()))?;
    let copy_index = || -> std::io::Result<()> {
        let admin = |at: &Path| yi_permission::git_dirs(at).into_iter().next();
        let (Some(from), Some(to)) = (admin(parent), admin(tree.path())) else {
            return Err(std::io::Error::other("no git dir"));
        };
        std::fs::copy(from.join("index"), to.join("index")).map(drop)
    };
    copy_index().map_err(io_error(tree.path()))?;
    // Exit 1 is "something needs updating", which the reset below answers.
    let _stat_refresh = git(tree.path(), &["update-index", "-q", "--refresh"]);
    if state.tracked_dirt {
        git(tree.path(), &["reset", "-q", "--hard", tree.base.as_str()])?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn cheap_copy(_parent: &Path, _tree: &ChildTree, _state: &ParentState) -> Result<(), LaneError> {
    Err(LaneError::Forge(
        "no cheap copy on this platform".to_owned(),
    ))
}

/// Depth-first on an explicit stack, since the parent's tree is written by a process that wants a
/// deep one. A whole directory clones in one call when nothing skipped lies under it.
#[cfg(target_os = "macos")]
fn clone_walk(parent: &Path, into: &Path, skipped: &HashSet<PathBuf>) -> std::io::Result<()> {
    use rustix::fs::{CWD, CloneFlags, Dir, FileType, Mode, OFlags, fclonefileat, fstat, openat};
    const DEPTH_MAX: usize = 256;
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let holds_skipped = |rel: &Path| skipped.iter().any(|path| path.starts_with(rel));
    let dest = openat(
        CWD,
        into,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let root = openat(CWD, parent, flags | OFlags::DIRECTORY, Mode::empty())?;
    let mut stack = vec![(root, dest, PathBuf::new(), 0usize)];
    while let Some((from, to, rel, depth)) = stack.pop() {
        if depth > DEPTH_MAX {
            return Err(std::io::Error::other("a tree deeper than the fork walks"));
        }
        let names: Vec<std::ffi::CString> = Dir::read_from(&from)?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_owned())
            .filter(|name| name.to_bytes() != b"." && name.to_bytes() != b"..")
            .collect();
        for name in names {
            let child = rel.join(std::ffi::OsStr::new(&*name.to_string_lossy()));
            if (rel.as_os_str().is_empty() && name.to_bytes() == b".git")
                || skipped.contains(&child)
            {
                continue;
            }
            let opened = openat(&from, name.as_c_str(), flags, Mode::empty());
            let Ok(fd) = opened else {
                // A symlink: recreated as a link, never followed.
                let target = rustix::fs::readlinkat(&from, name.as_c_str(), Vec::new())?;
                symlink_at(&target, &to, &name)?;
                continue;
            };
            let kind = FileType::from_raw_mode(fstat(&fd)?.st_mode);
            match kind {
                FileType::Directory if holds_skipped(&child) => {
                    rustix::fs::mkdirat(&to, name.as_c_str(), Mode::from_raw_mode(0o755))?;
                    let below = openat(
                        &to,
                        name.as_c_str(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                        Mode::empty(),
                    )?;
                    stack.push((fd, below, child, depth.saturating_add(1)));
                }
                FileType::Directory | FileType::RegularFile => {
                    fclonefileat(&fd, &to, name.as_c_str(), CloneFlags::NOOWNERCOPY)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn symlink_at(
    target: &std::ffi::CStr,
    dir: &std::os::fd::OwnedFd,
    name: &std::ffi::CStr,
) -> std::io::Result<()> {
    rustix::fs::symlinkat(target, dir, name).map_err(Into::into)
}
