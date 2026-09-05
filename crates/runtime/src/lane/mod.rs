//! Pooled git worktrees a session works in, and the hand-back ladder out of them.
//! A root session never edits the trunk checkout; it claims a slot.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::lane::SlotState;

use crate::subagent::{ChildStatus, SubagentHost};

pub mod land;
pub mod toolchain;

const GIT_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_SLOTS: u8 = 3;
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

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LaneError {
    #[error("no free lane: {held} held, {orphans} orphaned of {slots}; `yi lanes` lists them")]
    PoolFull { slots: u8, held: u8, orphans: u8 },
    #[error("lane {slot} is held by session {session}")]
    SlotBusy { slot: SlotIndex, session: String },
    #[error("lane {slot} was left by session {session}; `yi lanes reap {slot}` frees it")]
    Orphan { slot: SlotIndex, session: String },
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
    #[error("lane {slot}'s warmer (pid {pid}) did not exit within {waited_ms} ms")]
    WarmerStuck {
        slot: SlotIndex,
        pid: u32,
        waited_ms: u64,
    },
    #[error("no forge: the repository has no `origin` remote")]
    NoForge,
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
    let mut command = yi_tools::command(program);
    command.current_dir(cwd).args(args);
    let deadline = std::time::Instant::now()
        .checked_add(deadline)
        .unwrap_or_else(std::time::Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || std::time::Instant::now() >= deadline);
    let capture = yi_tools::run_captured(command, None, &cancelled, 30_000)
        .map_err(|error| format!("{program} {}: {error}", args.join(" ")))?;
    if capture.exit_code == Some(0) {
        return Ok(capture.stdout);
    }
    Err(format!(
        "{program} {} failed:\n{}{}",
        args.join(" "),
        capture.stdout,
        capture.stderr
    ))
}

pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String, LaneError> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotView {
    Idle {
        slot: SlotIndex,
        base: Option<String>,
        warm: bool,
    },
    Held {
        slot: SlotIndex,
        session: String,
        branch: Option<String>,
    },
    Orphan {
        slot: SlotIndex,
        session: String,
        branch: Option<String>,
    },
}

impl SlotView {
    pub fn slot(&self) -> SlotIndex {
        match self {
            Self::Idle { slot, .. } | Self::Held { slot, .. } | Self::Orphan { slot, .. } => *slot,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pool {
    repo: PathBuf,
    dir: PathBuf,
    home: PathBuf,
    slots: u8,
}

impl Pool {
    pub fn open(home: &Path, cwd: &Path, slots: u8) -> Result<Self, LaneError> {
        // Invariant: a lane is itself a worktree, so the pool is keyed by the common git dir.
        let common = git(
            cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .map_err(|_| LaneError::NotARepo(cwd.to_path_buf()))?;
        let common = PathBuf::from(common.trim());
        let root = common
            .parent()
            .ok_or_else(|| LaneError::NotARepo(cwd.to_path_buf()))?;
        let repo = root.canonicalize().map_err(io_error(root))?;
        let hash = crate::ext::content_hash(&repo.to_string_lossy());
        let short: String = hash.chars().take(16).collect();
        let dir = home.join(".yi/lanes").join(short);
        std::fs::create_dir_all(&dir).map_err(io_error(&dir))?;
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
        std::fs::write(&path, bytes).map_err(io_error(&path))
    }

    /// Invariant: the kernel drops this flock with the process; a crash cannot wedge the pool.
    fn lock(&self) -> Result<File, LaneError> {
        let path = self.dir.join("pool.lock");
        let file = File::create(&path).map_err(io_error(&path))?;
        file.lock().map_err(io_error(&path))?;
        Ok(file)
    }

    /// Invariant: held while a live process holds the `.held` flock; named under a free one is an orphan.
    fn probe(&self, slot: SlotIndex) -> Result<(File, Option<String>), LaneError> {
        let path = self.held_path(slot);
        let file = File::create(&path).map_err(io_error(&path))?;
        match file.try_lock() {
            Ok(()) => Ok((file, None)),
            Err(std::fs::TryLockError::WouldBlock) => {
                let session = self.read_state(slot)?.session.unwrap_or_default();
                Ok((file, Some(session)))
            }
            Err(std::fs::TryLockError::Error(source)) => Err(LaneError::Io { path, source }),
        }
    }

    fn branch_of(&self, slot: SlotIndex) -> Option<String> {
        git(
            &self.slot_path(slot),
            &["symbolic-ref", "--short", "-q", "HEAD"],
        )
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
    }

    pub fn list(&self) -> Result<Vec<SlotView>, LaneError> {
        let mut views = Vec::new();
        for n in 0..self.slots {
            let slot = SlotIndex(n);
            if !self.slot_path(slot).is_dir() {
                continue;
            }
            let state = self.read_state(slot)?;
            let (_guard, holder) = self.probe(slot)?;
            views.push(match (holder, state.session) {
                (Some(session), _) => SlotView::Held {
                    slot,
                    session,
                    branch: self.branch_of(slot),
                },
                (None, Some(session)) => SlotView::Orphan {
                    slot,
                    session,
                    branch: self.branch_of(slot),
                },
                (None, None) => SlotView::Idle {
                    slot,
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

    fn fetch_if_stale(&self) {
        let fetch_head = self.repo.join(".git/FETCH_HEAD");
        let fresh = std::fs::metadata(&fetch_head)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age.as_millis() < u128::from(FETCH_FRESH_MS));
        if fresh || git(&self.repo, &["remote", "get-url", "origin"]).is_err() {
            return;
        }
        let _ = git(&self.repo, &["fetch", "-q", "origin", "main"]);
    }

    fn resolve(&self, base: &ClaimBase) -> Result<String, LaneError> {
        let candidates: Vec<String> = match base {
            ClaimBase::Main => {
                self.fetch_if_stale();
                vec![
                    "origin/main".to_owned(),
                    "main".to_owned(),
                    "HEAD".to_owned(),
                ]
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

    pub fn claim(&self, session: &str, base: ClaimBase) -> Result<Lane, LaneError> {
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
        let sha = self.resolve(&base)?;
        let path = self.slot_path(slot);
        if fresh {
            let text = path.to_string_lossy().into_owned();
            git(
                &self.repo,
                &["worktree", "add", "-q", "--detach", &text, &sha],
            )?;
        } else {
            self.stop_warmer(slot)?;
            git(&path, &["reset", "-q", "--hard", &sha])?;
            git(&path, &["clean", "-qfd"])?;
        }
        let branch = BranchName::for_session(session)?;
        git(&path, &["checkout", "-q", "-B", branch.as_str()])?;
        let mut state = self.read_state(slot)?;
        state.base = Some(sha);
        state.session = Some(session.to_owned());
        self.write_state(slot, &state)?;
        let reason = format!("session:{session}");
        let text = path.to_string_lossy().into_owned();
        let _ = git(
            &self.repo,
            &["worktree", "lock", "--reason", &reason, &text],
        );
        Ok(Lane {
            slot,
            path,
            branch,
            pool: self.clone(),
            session: session.to_owned(),
            released: false,
            _held: guard,
        })
    }

    fn reclaim(&self, slot: SlotIndex, session: &str) -> Result<Lane, LaneError> {
        let (guard, _) = self.probe(slot)?;
        let branch = BranchName::for_session(session)?;
        let path = self.slot_path(slot);
        git(&path, &["checkout", "-q", branch.as_str()])?;
        Ok(Lane {
            slot,
            path,
            branch,
            pool: self.clone(),
            session: session.to_owned(),
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
        let text = pid.to_string();
        if capture(&self.dir, "kill", &["-0", &text], probe_deadline()).is_err() {
            return Ok(());
        }
        let _ = capture(&self.dir, "kill", &["-TERM", &text], probe_deadline());
        let started = std::time::Instant::now();
        while capture(&self.dir, "kill", &["-0", &text], probe_deadline()).is_ok() {
            if started.elapsed().as_millis() >= u128::from(WARMER_EXIT_WAIT_MS) {
                return Err(LaneError::WarmerStuck {
                    slot,
                    pid,
                    waited_ms: WARMER_EXIT_WAIT_MS,
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
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
        let Some(branch) = self.branch_of(slot) else {
            return Ok(true);
        };
        let base = self.resolve(&ClaimBase::Main)?;
        Ok(git(&self.repo, &["merge-base", "--is-ancestor", &branch, &base]).is_ok())
    }

    /// Invariant: an orphan's branch is the only copy of its work; it goes only once `main` has it.
    pub fn reap(&self, slot: SlotIndex) -> Result<String, LaneError> {
        let _pool = self.lock()?;
        let (guard, holder) = self.probe(slot)?;
        if let Some(session) = holder {
            return Err(LaneError::SlotBusy { slot, session });
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
        let base = self.resolve(&ClaimBase::Main)?;
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
        Ok(())
    }

    /// Incident: uncommitted child work merged as nothing, so it is committed first.
    pub fn merge_into(self, into: &Path, label: &str) -> Result<String, LaneError> {
        let status = git(&self.path, &["status", "--porcelain"])?;
        if !status.trim().is_empty() {
            git(&self.path, &["add", "-A"])?;
            git(
                &self.path,
                &["commit", "-q", "-m", &format!("subagent {label} work")],
            )?;
        }
        let output = git(
            into,
            &["merge", "--no-ff", "--no-edit", self.branch.as_str()],
        )?;
        self.release_inner(false)?;
        Ok(output)
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

    pub(crate) fn as_reply(&self) -> Map<String, Value> {
        let mut reply = Map::new();
        reply.insert(
            "branch".to_owned(),
            Value::String(self.branch.as_str().to_owned()),
        );
        reply.insert(
            "path".to_owned(),
            Value::String(self.path.to_string_lossy().into_owned()),
        );
        reply
    }
}

impl SubagentHost {
    /// B11 hand-back into the parent's own checkout, which is its lane when it has one.
    pub fn merge_worktree(&self, target: &str) -> Result<Map<String, Value>, String> {
        let (lane, name) = self.take_settled_worktree(target)?;
        let mut reply = lane.as_reply();
        let output = lane
            .merge_into(&self.options.cwd, &name)
            .map_err(|error| error.to_string())?;
        reply.insert("merged".to_owned(), Value::Bool(true));
        reply.insert("output".to_owned(), Value::String(output));
        Ok(reply)
    }

    pub fn discard_worktree(&self, target: &str) -> Result<Map<String, Value>, String> {
        let (lane, _) = self.take_settled_worktree(target)?;
        let mut reply = lane.as_reply();
        lane.discard().map_err(|error| error.to_string())?;
        reply.insert("discarded".to_owned(), Value::Bool(true));
        Ok(reply)
    }

    /// Invariant: taken off the record, so a tree is never handed back twice.
    fn take_settled_worktree(&self, target: &str) -> Result<(Lane, String), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let key = Self::key_of(&children, target)?;
        let record = children
            .get_mut(&key)
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
        if record.status == ChildStatus::Running {
            return Err(format!(
                "child \"{target}\" is still running; wait for it before touching its worktree"
            ));
        }
        let lane = record
            .worktree
            .take()
            .ok_or_else(|| format!("child \"{target}\" has no worktree (isolation was none)"))?;
        Ok((lane, record.session_name.clone()))
    }
}
