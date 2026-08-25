use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::process::{CommandCapture, OUTPUT_CAP, command, run_captured};
use crate::tool::CancelFlag;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub u64);

impl std::fmt::Display for JobId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct JobReport {
    pub id: JobId,
    pub command: String,
    pub finished: bool,
    pub reported: bool,
    pub exit_code: Option<i32>,
    pub output: String,
}

struct Job {
    command: String,
    started: u64,
    reported: bool,
    capture: Option<CommandCapture>,
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

/// One registry per process: a backgrounded command outlives the tool call
/// that started it, and the next call has to be able to find it.
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

    fn insert(&self, command: &str) -> JobId {
        let id = self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let mut jobs = self.lock();
        jobs.insert(
            id,
            Job {
                command: command.to_owned(),
                started: id,
                reported: false,
                capture: None,
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

    pub fn report(&self, id: JobId) -> Option<JobReport> {
        self.lock().get(&id.0).map(|job| render(id.0, job))
    }

    pub fn latest(&self) -> Option<JobReport> {
        let jobs = self.lock();
        jobs.iter()
            .max_by_key(|(_, job)| job.started)
            .map(|(id, job)| render(*id, job))
    }

    /// Finished jobs nobody has been told about yet; marking them keeps the
    /// follow-up queue from repeating itself every poll.
    pub fn take_finished(&self) -> Vec<JobReport> {
        let mut jobs = self.lock();
        let mut out = Vec::new();
        for (id, job) in jobs.iter_mut() {
            if job.capture.is_some() && !job.reported {
                job.reported = true;
                out.push(render(*id, job));
            }
        }
        out.sort_by_key(|report| report.id);
        out
    }
}

fn render(id: u64, job: &Job) -> JobReport {
    let (finished, exit_code, output) = match &job.capture {
        Some(capture) => (
            true,
            capture.exit_code,
            crate::reduce::strip_ansi(&format!("{}{}", capture.stdout, capture.stderr)),
        ),
        None => (false, None, String::new()),
    };
    JobReport {
        id: JobId(id),
        command: job.command.clone(),
        finished,
        reported: job.reported,
        exit_code,
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
        if jobs.get(id).is_some_and(|job| job.capture.is_some()) {
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
}

/// D13: a foreground command that outlives `auto_background` keeps running as
/// a job instead of holding the turn. `None` disables it, which is the
/// default — a command that silently detaches is its own kind of surprise.
pub fn run_or_background(
    shell_command: &str,
    cwd: &Path,
    cancelled: &CancelFlag,
    auto_background: Option<Duration>,
) -> Result<Run, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let id = registry().insert(shell_command);
    let text = shell_command.to_owned();
    let dir = cwd.to_path_buf();
    let flag = Arc::clone(cancelled);
    std::thread::spawn(move || {
        let mut process = command("sh");
        process.arg("-c").arg(&text).current_dir(&dir);
        let capture = run_captured(process, None, &flag, OUTPUT_CAP);
        if let Ok(capture) = &capture {
            registry().finish(id, capture.clone());
        }
        let _receiver_may_be_gone = sender.send(capture);
    });
    let waited = match auto_background {
        Some(limit) => receiver
            .recv_timeout(limit)
            .map_err(|error| error.to_string()),
        None => receiver.recv().map_err(|error| error.to_string()),
    };
    match waited {
        Ok(Ok(capture)) => {
            if let Some(job) = registry().lock().get_mut(&id.0) {
                job.reported = true;
            }
            Ok(Run::Finished(Box::new(capture)))
        }
        Ok(Err(message)) => Err(message),
        Err(_still_running) => Ok(Run::Backgrounded(id)),
    }
}

/// "Check on job N" is the same tool with no command, clamped so a poll can
/// neither spin nor hang the turn.
pub fn clamp_wait(seconds: u64) -> Duration {
    Duration::from_secs(seconds.clamp(5, 300))
}
