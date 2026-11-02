//! Pooled git worktrees a session works in, and the hand-back ladder out of them.
//! A root session never edits the trunk checkout; it claims a slot.

use std::fs::File;
use std::path::{Path, PathBuf};

use yi_types::lane::SlotState;

use reap::{Uncommitted, Wiped};

pub(crate) use hostgit::{PLUMBING_CAP, capture, capture_named, git, git_capture};
pub use names::{BranchName, Head, Sha};

pub mod carry;
pub mod fork;
pub mod handback;
pub(crate) mod hostgit;
pub mod land;
mod names;
pub mod reap;
pub mod remote;
pub mod settle;
pub mod stage;
pub mod toolchain;

const GIT_TIMEOUT_MS: u64 = 120_000;
/// Incident: three slots refused a console's fourth session, new or resumed. Unset, the
/// pool grows at the first missing slot and keeps it, so it settles at peak concurrency.
pub const DEFAULT_SLOTS: u8 = u8::MAX;
const FETCH_FRESH_MS: u64 = 60_000;

static FETCH_FORBIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `--faux`: a claim starts from the `origin/main` already fetched and never dials the
/// remote, which over ssh no proxy would see (#943).
pub fn forbid_fetch() {
    FETCH_FORBIDDEN.store(true, std::sync::atomic::Ordering::Relaxed);
}
const WARMER_EXIT_WAIT_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SlotIndex(u8);

impl SlotIndex {
    pub fn get(self) -> u8 {
        self.0
    }
}

impl std::fmt::Display for SlotIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::str::FromStr for SlotIndex {
    type Err = std::num::ParseIntError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.trim().parse().map(Self)
    }
}

const HEAD_MAX_BYTES: u64 = 256;

fn read_bounded(path: &Path) -> Result<String, LaneError> {
    use std::io::Read;
    let mut text = String::new();
    File::open(path)
        .and_then(|file| file.take(HEAD_MAX_BYTES).read_to_string(&mut text))
        .map_err(io_error(path))?;
    Ok(text)
}

pub fn head(cwd: &Path) -> Result<Head, LaneError> {
    let path = yi_permission::git_dirs(cwd)
        .into_iter()
        .next()
        .ok_or_else(|| LaneError::NotARepo(cwd.to_path_buf()))?
        .join("HEAD");
    let text = read_bounded(&path)?;
    let text = text.trim();
    if let Some(name) = text.strip_prefix("ref: refs/heads/") {
        return BranchName::parse(name).map(Head::Branch);
    }
    Sha::parse(text).map(Head::Detached).ok_or(LaneError::Head {
        path,
        reason: "neither a branch ref nor a commit sha",
    })
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LaneError {
    #[error("no free lane: {held} held, {orphans} orphaned of {slots}; `yi lanes` lists them")]
    PoolFull { slots: u8, held: u8, orphans: u8 },
    #[error("lane {slot} is held by session {session}")]
    SlotBusy { slot: SlotIndex, session: String },
    #[error("lane {slot} was left by session {session}; `yi lanes reap {slot}` frees it")]
    Orphan { slot: SlotIndex, session: String },
    #[error("lane {slot} changed under the prompt: expected {expected}, found {found:?}")]
    SlotChanged {
        slot: SlotIndex,
        expected: String,
        found: String,
    },
    #[error("branch {text:?} is not a git ref: {reason}")]
    Branch { text: String, reason: &'static str },
    #[error("lockfile {hash} was never synced by a session, so the warmer refuses it")]
    LockfileUnseen { hash: String },
    #[error("git {} {}: {output}", args.join(" "), exit_code.map_or_else(|| "failed".to_owned(), |code| format!("exited {code}")))]
    Git {
        args: Vec<String>,
        exit_code: Option<i32>,
        output: String,
    },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    State {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("git {}: output cut at {kept} of {total} bytes (lane.plumbing_cap {cap}); nothing read past it", args.join(" "))]
    Cut {
        args: Vec<String>,
        kept: usize,
        total: usize,
        cap: usize,
    },
    #[error(
        "HEAD names {head}, trunk or a branch checked out elsewhere; the host commits nothing there"
    )]
    ForeignHead { head: String },
    #[error("{0} is not inside a git repository")]
    NotARepo(PathBuf),
    #[error("{path}: HEAD unreadable: {reason}")]
    Head { path: PathBuf, reason: &'static str },
    #[error("lane {slot}'s warmer (pid {pid}) did not exit within {waited_ms} ms")]
    WarmerStuck {
        slot: SlotIndex,
        pid: u32,
        waited_ms: u64,
    },
    #[error("lane {slot} belonged to a repository deleted since; {} keeps its uncommitted files: copy them out and delete it, and the next claim makes a fresh lane there", path.display())]
    RepoGone { slot: SlotIndex, path: PathBuf },
    #[error("no forge: the repository has no `origin` remote")]
    NoForge,
    #[error("the lane is not quiescent: {} still running: {}", running.len(), running.join("; "))]
    Busy { running: Vec<String> },
    #[error("the run's deadline passed before the lane settled")]
    Deadline,
    #[error("{path}: HEAD is detached, so there is no branch to publish onto")]
    Unpublishable { path: PathBuf },
    #[error("{0}")]
    Forge(String),
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> LaneError + '_ {
    move |source| LaneError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimBase {
    Main,
    Commit(String),
}

/// What a slot's tree holds against `main`, or which read did not answer in time.
/// Invariant: `Unread` never renders as clean; a listing that cannot see a tree says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeState {
    Known {
        modified: u32,
        untracked: u32,
        ahead: u32,
        behind: u32,
    },
    Unread {
        step: &'static str,
    },
}

impl TreeState {
    /// Nothing in the tree and nothing `main` lacks: the next claim takes the slot (D130).
    pub fn is_empty(&self) -> bool {
        matches!(
            self,
            Self::Known {
                modified: 0,
                untracked: 0,
                ahead: 0,
                ..
            }
        )
    }
}

/// The session on a held or left slot, and what its tree holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub session: String,
    pub branch: Option<BranchName>,
    pub tree: TreeState,
    /// Since the branch was created, from its reflog; `None` when detached or unread.
    pub age: Option<std::time::Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotView {
    Idle {
        slot: SlotIndex,
        path: PathBuf,
        base: Option<String>,
        warm: bool,
    },
    Held {
        slot: SlotIndex,
        path: PathBuf,
        holder: Holder,
    },
    Orphan {
        slot: SlotIndex,
        path: PathBuf,
        holder: Holder,
    },
}

impl SlotView {
    pub fn slot(&self) -> SlotIndex {
        match self {
            Self::Idle { slot, .. } | Self::Held { slot, .. } | Self::Orphan { slot, .. } => *slot,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Idle { path, .. } | Self::Held { path, .. } | Self::Orphan { path, .. } => path,
        }
    }
}

/// Incident: `status.showUntrackedFiles=no` in a user's config read a tree of new files as clean.
/// No optional locks: an unlocked listing's status must not take the index lock from a claim.
pub(crate) const STATUS: [&str; 4] = [
    "--no-optional-locks",
    "status",
    "--porcelain",
    "--untracked-files=normal",
];

/// One deadline for a whole listing: every read past it is `Unread`, never a wait.
const LIST_BUDGET: std::time::Duration = std::time::Duration::from_millis(3_000);

fn probe_git(cwd: &Path, args: &[&str], deadline: std::time::Instant) -> Result<String, String> {
    capture(
        cwd,
        "git",
        args,
        deadline.saturating_duration_since(std::time::Instant::now()),
    )
}

fn count(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok()
}

fn tree_state(
    path: &Path,
    status: Result<String, String>,
    base: Option<&Sha>,
    deadline: std::time::Instant,
    home: &Path,
) -> TreeState {
    let Ok(status) = status else {
        return TreeState::Unread { step: "status" };
    };
    let (mut modified, mut untracked) = (0_u32, 0_u32);
    for line in status.lines() {
        if line.starts_with("??") {
            untracked = untracked.saturating_add(1);
        } else {
            modified = modified.saturating_add(1);
        }
    }
    // The same count `Uncommitted` reads, through the one helper that keeps its budget and its
    // unreadable-index default: a lane is never clean in a listing and dirty to its claim.
    let dirty = reap::submodule_paths(0, path, Some(deadline), Some(home));
    modified = modified.saturating_add(u32::try_from(dirty).unwrap_or(u32::MAX));
    let Some(base) = base else {
        return TreeState::Unread { step: "main" };
    };
    let spec = format!("{}...HEAD", base.as_str());
    let counts = probe_git(
        path,
        &["rev-list", "--left-right", "--count", &spec],
        deadline,
    );
    let Ok(counts) = counts else {
        return TreeState::Unread { step: "rev-list" };
    };
    let mut parts = counts.split_whitespace();
    match (parts.next().and_then(count), parts.next().and_then(count)) {
        (Some(behind), Some(ahead)) => TreeState::Known {
            modified,
            untracked,
            ahead,
            behind,
        },
        _ => TreeState::Unread { step: "rev-list" },
    }
}

/// The branch's creation from its reflog's oldest entry (`…@{<unix>}: branch: Created`).
fn age_of(
    path: &Path,
    branch: &BranchName,
    deadline: std::time::Instant,
) -> Option<std::time::Duration> {
    let refname = format!("refs/heads/{}", branch.as_str());
    let text = probe_git(path, &["reflog", "show", "--date=unix", &refname], deadline).ok()?;
    let last = text.lines().last()?;
    let created = last
        .split_once("@{")?
        .1
        .split_once('}')?
        .0
        .parse::<u64>()
        .ok()?;
    let now = crate::session_store::now_ms().checked_div(1_000)?;
    now.checked_sub(created).map(std::time::Duration::from_secs)
}

#[derive(Debug, Clone)]
pub struct Pool {
    repo: PathBuf,
    dir: PathBuf,
    home: PathBuf,
    slots: u8,
}

static FOUND: std::sync::Mutex<Vec<(PathBuf, PathBuf)>> = std::sync::Mutex::new(Vec::new());

fn remember_repo(dir: &Path, repo: &Path) {
    if let Ok(mut found) = FOUND.lock()
        && !found.iter().any(|(known, _)| known == dir)
    {
        found.push((dir.to_path_buf(), repo.to_path_buf()));
    }
}

/// Invariant: a lane is itself a worktree, so lanes and memory key off the common git dir.
pub fn canonical_repo(cwd: &Path) -> Option<PathBuf> {
    let hit = FOUND.lock().ok().and_then(|found| {
        found
            .iter()
            .find(|(dir, _)| dir == cwd)
            .map(|(_, repo)| repo.clone())
    });
    if hit.is_some() {
        return hit;
    }
    let _span = yi_types::trace::span("lane.canonical_repo");
    let common = git(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    let repo = PathBuf::from(common.trim()).parent()?.canonicalize().ok()?;
    remember_repo(cwd, &repo);
    Some(repo)
}

impl Pool {
    pub fn open(home: &Path, cwd: &Path, slots: u8) -> Result<Self, LaneError> {
        let repo = canonical_repo(cwd).ok_or_else(|| LaneError::NotARepo(cwd.to_path_buf()))?;
        let hash = crate::ext::content_hash(&repo.to_string_lossy());
        let short: String = hash.chars().take(16).collect();
        // Invariant: opening the pool writes nothing; a `--here` start or a listing
        // leaves no directory behind. The first claim or reap creates it.
        let dir = home.join(".yi/lanes").join(short);
        Ok(Self {
            repo,
            dir,
            home: home.to_path_buf(),
            slots: slots.max(1),
        })
    }

    pub(crate) fn branch_exists(&self, branch: &BranchName) -> Result<bool, LaneError> {
        let local = format!("refs/heads/{}", branch.as_str());
        no(git(&self.repo, &["rev-parse", "--verify", "-q", &local])).map(|absent| !absent)
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn slot_path(&self, slot: SlotIndex) -> PathBuf {
        self.dir.join(slot.to_string())
    }

    fn state_path(&self, slot: SlotIndex) -> PathBuf {
        self.dir.join(format!("{slot}.json"))
    }

    fn held_path(&self, slot: SlotIndex) -> PathBuf {
        self.dir.join(format!("{slot}.held"))
    }

    pub(crate) fn read_state(&self, slot: SlotIndex) -> Result<SlotState, LaneError> {
        let path = self.state_path(slot);
        match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|source| LaneError::State { path, source })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SlotState::default()),
            Err(source) => Err(LaneError::Io { path, source }),
        }
    }

    pub(crate) fn write_state(&self, slot: SlotIndex, state: &SlotState) -> Result<(), LaneError> {
        let path = self.state_path(slot);
        let bytes = serde_json::to_vec_pretty(state).map_err(|source| LaneError::State {
            path: path.clone(),
            source,
        })?;
        yi_session::replace_file(&path, &bytes).map_err(io_error(&path))
    }

    fn lock(&self) -> Result<File, LaneError> {
        let path = self.dir.join("pool.lock");
        yi_session::lock_file(&path).map_err(io_error(&path))
    }

    /// Invariant: held while a live process holds the `.held` flock; named under a free one is an orphan.
    fn probe(&self, slot: SlotIndex) -> Result<(File, Option<String>), LaneError> {
        let path = self.held_path(slot);
        match yi_session::try_lock_file(&path).map_err(io_error(&path))? {
            Ok(file) => Ok((file, None)),
            Err(file) => Ok((
                file,
                Some(self.read_state(slot)?.session.unwrap_or_default()),
            )),
        }
    }

    fn branch_of(&self, slot: SlotIndex) -> Option<String> {
        head(&self.slot_path(slot))
            .ok()
            .and_then(|head| head.branch().map(ToString::to_string))
    }

    pub fn list(&self) -> Result<Vec<SlotView>, LaneError> {
        let deadline = std::time::Instant::now()
            .checked_add(LIST_BUDGET)
            .unwrap_or_else(std::time::Instant::now);
        // Invariant: a listing never fetches; `behind` counts against the `main` last seen.
        let base = self.local_main(deadline);
        let mut views = Vec::new();
        for n in 0..self.slots {
            let slot = SlotIndex(n);
            let path = self.slot_path(slot);
            if !path.is_dir() {
                continue;
            }
            let state = self.read_state(slot)?;
            let (_guard, live) = self.probe(slot)?;
            let status = || probe_git(&path, &STATUS, deadline);
            let holder = |session: String, status| {
                let branch = head(&path).ok().and_then(|head| head.branch().cloned());
                Holder {
                    session,
                    tree: tree_state(&path, status, base.as_ref(), deadline, &self.home),
                    age: branch
                        .as_ref()
                        .and_then(|branch| age_of(&path, branch, deadline)),
                    branch,
                }
            };
            views.push(match (live, state.session) {
                (Some(session), _) => SlotView::Held {
                    slot,
                    holder: holder(session, status()),
                    path,
                },
                (None, Some(session)) => SlotView::Orphan {
                    slot,
                    holder: holder(session, status()),
                    path,
                },
                (None, None) => match status() {
                    // Invariant: only a status read dirty makes an orphan; the claim decides an unread one.
                    Ok(dirty) if !dirty.trim().is_empty() => SlotView::Orphan {
                        slot,
                        holder: holder(String::new(), Ok(dirty)),
                        path,
                    },
                    _ => SlotView::Idle {
                        slot,
                        path,
                        warm: state.warm.as_ref().is_some_and(|warm| {
                            state.base.as_deref() == Some(warm.base.as_str())
                                && state.lockfile.as_deref() == Some(warm.lockfile.as_str())
                        }),
                        base: state.base,
                    },
                },
            });
        }
        Ok(views)
    }

    fn fetch_fresh(&self) -> bool {
        std::fs::metadata(self.repo.join(".git/FETCH_HEAD"))
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age.as_millis() < u128::from(FETCH_FRESH_MS))
    }

    fn fetch_if_stale(&self) -> bool {
        let _span = yi_types::trace::span("lane.fetch_if_stale");
        if FETCH_FORBIDDEN.load(std::sync::atomic::Ordering::Relaxed)
            || self.fetch_fresh()
            || git(&self.repo, &["remote", "get-url", "origin"]).is_err()
        {
            return false;
        }
        let trunk = hostgit::trunk(&self.repo);
        let _ = git(&self.repo, &["fetch", "-q", "origin", trunk.as_str()]);
        true
    }

    /// A claim starts from the `origin/main` last fetched and refreshes it beside the session.
    fn refresh_in_background(&self) {
        static FETCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        use std::sync::atomic::Ordering::SeqCst;
        if self.fetch_fresh() || FETCHING.swap(true, SeqCst) {
            return;
        }
        let pool = self.clone();
        std::thread::spawn(move || {
            pool.fetch_if_stale();
            FETCHING.store(false, SeqCst);
        });
    }

    fn resolve_commit(&self, candidate: &str) -> Result<String, LaneError> {
        let spec = format!("{candidate}^{{commit}}");
        git(&self.repo, &["rev-parse", "--verify", "-q", &spec]).map(|sha| sha.trim().to_owned())
    }

    fn local_main(&self, deadline: std::time::Instant) -> Option<Sha> {
        let trunk = hostgit::trunk(&self.repo);
        let (remote, local) = (
            format!("origin/{trunk}^{{commit}}"),
            format!("{trunk}^{{commit}}"),
        );
        probe_git(
            &self.repo,
            &["rev-parse", "--verify", "-q", &remote],
            deadline,
        )
        .or_else(|_| {
            probe_git(
                &self.repo,
                &["rev-parse", "--verify", "-q", &local],
                deadline,
            )
        })
        .ok()
        .and_then(|text| Sha::parse(&text))
    }

    fn resolve(&self, base: &ClaimBase, background: bool) -> Result<String, LaneError> {
        match base {
            ClaimBase::Commit(sha) => return self.resolve_commit(sha),
            ClaimBase::Main => {}
        }
        let trunk = hostgit::trunk(&self.repo);
        let known = background.then(|| self.resolve_commit(&format!("origin/{trunk}")));
        if let Some(Ok(sha)) = &known {
            self.refresh_in_background();
            return Ok(sha.clone());
        }
        let fetched = self.fetch_if_stale();
        let missing = known.is_some() && !fetched;
        if missing {
            if let Ok(sha) = self.resolve_commit(trunk.as_str()) {
                return Ok(sha);
            }
        } else {
            let deadline = std::time::Instant::now()
                .checked_add(std::time::Duration::from_millis(GIT_TIMEOUT_MS))
                .unwrap_or_else(std::time::Instant::now);
            if let Some(sha) = self.local_main(deadline) {
                return Ok(sha.as_str().to_owned());
            }
        }
        self.resolve_commit("HEAD")
    }

    /// How many lanes this pool holds, so a caller can size its own share of them.
    pub fn slots(&self) -> u8 {
        self.slots
    }

    pub fn claim(&self, session: &str, base: ClaimBase) -> Result<Lane, LaneError> {
        let _span = yi_types::trace::span("lane.claim");
        let pool_lock = self.lock()?;
        let (mut held, mut orphans) = (0_u8, 0_u8);
        let mut free = None;
        // A resumed session's tree holds its only copy of the work, so its gone slot is kept.
        for slot in (0..self.slots).map(SlotIndex) {
            let own = || {
                self.read_state(slot)
                    .is_ok_and(|state| state.session.as_deref() == Some(session))
            };
            if !self.slot_path(slot).is_dir() || !own() {
                continue;
            }
            if self.repo_gone(slot) {
                return Err(LaneError::RepoGone {
                    slot,
                    path: self.slot_path(slot),
                });
            }
            // Invariant: a resume takes its own slot before a free one, where its branch cannot check
            // out; the probe's flock is a temporary of the condition, so it drops before the reclaim.
            if self.probe(slot)?.1.is_none() {
                return self.reclaim(slot, session);
            }
        }
        for n in 0..self.slots {
            let slot = SlotIndex(n);
            // Invariant: the guard comes before the directory test, so a slot another claimer
            // is still creating reads held and two claimers never pick one fresh slot.
            let (guard, holder) = self.probe(slot)?;
            if holder.is_some() {
                held = held.saturating_add(1);
                continue;
            }
            if !self.slot_path(slot).is_dir() {
                free = Some((slot, guard, true));
                break;
            }
            if self.repo_gone(slot) {
                self.stop_warmer(slot)?;
                let path = self.slot_path(slot);
                std::fs::remove_dir_all(&path).map_err(io_error(&path))?;
                self.write_state(slot, &SlotState::default())?;
                free = Some((slot, guard, true));
                break;
            }
            match self.read_state(slot)?.session {
                Some(_) if self.abandoned(slot)? => {
                    self.detach(slot, self.branch_of(slot).as_deref())?;
                    self.clear_session(slot)?;
                    free = Some((slot, guard, false));
                    break;
                }
                Some(_) => orphans = orphans.saturating_add(1),
                // Invariant: a dirty tree with no session is an older binary's quit, never free.
                None if self.dirty(slot) => orphans = orphans.saturating_add(1),
                None => {
                    free = Some((slot, guard, false));
                    break;
                }
            }
        }
        let Some((slot, guard, fresh)) = free else {
            return Err(LaneError::PoolFull {
                slots: self.slots,
                held,
                orphans,
            });
        };
        drop(pool_lock);
        let branch = BranchName::for_session(session)?;
        let sha = self.resolve(&base, true)?;
        let path = self.slot_path(slot);
        let reason = format!("session:{session}");
        let text = path.to_string_lossy().into_owned();
        // Invariant: `-B` runs only over no branch or a merged one; any unread answer refuses.
        let ancestor = ["merge-base", "--is-ancestor", branch.as_str(), &sha];
        let kept = self.branch_exists(&branch)? && no(git(&self.repo, &ancestor))?;
        let mut add = vec!["worktree", "add", "-q", "--lock", "--reason", &reason];
        let mut checkout = vec!["checkout", "-q", "-f"];
        if kept {
            add.extend([text.as_str(), branch.as_str()]);
            checkout.push(branch.as_str());
        } else {
            add.extend(["-B", branch.as_str(), &text, &sha]);
            checkout.extend(["-B", branch.as_str(), &sha]);
        }
        if fresh {
            git(&self.repo, &add)?;
        } else {
            self.stop_warmer(slot)?;
            git(&path, &checkout)?;
            git(&path, &["clean", "-qfd"])?;
            let _ = git(
                &self.repo,
                &["worktree", "lock", "--reason", &reason, &text],
            );
        }
        let mut state = self.read_state(slot)?;
        state.base = Some(sha);
        state.session = Some(session.to_owned());
        self.write_state(slot, &state)?;
        remember_repo(&path, &self.repo);
        Ok(Lane {
            slot,
            path,
            branch,
            pool: self.clone(),
            session: session.to_owned(),
            locked: true,
            released: false,
            _held: guard,
        })
    }

    fn reclaim(&self, slot: SlotIndex, session: &str) -> Result<Lane, LaneError> {
        let (guard, _) = self.probe(slot)?;
        let branch = BranchName::for_session(session)?;
        let path = self.slot_path(slot);
        git(&path, &["checkout", "-q", branch.as_str()])?;
        remember_repo(&path, &self.repo);
        Ok(Lane {
            slot,
            path,
            branch,
            pool: self.clone(),
            session: session.to_owned(),
            locked: false,
            released: false,
            _held: guard,
        })
    }

    /// Incident: a repository recreated at its path left slots whose `.git` named a gone gitdir.
    fn repo_gone(&self, slot: SlotIndex) -> bool {
        let path = self.slot_path(slot);
        path.join(".git").is_file() && yi_permission::git_dirs(&path).is_empty()
    }

    fn detach(&self, slot: SlotIndex, branch: Option<&str>) -> Result<bool, LaneError> {
        let path = self.slot_path(slot);
        let text = path.to_string_lossy().into_owned();
        let _ = git(&self.repo, &["worktree", "unlock", &text]);
        git(&path, &["checkout", "-q", "--detach"])?;
        let Some(branch) = branch else {
            return Ok(false);
        };
        let base = self.resolve(&ClaimBase::Main, false)?;
        let merged = git(&self.repo, &["merge-base", "--is-ancestor", branch, &base]).is_ok();
        if merged {
            git(&self.repo, &["branch", "-q", "-D", branch])?;
        }
        Ok(!merged)
    }
}

/// A git query's exit 1 is its answer no; any other failure is an error, never a no.
fn no(answer: Result<String, LaneError>) -> Result<bool, LaneError> {
    match answer {
        Ok(_) => Ok(false),
        Err(LaneError::Git {
            exit_code: Some(1), ..
        }) => Ok(true),
        Err(error) => Err(error),
    }
}

fn probe_deadline() -> std::time::Duration {
    std::time::Duration::from_millis(5_000)
}

/// Invariant: every way out takes `self`, so a second hand-back does not compile.
#[derive(Debug)]
pub struct Lane {
    slot: SlotIndex,
    path: PathBuf,
    branch: BranchName,
    pool: Pool,
    session: String,
    /// The worktree lock names `session`: set by this process, so a bind to it again is a no-op.
    locked: bool,
    released: bool,
    _held: File,
}

/// Invariant: a dropped clean lane is handed back detached, a dirty one kept; only release warms.
impl Drop for Lane {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let _ = self.pool.lock().and_then(|_pool| {
            if self.pool.dirty(self.slot) {
                return Ok(());
            }
            self.pool.detach(self.slot, Some(self.branch.as_str()))?;
            self.pool.clear_session(self.slot)
        });
    }
}

impl Lane {
    pub fn slot(&self) -> SlotIndex {
        self.slot
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    pub fn base(&self) -> Option<String> {
        self.pool
            .read_state(self.slot)
            .ok()
            .and_then(|state| state.base)
    }

    /// Invariant: the branch carries the session id, or a resume cannot find it.
    pub fn bind_session(&mut self, session: &str) -> Result<(), LaneError> {
        let _span = yi_types::trace::span("lane.bind_session");
        if self.locked && self.session == session {
            return Ok(());
        }
        let branch = BranchName::for_session(session)?;
        if branch != self.branch {
            git(&self.path, &["branch", "-q", "-m", branch.as_str()])?;
            self.branch = branch;
        }
        self.session = session.to_owned();
        let mut state = self.pool.read_state(self.slot)?;
        state.session = Some(session.to_owned());
        self.pool.write_state(self.slot, &state)?;
        let text = self.path.to_string_lossy().into_owned();
        let reason = format!("session:{session}");
        let _ = git(&self.pool.repo, &["worktree", "unlock", &text]);
        let _ = git(
            &self.pool.repo,
            &["worktree", "lock", "--reason", &reason, &text],
        );
        self.locked = true;
        Ok(())
    }

    pub fn discard(self) -> Result<Wiped, LaneError> {
        let branch = self.branch.as_str().to_owned();
        let repo = self.pool.repo.clone();
        // Invariant: only a release warms; a discard deletes the branch a warmer would build for.
        let wiped = self.take_slot(false)?;
        let _ = git(&repo, &["branch", "-q", "-D", &branch]);
        Ok(wiped)
    }

    /// A non-empty answer is the work that kept the slot as this session's orphan.
    pub fn release(self) -> Result<Uncommitted, LaneError> {
        let mut lane = self;
        lane.released = true;
        let kept = Uncommitted::read(&lane.path, Some(lane.pool.home()))?;
        if !kept.is_empty() {
            return Ok(kept);
        }
        // The read above said the tree is clean, so the hand-back pays no second one.
        let _ = lane.finish(true, Some(Wiped::default()))?;
        Ok(Uncommitted::default())
    }

    pub(super) fn take_slot(self, quit: bool) -> Result<Wiped, LaneError> {
        self.finish(quit, None)
    }

    /// `wiped` is the measurement a caller already made of a tree it found clean; `None` wipes now.
    fn finish(mut self, quit: bool, wiped: Option<Wiped>) -> Result<Wiped, LaneError> {
        self.released = true;
        let _pool = self.pool.lock()?;
        self.pool.detach(self.slot, Some(self.branch.as_str()))?;
        let wiped = match wiped {
            Some(wiped) => wiped,
            None => self.pool.wipe(self.slot)?,
        };
        self.pool.clear_session(self.slot)?;
        if quit {
            // Invariant: an unseen lockfile stays cold; that is not a failure.
            match toolchain::warm(&self.pool, self.slot) {
                Ok(()) | Err(LaneError::LockfileUnseen { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(wiped)
    }

    pub fn sync(&self) -> Result<Option<String>, LaneError> {
        toolchain::sync(&self.pool, self.slot, &self.path)
    }

    pub fn session(&self) -> &str {
        &self.session
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_denied_sync_names_the_toolchains_write_target_not_the_wrapper() -> Result<(), String> {
        let cwd = std::env::temp_dir().join("yi-lane-refusal-hint");
        std::fs::create_dir_all(&cwd).map_err(|error| error.to_string())?;
        let home = std::env::temp_dir();
        let sandbox = yi_tools::Sandbox::for_workspace(&cwd, &home, None);
        let script = "echo x > /denied-target/a; \
                      printf 'sh: /etc/hosts: Operation not permitted\\n' >&2; exit 3";
        let label = format!("sh -c {script}");
        let Err(error) = capture_named(
            &cwd,
            &label,
            "sh",
            &["-c", script],
            std::time::Duration::from_secs(10),
            yi_tools::OUTPUT_CAP,
            Some(&sandbox),
        ) else {
            return Err("the denied sync succeeded".to_owned());
        };
        assert!(
            error.contains("next: the sandbox refused writing `/denied-target/a"),
            "the hint names the toolchain command's write target, not the wrapper: {error}"
        );
        Ok(())
    }
}
