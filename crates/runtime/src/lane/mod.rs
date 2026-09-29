//! Pooled git worktrees a session works in, and the hand-back ladder out of them.
//! A root session never edits the trunk checkout; it claims a slot.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_types::lane::SlotState;

pub mod land;
pub mod settle;
pub mod toolchain;

const GIT_TIMEOUT_MS: u64 = 120_000;
/// Incident: three slots refused a console's fourth session, new or resumed. Unset, the
/// pool grows at the first missing slot and keeps it, so it settles at peak concurrency.
pub const DEFAULT_SLOTS: u8 = u8::MAX;
const FETCH_FRESH_MS: u64 = 60_000;
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

/// Invariant: checked against git's ref-format charset once; `git` never sees a flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchName(String);

impl BranchName {
    pub fn parse(text: &str) -> Result<Self, LaneError> {
        let reason = if text.is_empty() {
            Some("empty")
        } else if text.starts_with('-') {
            Some("leading dash")
        } else if text.contains("..") || text.contains("//") {
            Some("repeated separator")
        } else if text.ends_with('/') || text.ends_with('.') || text.ends_with(".lock") {
            Some("trailing separator")
        } else if !text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        {
            Some("character outside [A-Za-z0-9._/-]")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(LaneError::Branch {
                text: text.to_owned(),
                reason,
            }),
            None => Ok(Self(text.to_owned())),
        }
    }

    pub fn for_session(session: &str) -> Result<Self, LaneError> {
        Self::parse(&format!("yi/{session}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Invariant: forty hex digits, checked once; a sha reaches git argv only through this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha(String);

impl Sha {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        (text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> &str {
        self.0.get(..8).unwrap_or(&self.0)
    }
}

/// What `.git/HEAD` names: the one reader the status row, the environment block
/// and the pool share, a file read with no git process behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(BranchName),
    Detached(Sha),
}

impl Head {
    pub fn branch(&self) -> Option<&BranchName> {
        match self {
            Self::Branch(branch) => Some(branch),
            Self::Detached(_) => None,
        }
    }

    /// The branch name, or the short sha when detached: one spelling on every surface.
    pub fn label(&self) -> String {
        match self {
            Self::Branch(branch) => branch.to_string(),
            Self::Detached(sha) => sha.short().to_owned(),
        }
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
    #[error("git {} exited {exit_code:?}: {output}", args.join(" "))]
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

pub(crate) fn capture(
    cwd: &Path,
    program: &str,
    args: &[&str],
    deadline: std::time::Duration,
) -> Result<String, String> {
    capture_capped(cwd, program, args, deadline, 30_000).map(|capture| capture.stdout)
}

pub(crate) fn capture_capped(
    cwd: &Path,
    program: &str,
    args: &[&str],
    deadline: std::time::Duration,
    cap: usize,
) -> Result<yi_tools::CommandCapture, String> {
    let _span = yi_types::trace::span("lane.capture").arg("program", program);
    let mut command = yi_tools::command(program);
    command.current_dir(cwd).args(args);
    let deadline = std::time::Instant::now()
        .checked_add(deadline)
        .unwrap_or_else(std::time::Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || std::time::Instant::now() >= deadline);
    let capture = yi_tools::run_captured(command, None, &cancelled, cap)
        .map_err(|error| format!("{program} {}: {error}", args.join(" ")))?;
    if capture.exit_code == Some(0) {
        return Ok(capture);
    }
    Err(format!(
        "{program} {} failed:\n{}{}",
        args.join(" "),
        capture.stdout,
        capture.stderr
    ))
}

pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String, LaneError> {
    let _span = yi_types::trace::span("lane.git").arg("args", args.join(" "));
    let mut command = yi_tools::command("git");
    command.current_dir(cwd).args(args);
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(GIT_TIMEOUT_MS))
        .unwrap_or_else(std::time::Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || std::time::Instant::now() >= deadline);
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let capture = yi_tools::run_captured(command, None, &cancelled, 30_000).map_err(|error| {
        LaneError::Git {
            args: owned.clone(),
            exit_code: None,
            output: error,
        }
    })?;
    if capture.exit_code == Some(0) {
        return Ok(capture.stdout);
    }
    Err(LaneError::Git {
        args: owned,
        exit_code: capture.exit_code,
        output: format!("{}{}", capture.stdout, capture.stderr),
    })
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

fn tree_state(path: &Path, base: Option<&Sha>, deadline: std::time::Instant) -> TreeState {
    let Ok(status) = probe_git(path, &["status", "--porcelain"], deadline) else {
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
        let base = probe_git(
            &self.repo,
            &["rev-parse", "--verify", "-q", "origin/main^{commit}"],
            deadline,
        )
        .or_else(|_| {
            probe_git(
                &self.repo,
                &["rev-parse", "--verify", "-q", "main^{commit}"],
                deadline,
            )
        })
        .ok()
        .and_then(|text| Sha::parse(&text));
        let mut views = Vec::new();
        for n in 0..self.slots {
            let slot = SlotIndex(n);
            let path = self.slot_path(slot);
            if !path.is_dir() {
                continue;
            }
            let state = self.read_state(slot)?;
            let (_guard, live) = self.probe(slot)?;
            let holder = |session: String| {
                let branch = head(&path).ok().and_then(|head| head.branch().cloned());
                Holder {
                    session,
                    tree: tree_state(&path, base.as_ref(), deadline),
                    age: branch
                        .as_ref()
                        .and_then(|branch| age_of(&path, branch, deadline)),
                    branch,
                }
            };
            views.push(match (live, state.session) {
                (Some(session), _) => SlotView::Held {
                    slot,
                    holder: holder(session),
                    path,
                },
                (None, Some(session)) => SlotView::Orphan {
                    slot,
                    holder: holder(session),
                    path,
                },
                (None, None) => SlotView::Idle {
                    slot,
                    path,
                    warm: state.warm.as_ref().is_some_and(|warm| {
                        state.base.as_deref() == Some(warm.base.as_str())
                            && state.lockfile.as_deref() == Some(warm.lockfile.as_str())
                    }),
                    base: state.base,
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
        if self.fetch_fresh() || git(&self.repo, &["remote", "get-url", "origin"]).is_err() {
            return false;
        }
        let _ = git(&self.repo, &["fetch", "-q", "origin", "main"]);
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

    fn resolve(&self, base: &ClaimBase, background: bool) -> Result<String, LaneError> {
        let candidates: Vec<String> = match base {
            ClaimBase::Main => {
                let known = background.then(|| {
                    git(
                        &self.repo,
                        &["rev-parse", "--verify", "-q", "origin/main^{commit}"],
                    )
                });
                if let Some(Ok(sha)) = &known {
                    self.refresh_in_background();
                    return Ok(sha.trim().to_owned());
                }
                let fetched = self.fetch_if_stale();
                let missing = known.is_some() && !fetched;
                ["origin/main", "main", "HEAD"]
                    .into_iter()
                    .filter(|name| !missing || *name != "origin/main")
                    .map(str::to_owned)
                    .collect()
            }
            ClaimBase::Commit(sha) => vec![sha.clone()],
        };
        let mut last = None;
        for candidate in candidates {
            let spec = format!("{candidate}^{{commit}}");
            match git(&self.repo, &["rev-parse", "--verify", "-q", &spec]) {
                Ok(sha) => return Ok(sha.trim().to_owned()),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or(LaneError::NotARepo(self.repo.clone())))
    }

    /// How many lanes this pool holds, so a caller can size its own share of them.
    pub fn slots(&self) -> u8 {
        self.slots
    }

    pub fn claim(&self, session: &str, base: ClaimBase) -> Result<Lane, LaneError> {
        let _span = yi_types::trace::span("lane.claim");
        let _pool = self.lock()?;
        let (mut held, mut orphans) = (0_u8, 0_u8);
        let mut free = None;
        for n in 0..self.slots {
            let slot = SlotIndex(n);
            if !self.slot_path(slot).is_dir() {
                free = Some((slot, None, true));
                break;
            }
            let (guard, holder) = self.probe(slot)?;
            if holder.is_some() {
                held = held.saturating_add(1);
                continue;
            }
            match self.read_state(slot)?.session {
                Some(left) if left == session => {
                    // A resumed session gets its own slot and branch back, work intact.
                    drop(guard);
                    return self.reclaim(slot, session);
                }
                Some(_) if self.abandoned(slot)? => {
                    self.detach(slot, self.branch_of(slot).as_deref())?;
                    let mut state = self.read_state(slot)?;
                    state.session = None;
                    self.write_state(slot, &state)?;
                    free = Some((slot, Some(guard), false));
                    break;
                }
                Some(_) => orphans = orphans.saturating_add(1),
                None => {
                    free = Some((slot, Some(guard), false));
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
        let guard = match guard {
            Some(guard) => guard,
            None => self.probe(slot)?.0,
        };
        let branch = BranchName::for_session(session)?;
        let sha = self.resolve(&base, true)?;
        let path = self.slot_path(slot);
        let reason = format!("session:{session}");
        let text = path.to_string_lossy().into_owned();
        if fresh {
            git(
                &self.repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "--lock",
                    "--reason",
                    &reason,
                    "-B",
                    branch.as_str(),
                    &text,
                    &sha,
                ],
            )?;
        } else {
            self.stop_warmer(slot)?;
            git(
                &path,
                &["checkout", "-q", "-f", "-B", branch.as_str(), &sha],
            )?;
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

    /// Incident: a killed install leaves half a tree, so the lockfile hash is forgotten too.
    fn stop_warmer(&self, slot: SlotIndex) -> Result<(), LaneError> {
        let mut state = self.read_state(slot)?;
        let Some(pid) = state.warm.as_ref().and_then(|warm| warm.pid) else {
            return Ok(());
        };
        let running = || yi_kernel::bootstrap::process_is_running(pid).map_err(io_error(&self.dir));
        if !running()? {
            return Ok(());
        }
        let term = ["-c", r#"kill -TERM "$1""#, "kill", &pid.to_string()];
        let _the_poll_below_sees_a_failed_term =
            capture(&self.dir, "/bin/sh", &term, probe_deadline());
        let started = std::time::Instant::now();
        let _span = yi_types::trace::span("lane.warmer_exit");
        let mut pace = yi_types::backoff::backoff(std::time::Duration::from_millis(200));
        while running()? {
            if started.elapsed().as_millis() >= u128::from(WARMER_EXIT_WAIT_MS) {
                return Err(LaneError::WarmerStuck {
                    slot,
                    pid,
                    waited_ms: WARMER_EXIT_WAIT_MS,
                });
            }
            std::thread::sleep(pace());
        }
        if let Some(warm) = state.warm.as_mut() {
            warm.pid = None;
        }
        state.lockfile = None;
        self.write_state(slot, &state)
    }

    /// Incident: three lanes left by dead `pid-` drives held nothing and still filled the pool.
    /// An orphan with a clean tree and a branch `main` already contains has nothing to lose.
    fn abandoned(&self, slot: SlotIndex) -> Result<bool, LaneError> {
        let path = self.slot_path(slot);
        if !git(&path, &["status", "--porcelain"])?.trim().is_empty() {
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
        let mut state = self.read_state(slot)?;
        state.session = None;
        self.write_state(slot, &state)?;
        drop(guard);
        Ok(match (branch, kept) {
            (Some(branch), true) => format!("lane {slot} freed; branch {branch} kept (unmerged)"),
            (Some(branch), false) => format!("lane {slot} freed; branch {branch} deleted (merged)"),
            (None, _) => format!("lane {slot} freed"),
        })
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

/// Invariant: a dropped lane is handed back detached; only the explicit release warms.
impl Drop for Lane {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let _ = self.pool.lock().and_then(|_pool| {
            self.pool.detach(self.slot, Some(self.branch.as_str()))?;
            let mut state = self.pool.read_state(self.slot)?;
            state.session = None;
            self.pool.write_state(self.slot, &state)
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

    pub fn discard(self) -> Result<(), LaneError> {
        let branch = self.branch.as_str().to_owned();
        let pool = self.pool.clone();
        let slot = self.slot;
        self.release_inner(false)?;
        let _ = git(&pool.repo, &["branch", "-q", "-D", &branch]);
        let mut state = pool.read_state(slot)?;
        state.session = None;
        pool.write_state(slot, &state)
    }

    pub fn release(self) -> Result<(), LaneError> {
        self.release_inner(true)
    }

    fn release_inner(mut self, warm: bool) -> Result<(), LaneError> {
        self.released = true;
        let _pool = self.pool.lock()?;
        self.pool.detach(self.slot, Some(self.branch.as_str()))?;
        let mut state = self.pool.read_state(self.slot)?;
        state.session = None;
        self.pool.write_state(self.slot, &state)?;
        if warm {
            // Invariant: an unseen lockfile stays cold; that is not a failure.
            match toolchain::warm(&self.pool, self.slot) {
                Ok(()) | Err(LaneError::LockfileUnseen { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn sync(&self) -> Result<Option<String>, LaneError> {
        toolchain::sync(&self.pool, self.slot, &self.path)
    }

    pub fn session(&self) -> &str {
        &self.session
    }
}
