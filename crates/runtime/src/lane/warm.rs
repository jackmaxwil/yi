//! Checks on warm directories: a per-repository pool of `node.slots` trees whose cargo output
//! outlives a check, scrubbed to cargo's bookkeeping so no borrower reads another's files.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::hostgit::{git, identity, sha_of};
use super::{ClaimBase, Lane, LaneError, Pool, Sha};

/// How long a check may wait for a directory when the session names no deadline.
pub const CHECK_QUEUE: Duration = Duration::from_secs(3_600);
const BACKOFF_START: Duration = Duration::from_millis(50);
const BACKOFF_MAX: Duration = Duration::from_secs(2);
/// What survives under `target/<profile>`: a binary at `target/debug/app` would run the last
/// borrower's with no build at all.
const KEPT_UNDER_PROFILE: [&str; 4] = ["deps", "build", ".fingerprint", "incremental"];

/// A check's answer. `Abstain` is no verdict on the work: past a bound, or refused by the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Verdict {
    Green,
    Red { exit: Option<i32> },
    Abstain { why: String },
}

#[derive(Debug, Clone)]
pub struct WarmRun {
    pub verdict: Verdict,
    /// What the check printed, its paths the model's. `None` when it never ran.
    pub capture: Option<yi_tools::CommandCapture>,
}

impl WarmRun {
    #[must_use]
    pub fn output(&self) -> String {
        self.capture.as_ref().map_or_else(String::new, |capture| {
            format!("{}{}", capture.stdout, capture.stderr)
        })
    }
}

impl Pool {
    /// Its own pool, since the verification lanes are capped for plan acceptances, not directories.
    pub fn warm(&self) -> Result<Self, LaneError> {
        let card = crate::node::card(&self.home).map_err(LaneError::Forge)?;
        Ok(Self {
            repo: self.repo.clone(),
            dir: self.dir.join("warm"),
            home: self.home.clone(),
            slots: card.slots.get(),
        })
    }
}

impl Lane {
    /// Unlike a discard, cargo's output stays for the next borrower's build. Incident: a check that
    /// wrote `Cargo.lock` left the slot dirty, the pool read it as an orphan, and the next check waited.
    pub fn release_warm(mut self) -> Result<(), LaneError> {
        self.released = true;
        let _pool = self.pool.lock()?;
        let cleaned = git(&self.path, &["reset", "-q", "--hard"]).and_then(|_| scrub(&self.path));
        if let Err(error) = cleaned {
            self.remove_warm_tree()?;
            return Err(error);
        }
        let branch = self.branch.as_str().to_owned();
        self.pool.detach(self.slot, None)?;
        let _kept_only_by_its_own_name = git(&self.pool.repo, &["branch", "-q", "-D", &branch]);
        self.pool.clear_session(self.slot)
    }

    /// Incident: a reset or a scrub that failed left the slot dirty with its session recorded, so
    /// every later check waited on it. A warm tree holds no work; gone, the next claim makes a new one.
    fn remove_warm_tree(&self) -> Result<(), LaneError> {
        let (repo, text) = (&self.pool.repo, self.path.to_string_lossy().into_owned());
        let _gone_already_is_fine = git(repo, &["worktree", "unlock", &text]);
        self.pool.stop_warmer(self.slot)?;
        std::fs::remove_dir_all(&self.path).map_err(super::io_error(&self.path))?;
        let _the_next_add_prunes_too = git(repo, &["worktree", "prune", "--expire=now"]);
        let branch = self.branch.as_str();
        let _kept_only_by_its_own_name = git(repo, &["branch", "-q", "-D", branch]);
        self.pool
            .write_state(self.slot, &yi_types::lane::SlotState::default())
    }
}

/// `tree`'s files as a commit nobody's branch holds: the index is a copy, so the child's staging is
/// untouched, and ignored files stay out as they do from a person's own `git add`.
pub fn snapshot(tree: &Path) -> Result<Sha, LaneError> {
    let index = git(
        tree,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    let (index, copy) = (PathBuf::from(index.trim()), temp_index(tree));
    let copied = std::fs::copy(&index, &copy);
    let run = |args: &[&str]| super::hostgit::git_indexed(tree, &copy, args);
    let result = copied
        .map_err(|error| LaneError::Forge(error.to_string()))
        .and_then(|_| {
            run(&["add", "-A"])?;
            let written = run(&["write-tree"])?;
            let commit = ["commit-tree", written.trim(), "-p", "HEAD", "-m", "check"];
            run(&[&identity(tree)[..], &commit[..]].concat())
        });
    let _gone_already_is_fine = std::fs::remove_file(&copy);
    sha_of(result?.trim())
}

fn temp_index(tree: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = format!("yi-check-index-{}-{n}", std::process::id());
    std::env::temp_dir().join(tree.file_name().map_or_else(
        || name.clone(),
        |dir| format!("{name}-{}", dir.to_string_lossy()),
    ))
}

/// Every check, since a contained process wrote this tree last. What it keeps is only what cargo
/// rebuilds from if it doubts: `.venv` and `node_modules` go, since no tool here re-verifies them.
pub fn scrub(tree: &Path) -> Result<(), LaneError> {
    git(tree, &["clean", "-qffdx", "-e", "target"])?;
    let profiles = std::fs::read_dir(tree.join("target"))
        .into_iter()
        .flatten()
        .flatten();
    for profile in profiles.filter(|entry| entry.path().is_dir()) {
        for entry in std::fs::read_dir(profile.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if KEPT_UNDER_PROFILE
                .iter()
                .any(|kept| entry.file_name() == *kept)
            {
                continue;
            }
            let path = entry.path();
            let removed = match entry.file_type() {
                Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
                _ => std::fs::remove_file(&path),
            };
            removed.map_err(super::io_error(&path))?;
        }
    }
    Ok(())
}

fn fresh_session() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "warm-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Flocks across processes have nothing to block on, so a full pool polls. Order is not FIFO, and
/// the wait is reported.
fn claim(
    warm: &Pool,
    commit: &Sha,
    queue_until: Instant,
) -> Result<Lane, Result<Duration, LaneError>> {
    let mut pause = BACKOFF_START;
    let base = || ClaimBase::Commit(commit.as_str().to_owned());
    loop {
        match warm.claim(&fresh_session(), base()) {
            Ok(lane) => return Ok(lane),
            Err(LaneError::PoolFull { .. }) if Instant::now() < queue_until => {
                std::thread::sleep(pause);
                pause = pause.saturating_mul(2).min(BACKOFF_MAX);
            }
            Err(LaneError::PoolFull { .. }) => return Err(Ok(pause)),
            Err(error) => return Err(Err(error)),
        }
    }
}

/// `command` on `commit` in a warm directory, contained through `checker`. `shown_as` replaces the
/// directory's path in the output, since the model has no use for a path it never worked in.
pub fn check_commit(
    warm: &Pool,
    commit: &Sha,
    shown_as: &Path,
    command: &str,
    queue_until: Instant,
    budget: Duration,
    checker: &crate::goal::Checker,
) -> WarmRun {
    let _span = yi_types::trace::span("tree.check");
    let begun = Instant::now();
    let stopped = |why: String| WarmRun {
        verdict: Verdict::Abstain { why },
        capture: None,
    };
    let lane = match claim(warm, commit, queue_until) {
        Ok(lane) => lane,
        Err(Ok(_)) => {
            return stopped(format!(
                "no warm directory came free in {} s; {n} of {n} held (node.slots {n})",
                begun.elapsed().as_secs(),
                n = warm.slots
            ));
        }
        Err(Err(error)) => return stopped(format!("no warm directory: {error}")),
    };
    let path = lane.path().to_path_buf();
    let prepared = scrub(&path).and_then(|()| {
        if !path.join("target").exists() {
            let _a_cold_build_is_slower_and_never_wrong =
                super::carry::carry_cargo(&warm.repo, &path, "warm");
        }
        super::toolchain::sync_tree(&path, &warm.home, &super::toolchain::detect(&path))
    });
    if let Err(error) = prepared {
        let _released_or_reclaimed_by_the_next_claim = lane.release_warm();
        return stopped(format!("the warm directory could not be prepared: {error}"));
    }
    let began = Instant::now();
    let until = began.checked_add(budget).unwrap_or(began);
    let capture = crate::goal::run_check_in(&path, command, &[], None, until, None, checker);
    let run = |verdict, capture| WarmRun { verdict, capture };
    let shown = path.to_string_lossy().into_owned();
    let result = match capture {
        Err(error) => run(Verdict::Abstain { why: error }, None),
        Ok(mut capture) => {
            let named = shown_as.to_string_lossy();
            capture.stdout = capture.stdout.replace(&shown, &named);
            capture.stderr = capture.stderr.replace(&shown, &named);
            let verdict = if capture.cancelled {
                let why = format!("the check ran past its {} s budget", budget.as_secs());
                Verdict::Abstain { why }
            } else if capture.exit_code == Some(0) {
                Verdict::Green
            } else if let Some(why) = checker.refusal(&path, command, &capture) {
                Verdict::Abstain { why }
            } else {
                Verdict::Red {
                    exit: capture.exit_code,
                }
            };
            run(verdict, Some(capture))
        }
    };
    let _released_or_reclaimed_by_the_next_claim = lane.release_warm();
    result
}
