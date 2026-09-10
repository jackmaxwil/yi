use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::process::{CommandCapture, OUTPUT_CAP, command, run_captured_live};
use crate::tool::CancelFlag;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub u64);

impl std::fmt::Display for JobId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("no such job: {0}")]
    UnknownJob(JobId),
}

/// How a job stopped. A signalled job is not a job that chose to fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Exited { code: Option<i32> },
    Killed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Settled(Outcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    Signalled,
    AlreadySettled(Outcome),
}

/// Every byte past `cursor`, plus how many were trimmed away before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputChunk {
    pub text: String,
    pub next: u64,
    pub dropped: u64,
}

#[derive(Debug, Clone)]
pub struct JobReport {
    pub id: JobId,
    pub command: String,
    pub finished: bool,
    pub reported: bool,
    pub exit_code: Option<i32>,
    pub state: JobState,
    pub output: String,
}

/// Who settles the job: the two-second follow-up poller, or one handle that
/// asked for it by [`spawn_job`] and will [`Jobs::release`] it itself.
enum Reaper {
    Poller { reported: bool },
    Handle,
}

struct Job {
    command: String,
    started: u64,
    reaper: Reaper,
    capture: Option<CommandCapture>,
    live: Arc<LiveOutput>,
    kill: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct LiveBuffer {
    bytes: Vec<u8>,
    // Invariant: the absolute stream offset of `bytes[0]`. Trimming drops the oldest bytes,
    // so a cursor below this names bytes that are gone and the gap is how many.
    start: u64,
}

/// A running job's output so far, bounded by [`crate::process::OUTPUT_CAP`].
#[derive(Debug, Default)]
pub(crate) struct LiveOutput {
    inner: Mutex<LiveBuffer>,
}

impl LiveOutput {
    fn lock(&self) -> std::sync::MutexGuard<'_, LiveBuffer> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn push(&self, chunk: &[u8]) {
        let mut buffer = self.lock();
        buffer.bytes.extend_from_slice(chunk);
        let excess = buffer.bytes.len().saturating_sub(OUTPUT_CAP);
        if excess > 0 {
            buffer.bytes.drain(..excess);
            let excess = u64::try_from(excess).unwrap_or(u64::MAX);
            buffer.start = buffer.start.saturating_add(excess);
        }
    }

    fn since(&self, cursor: u64) -> OutputChunk {
        let buffer = self.lock();
        let held = u64::try_from(buffer.bytes.len()).unwrap_or(u64::MAX);
        let from = usize::try_from(cursor.saturating_sub(buffer.start))
            .unwrap_or(usize::MAX)
            .min(buffer.bytes.len());
        OutputChunk {
            text: crate::reduce::strip_ansi(&String::from_utf8_lossy(
                buffer.bytes.get(from..).unwrap_or(&[]),
            )),
            next: buffer.start.saturating_add(held),
            dropped: buffer.start.saturating_sub(cursor),
        }
    }
}

// T12: eviction is LRU with the eight most recent protected — an idle timer
// would kill a quiet `cargo build` that is still the point of the turn.
const PROTECTED: usize = 8;
const MAX_JOBS: usize = 32;

#[derive(Default)]
pub struct Jobs {
    inner: Mutex<HashMap<u64, Job>>,
    next: AtomicU64,
}

/// One per process: a backgrounded command outlives the call that started it.
pub fn registry() -> &'static Jobs {
    static REGISTRY: OnceLock<Jobs> = OnceLock::new();
    REGISTRY.get_or_init(Jobs::default)
}

impl Jobs {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Job>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn insert(
        &self,
        command: &str,
        reaper: Reaper,
        live: Arc<LiveOutput>,
        kill: Arc<AtomicBool>,
    ) -> JobId {
        let id = self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let mut jobs = self.lock();
        jobs.insert(
            id,
            Job {
                command: command.to_owned(),
                started: id,
                reaper,
                capture: None,
                live,
                kill,
            },
        );
        evict(&mut jobs);
        JobId(id)
    }

    fn finish(&self, id: JobId, capture: CommandCapture) {
        if let Some(job) = self.lock().get_mut(&id.0) {
            job.capture = Some(capture);
        }
    }

    fn mark_reported(&self, id: JobId) {
        if let Some(job) = self.lock().get_mut(&id.0)
            && let Reaper::Poller { reported } = &mut job.reaper
        {
            *reported = true;
        }
    }

    pub fn report(&self, id: JobId) -> Option<JobReport> {
        self.lock().get(&id.0).map(|job| render(id.0, job))
    }

    pub fn latest(&self) -> Option<JobReport> {
        let jobs = self.lock();
        jobs.iter()
            .max_by_key(|(_, job)| job.started)
            .map(|(id, job)| render(*id, job))
    }

    /// Output produced since `cursor`, readable while the job still runs and
    /// re-readable from any cursor: reading advances nothing on its own.
    pub fn output_since(&self, id: JobId, cursor: u64) -> Result<OutputChunk, JobError> {
        let jobs = self.lock();
        let job = jobs.get(&id.0).ok_or(JobError::UnknownJob(id))?;
        Ok(job.live.since(cursor))
    }

    /// Terminates the child and its group; a job that already settled is left
    /// alone and says how it settled.
    pub fn kill(&self, id: JobId) -> Result<KillOutcome, JobError> {
        let jobs = self.lock();
        let job = jobs.get(&id.0).ok_or(JobError::UnknownJob(id))?;
        match state(job) {
            JobState::Settled(outcome) => Ok(KillOutcome::AlreadySettled(outcome)),
            JobState::Running => {
                job.kill.store(true, Ordering::SeqCst);
                Ok(KillOutcome::Signalled)
            }
        }
    }

    /// Drops a [`spawn_job`] job from the registry once its handle is done with
    /// it. A still-running job keeps running: [`Jobs::kill`] it first.
    pub fn release(&self, id: JobId) -> Result<JobReport, JobError> {
        let job = self.lock().remove(&id.0).ok_or(JobError::UnknownJob(id))?;
        Ok(render(id.0, &job))
    }

    /// Marking them keeps the follow-up queue from repeating every poll.
    pub fn take_finished(&self) -> Vec<JobReport> {
        let mut jobs = self.lock();
        let mut out = Vec::new();
        for (id, job) in jobs.iter_mut() {
            if job.capture.is_some() && matches!(job.reaper, Reaper::Poller { reported: false }) {
                job.reaper = Reaper::Poller { reported: true };
                out.push(render(*id, job));
            }
        }
        out.sort_by_key(|report| report.id);
        out
    }
}

fn state(job: &Job) -> JobState {
    match &job.capture {
        None => JobState::Running,
        Some(capture) if capture.cancelled => JobState::Settled(Outcome::Killed),
        Some(capture) => JobState::Settled(Outcome::Exited {
            code: capture.exit_code,
        }),
    }
}

fn render(id: u64, job: &Job) -> JobReport {
    let state = state(job);
    let output = match &job.capture {
        Some(capture) => {
            crate::reduce::strip_ansi(&format!("{}{}", capture.stdout, capture.stderr))
        }
        None => String::new(),
    };
    JobReport {
        id: JobId(id),
        command: job.command.clone(),
        finished: matches!(state, JobState::Settled(_)),
        reported: match &job.reaper {
            Reaper::Poller { reported } => *reported,
            Reaper::Handle => true,
        },
        exit_code: match state {
            JobState::Settled(Outcome::Exited { code }) => code,
            JobState::Settled(Outcome::Killed) | JobState::Running => None,
        },
        state,
        output,
    }
}

fn evict(jobs: &mut HashMap<u64, Job>) {
    if jobs.len() <= MAX_JOBS {
        return;
    }
    let mut ids: Vec<u64> = jobs.keys().copied().collect();
    ids.sort_unstable();
    let protected_from = ids.len().saturating_sub(PROTECTED);
    for id in ids.iter().take(protected_from) {
        if jobs
            .get(id)
            .is_some_and(|job| job.capture.is_some() && matches!(job.reaper, Reaper::Poller { .. }))
        {
            jobs.remove(id);
        }
        if jobs.len() <= MAX_JOBS {
            return;
        }
    }
}

pub enum Run {
    Finished(Box<CommandCapture>),
    Backgrounded(JobId),
    TimedOut(Box<CommandCapture>),
}

fn start(
    shell_command: &str,
    cwd: &Path,
    cancelled: &CancelFlag,
    sandbox: Option<&crate::sandbox::Sandbox>,
    reaper: Reaper,
) -> (
    JobId,
    std::sync::mpsc::Receiver<Result<CommandCapture, String>>,
) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let live = Arc::new(LiveOutput::default());
    let kill = Arc::new(AtomicBool::new(false));
    let id = registry().insert(shell_command, reaper, Arc::clone(&live), Arc::clone(&kill));
    let text = shell_command.to_owned();
    let dir = cwd.to_path_buf();
    let outer = Arc::clone(cancelled);
    let flag: CancelFlag = Arc::new(move || kill.load(Ordering::SeqCst) || outer());
    let wrapped = sandbox.map(|sandbox| sandbox.wrap("sh", &["-c", shell_command]));
    std::thread::spawn(move || {
        let mut process = match &wrapped {
            Some((program, args)) => {
                let mut process = command(program);
                process.args(args);
                process
            }
            None => {
                let mut process = command("sh");
                process.arg("-c").arg(&text);
                process
            }
        };
        process.current_dir(&dir);
        let capture = run_captured_live(process, None, &flag, OUTPUT_CAP, Some(live));
        if let Ok(capture) = &capture {
            registry().finish(id, capture.clone());
        }
        let _receiver_may_be_gone = sender.send(capture);
    });
    (id, receiver)
}

/// A job owned by its caller's handle: never announced by another session's poller and never
/// evicted, so only [`Jobs::release`] retires it. Returns at once; the child has a thread.
pub fn spawn_job(
    shell_command: &str,
    cwd: &Path,
    cancelled: &CancelFlag,
    sandbox: Option<&crate::sandbox::Sandbox>,
) -> JobId {
    start(shell_command, cwd, cancelled, sandbox, Reaper::Handle).0
}

/// A command outliving `auto_background` keeps running as a job instead of holding the turn
/// (`None` disables it: silently detaching surprises); past `timeout` it is killed instead.
pub fn run_or_background(
    shell_command: &str,
    cwd: &Path,
    cancelled: &CancelFlag,
    auto_background: Option<Duration>,
    timeout: Duration,
    sandbox: Option<&crate::sandbox::Sandbox>,
) -> Result<Run, String> {
    let (id, receiver) = start(
        shell_command,
        cwd,
        cancelled,
        sandbox,
        Reaper::Poller { reported: false },
    );
    let background = auto_background.filter(|limit| *limit <= timeout);
    match receiver.recv_timeout(background.unwrap_or(timeout)) {
        Ok(Ok(capture)) => {
            registry().mark_reported(id);
            Ok(Run::Finished(Box::new(capture)))
        }
        Ok(Err(message)) => Err(message),
        Err(RecvTimeoutError::Timeout) if background.is_some() => Ok(Run::Backgrounded(id)),
        Err(RecvTimeoutError::Timeout) => {
            let _a_job_that_settled_meanwhile_is_fine = registry().kill(id);
            // Incident: `timeout 3000 python3 …` left the shell's group and held the pipes
            // for 43 minutes; the kill gets five seconds, then the turn moves on.
            match receiver.recv_timeout(KILL_GRACE) {
                Ok(Ok(capture)) => {
                    registry().mark_reported(id);
                    Ok(Run::TimedOut(Box::new(capture)))
                }
                Ok(Err(message)) => Err(message),
                Err(_) => Ok(Run::TimedOut(Box::new(CommandCapture {
                    stdout: String::new(),
                    stderr: format!(
                        "[the process outlived the kill and keeps running as job {id}; its output arrives as a job result]"
                    ),
                    exit_code: None,
                    cancelled: true,
                    truncated: false,
                    kill_error: None,
                }))),
            }
        }
        Err(RecvTimeoutError::Disconnected) => {
            Err("command thread ended without a result".to_owned())
        }
    }
}

/// Clamped so a poll can neither spin nor hang the turn.
pub fn clamp_wait(seconds: u64) -> Duration {
    Duration::from_secs(seconds.clamp(5, 300))
}

pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
const KILL_GRACE: Duration = Duration::from_secs(5);
/// One sixth of a one-hour attempt: room for a cold build or a whole suite, and twice the
/// wait clamp, so anything longer is already a job.
pub const MAX_TIMEOUT_SECS: u64 = 600;

/// Absent or zero is the default; the ceiling holds whatever the model asks.
pub fn clamp_timeout(seconds: Option<u64>) -> Duration {
    Duration::from_secs(
        seconds
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::path::PathBuf;
    use std::time::Instant;

    type Fallible = Result<(), Box<dyn Error>>;

    fn never() -> CancelFlag {
        Arc::new(|| false)
    }

    fn scratch() -> Result<PathBuf, Box<dyn Error>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "yi-jobs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn poll_until<T>(mut probe: impl FnMut() -> Option<T>) -> Result<T, Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Some(value) = probe() {
                return Ok(value);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("timed out waiting for the job state under test".into())
    }

    fn settled(id: JobId) -> Result<Outcome, Box<dyn Error>> {
        poll_until(|| match registry().report(id)?.state {
            JobState::Settled(outcome) => Some(outcome),
            JobState::Running => None,
        })
    }

    #[test]
    fn timeout_secs_defaults_and_clamps() {
        assert_eq!(
            clamp_timeout(None),
            Duration::from_secs(DEFAULT_TIMEOUT_SECS)
        );
        assert_eq!(
            clamp_timeout(Some(0)),
            Duration::from_secs(DEFAULT_TIMEOUT_SECS)
        );
        assert_eq!(clamp_timeout(Some(30)), Duration::from_secs(30));
        assert_eq!(
            clamp_timeout(Some(10_000)),
            Duration::from_secs(MAX_TIMEOUT_SECS)
        );
    }

    #[test]
    fn output_is_readable_before_the_job_exits() -> Fallible {
        let dir = scratch()?;
        let id = spawn_job("printf hello; sleep 300", &dir, &never(), None);
        let chunk = poll_until(|| {
            let chunk = registry().output_since(id, 0).ok()?;
            chunk.text.contains("hello").then_some(chunk)
        })?;
        assert_eq!(chunk.dropped, 0);
        assert_eq!(
            registry().report(id).map(|report| report.state),
            Some(JobState::Running)
        );
        registry().kill(id)?;
        registry().release(id)?;
        Ok(())
    }

    #[test]
    fn a_second_read_returns_only_what_is_new() -> Fallible {
        let dir = scratch()?;
        let gate = dir.join("gate");
        let id = spawn_job(
            &format!(
                "printf aaa; until [ -f {} ]; do sleep 1; done; printf bbb",
                gate.display()
            ),
            &dir,
            &never(),
            None,
        );
        let first = poll_until(|| {
            let chunk = registry().output_since(id, 0).ok()?;
            chunk.text.contains("aaa").then_some(chunk)
        })?;
        let idle = registry().output_since(id, first.next)?;
        assert_eq!(idle.text, "");
        assert_eq!(idle.next, first.next);
        assert_eq!(registry().output_since(id, 0)?.text, first.text);

        std::fs::write(&gate, b"")?;
        let second = poll_until(|| {
            let chunk = registry().output_since(id, first.next).ok()?;
            chunk.text.contains("bbb").then_some(chunk)
        })?;
        assert_eq!(second.text, "bbb");
        assert_eq!(second.dropped, 0);
        assert!(second.next > first.next);
        settled(id)?;
        registry().release(id)?;
        Ok(())
    }

    #[test]
    fn kill_stops_a_sleeper_and_reports_killed() -> Fallible {
        let dir = scratch()?;
        let id = spawn_job("sleep 300", &dir, &never(), None);
        assert_eq!(registry().kill(id)?, KillOutcome::Signalled);
        assert_eq!(settled(id)?, Outcome::Killed);
        let report = registry().release(id)?;
        assert!(report.finished);
        assert_eq!(report.exit_code, None);
        Ok(())
    }

    #[test]
    fn kill_after_exit_is_a_no_op() -> Fallible {
        let dir = scratch()?;
        let id = spawn_job("exit 3", &dir, &never(), None);
        let exited = Outcome::Exited { code: Some(3) };
        assert_eq!(settled(id)?, exited);
        assert_eq!(registry().kill(id)?, KillOutcome::AlreadySettled(exited));
        assert_eq!(registry().kill(id)?, KillOutcome::AlreadySettled(exited));
        registry().release(id)?;
        assert_eq!(registry().kill(id), Err(JobError::UnknownJob(id)));
        Ok(())
    }

    #[test]
    fn the_live_buffer_trims_and_says_it_trimmed() -> Fallible {
        let dir = scratch()?;
        let id = spawn_job(
            "awk 'BEGIN{for(i=0;i<5000;i++) print \"0123456789\"}'",
            &dir,
            &never(),
            None,
        );
        settled(id)?;
        let chunk = registry().output_since(id, 0)?;
        let cap = u64::try_from(OUTPUT_CAP)?;
        assert!(chunk.next > cap);
        assert_eq!(chunk.dropped, chunk.next.saturating_sub(cap));
        assert_eq!(u64::try_from(chunk.text.len())?, cap);
        registry().release(id)?;
        Ok(())
    }
}
