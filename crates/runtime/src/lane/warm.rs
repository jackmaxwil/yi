//! Checks on warm directories: a per-repository pool of `node.slots` trees whose cargo output
//! outlives a check, scrubbed to cargo's bookkeeping so no borrower reads another's files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use super::hostgit::{git, identity, sha_of};
use super::{ClaimBase, Lane, LaneError, Pool, Sha};

/// How long a check may wait for a directory when the session names no deadline.
pub const CHECK_QUEUE: Duration = Duration::from_secs(3_600);
const TICKETS_MAX: usize = 256;
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
    pub waited: Duration,
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
        git(&self.path, &["reset", "-q", "--hard"])?;
        scrub(&self.path)?;
        let branch = self.branch.as_str().to_owned();
        self.pool.detach(self.slot, None)?;
        let _kept_only_by_its_own_name = git(&self.pool.repo, &["branch", "-q", "-D", &branch]);
        self.pool.clear_session(self.slot)
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
    let run = |args: &[&str]| -> Result<String, LaneError> {
        let mut command = yi_tools::host_command("git", tree)
            .map_err(|error| LaneError::Forge(error.to_string()))?;
        command
            .current_dir(tree)
            .env("GIT_INDEX_FILE", &copy)
            .args(args);
        let never: yi_tools::CancelFlag = Arc::new(|| false);
        let capture = yi_tools::run_captured(command, None, &never, super::PLUMBING_CAP)
            .map_err(LaneError::Forge)?;
        match capture.exit_code {
            Some(0) => Ok(capture.stdout),
            _ => Err(LaneError::Forge(format!(
                "git {args:?}: {}",
                capture.stderr
            ))),
        }
    };
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
/// rebuilds from if it doubts, so a stale binary cannot pass and an ignored report cannot be read.
pub fn scrub(tree: &Path) -> Result<(), LaneError> {
    git(
        tree,
        &[
            "clean",
            "-qffdx",
            "-e",
            "target",
            "-e",
            ".venv",
            "-e",
            "node_modules",
        ],
    )?;
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
    let stopped = |why: String, begun: Instant| WarmRun {
        verdict: Verdict::Abstain { why },
        capture: None,
        waited: begun.elapsed(),
    };
    let lane = match claim(warm, commit, queue_until) {
        Ok(lane) => lane,
        Err(Ok(_)) => {
            return stopped(
                format!(
                    "no warm directory came free in {} s; {n} of {n} held (node.slots {n})",
                    begun.elapsed().as_secs(),
                    n = warm.slots
                ),
                begun,
            );
        }
        Err(Err(error)) => return stopped(format!("no warm directory: {error}"), begun),
    };
    let waited = begun.elapsed();
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
        return stopped(
            format!("the warm directory could not be prepared: {error}"),
            begun,
        );
    }
    let began = Instant::now();
    let until = began.checked_add(budget).unwrap_or(began);
    let capture = crate::goal::run_check_in(&path, command, &[], None, until, None, checker);
    let run = |verdict, capture| WarmRun {
        verdict,
        capture,
        waited,
    };
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

/// One run per tree and command, which outlives the cell that started it and is read once.
#[derive(Default)]
struct Ticket {
    done: Mutex<Option<WarmRun>>,
    settled: Condvar,
}

/// What asking about a check answered.
#[derive(Debug, Clone)]
pub enum Asked {
    Finished(WarmRun),
    /// The notice names how to read it: a cell's 600 s abort does not end the run.
    Waiting {
        row: String,
    },
}

type Tickets = HashMap<(String, String), (Arc<Ticket>, Instant)>;

fn tickets() -> &'static Mutex<Tickets> {
    static TICKETS: OnceLock<Mutex<Tickets>> = OnceLock::new();
    TICKETS.get_or_init(Mutex::default)
}

/// The run for `(tree, command)`: started once, waited on up to `wait`, handed back once.
/// `start` runs on its own thread; a refused thread or a full table is an `Abstain`.
pub fn ticket(
    tree: &str,
    command: &str,
    wait: Duration,
    start: impl FnOnce() -> WarmRun + Send + 'static,
) -> Asked {
    let key = (tree.to_owned(), command.to_owned());
    let abstain = |why: String| WarmRun {
        verdict: Verdict::Abstain { why },
        capture: None,
        waited: Duration::ZERO,
    };
    let (held, begun) = {
        let mut table = tickets().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((held, begun)) = table.get(&key) {
            (Arc::clone(held), *begun)
        } else {
            if table.len() >= TICKETS_MAX {
                return Asked::Finished(abstain(format!(
                    "[… {} checks unread (check.tickets {TICKETS_MAX}); read a finished one with its own call, or discard its tree]",
                    table.len()
                )));
            }
            let held = Arc::new(Ticket::default());
            let (filled, begun) = (Arc::clone(&held), Instant::now());
            let spawned = std::thread::Builder::new()
                .name("yi-tree-check".to_owned())
                .spawn(move || {
                    let run = start();
                    *filled.done.lock().unwrap_or_else(PoisonError::into_inner) = Some(run);
                    filled.settled.notify_all();
                });
            if let Err(error) = spawned {
                return Asked::Finished(abstain(format!("no thread for the check: {error}")));
            }
            table.insert(key.clone(), (Arc::clone(&held), begun));
            (held, begun)
        }
    };
    let done = held.done.lock().unwrap_or_else(PoisonError::into_inner);
    let (mut done, _timed_out) = held
        .settled
        .wait_timeout_while(done, wait, |run| run.is_none())
        .unwrap_or_else(PoisonError::into_inner);
    match done.take() {
        Some(run) => {
            drop(done);
            tickets()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&key);
            Asked::Finished(run)
        }
        None => Asked::Waiting {
            row: format!(
                "[check still running after {} s, in a warm directory or waiting for one (node.slots); the same call again reads this run]",
                begun.elapsed().as_secs()
            ),
        },
    }
}

/// A tree gone takes its unread runs with it.
pub fn forget_tickets(tree: &str) {
    let mut table = tickets().lock().unwrap_or_else(PoisonError::into_inner);
    table.retain(|(owner, _), _| owner != tree);
}
