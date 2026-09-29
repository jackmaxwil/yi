use std::path::{Path, PathBuf};

const BOOTSTRAP_LOCK_NAME: &str = ".bootstrap.flock";
const BOOTSTRAP_LOCK_STALE_WITHOUT_PID_MS: u128 = 30_000;
const SWEEP_AFTER_SECS: u64 = 86_400;

fn bootstrap_lock_path(venv: &Path) -> PathBuf {
    let name = venv
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    venv.with_file_name(format!("{name}{BOOTSTRAP_LOCK_NAME}"))
}

#[cfg(unix)]
pub fn process_is_running(pid: u32) -> std::io::Result<bool> {
    // kill -0 as the shell's builtin: slim images ship no kill(1), whose ENOENT read as dead.
    crate::bootstrap::command(Path::new("/bin/sh"))
        .args(["-c", r#"kill -0 "$1""#, "kill", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
}

#[cfg(not(unix))]
pub fn process_is_running(_pid: u32) -> std::io::Result<bool> {
    Ok(true)
}

/// A probe that could not run says nothing of the holder: the lock's age decides, as for no pid.
pub fn lock_is_stale(lock_dir: &Path, probe: Option<std::io::Result<bool>>) -> bool {
    match probe {
        Some(Ok(running)) => !running,
        Some(Err(_)) | None => lock_missing_pid_is_stale(lock_dir),
    }
}

pub fn read_lock_pid(lock_dir: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(lock_dir.join("pid")).ok()?;
    let pid: u32 = raw.trim().parse().ok()?;
    (pid > 0).then_some(pid)
}

pub fn lock_missing_pid_is_stale(lock_dir: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(lock_dir) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|age| age.as_millis() > BOOTSTRAP_LOCK_STALE_WITHOUT_PID_MS)
        .unwrap_or(false)
}

/// A flock, so the OS releases it with its holder: a boot killed mid-build frees the lock to
/// one waiter, where a pid file freed it to every waiter that read the dead pid.
pub(crate) struct BootstrapLock {
    path: PathBuf,
    _file: std::fs::File,
}

impl Drop for BootstrapLock {
    fn drop(&mut self) {
        // Unlinked before the file closes, so a waiter that wins it next sees it gone and reopens.
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_file(venv: &Path, block: bool) -> std::io::Result<Option<BootstrapLock>> {
    let path = bootstrap_lock_path(venv);
    loop {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let locked = if block {
            file.lock().map_err(std::fs::TryLockError::Error)
        } else {
            file.try_lock()
        };
        match locked {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        if names_file(&path, &file) {
            return Ok(Some(BootstrapLock { path, _file: file }));
        }
    }
}

#[cfg(unix)]
fn names_file(path: &Path, file: &std::fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(path), file.metadata()) {
        (Ok(named), Ok(held)) => named.dev() == held.dev() && named.ino() == held.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn names_file(path: &Path, _file: &std::fs::File) -> bool {
    path.exists()
}

pub(crate) fn acquire_bootstrap_lock(venv: &Path) -> Result<BootstrapLock, String> {
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    lock_file(venv, true)
        .and_then(|lock| lock.ok_or_else(|| std::io::Error::other("lock not taken")))
        .map_err(|error| format!("{}: {error}", bootstrap_lock_path(venv).display()))
}

fn try_bootstrap_lock(venv: &Path) -> Option<BootstrapLock> {
    lock_file(venv, false).ok().flatten()
}

fn running_commands() -> Option<String> {
    let output = crate::bootstrap::command(Path::new("ps"))
        .args(["-Aww", "-o", "command="])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[expect(
    clippy::disallowed_methods,
    reason = "a ready boot is the last use the sweep ages the venv by"
)]
pub(crate) fn restart_sweep_clock(venv: &Path) {
    let now = std::time::SystemTime::now();
    let _best_effort = std::fs::File::open(venv).and_then(|dir| dir.set_modified(now));
}

fn settled(venv: &Path) -> bool {
    std::fs::metadata(venv)
        .and_then(|meta| meta.modified())
        .is_ok_and(|at| {
            at.elapsed()
                .is_ok_and(|age| age.as_secs() > SWEEP_AFTER_SECS)
        })
}

pub fn remove_stale_venvs(current: &Path) -> Vec<PathBuf> {
    let Some(entries) = current
        .parent()
        .and_then(|parent| std::fs::read_dir(parent).ok())
    else {
        return Vec::new();
    };
    let stale: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            path.as_path() != current
                && settled(path)
                && name.is_some_and(|name| {
                    name.starts_with("kernel-venv-") && !name.ends_with(BOOTSTRAP_LOCK_NAME)
                })
        })
        .collect();
    if stale.is_empty() {
        return stale;
    }
    let Some(running) = running_commands() else {
        return Vec::new();
    };
    stale
        .into_iter()
        .filter(|venv| !running.contains(&format!("{}/", venv.display())))
        .filter(|venv| {
            try_bootstrap_lock(venv).is_some_and(|_held| std::fs::remove_dir_all(venv).is_ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    const TRIALS: usize = 10;
    const WAITERS: usize = 8;

    /// Incident (forge task 12770): a boot killed mid-build handed its lock to every waiter at
    /// once, and a second boot deleted the venv the first was installing into. The race is a
    /// window a round loses about half the time, so ten dead locks are raced in turn.
    #[test]
    fn a_killed_holder_passes_the_lock_to_one_waiter_at_a_time()
    -> Result<(), Box<dyn std::error::Error>> {
        const NAME: &str = "lock::tests::a_killed_holder_passes_the_lock_to_one_waiter_at_a_time";
        const HOLDER: &str = "holder-for:";
        let venv = |root: &Path, trial: usize| root.join(format!("kernel-venv-{trial}"));
        // The holder is this test rerun with a second filter that matches no test.
        if let Some(root) =
            std::env::args().find_map(|arg| arg.strip_prefix(HOLDER).map(PathBuf::from))
        {
            let _held = (0..TRIALS)
                .map(|trial| acquire_bootstrap_lock(&venv(&root, trial)))
                .collect::<Result<Vec<_>, _>>()?;
            std::fs::write(root.join("held"), "")?;
            std::thread::sleep(std::time::Duration::from_secs(60));
            return Ok(());
        }
        let root = Scratch::new("yi-kernel-lock")?;
        let mut holder = crate::bootstrap::command(&std::env::current_exe()?)
            .args(["--exact", NAME, &format!("{HOLDER}{}", root.display())])
            .stdout(std::process::Stdio::null())
            .spawn()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !root.join("held").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let held = root.join("held").exists();
        holder.kill()?;
        holder.wait()?;
        assert!(held, "the holder never took the locks");
        // Each round's waiters arrive together at one dead lock, as the retry and the rpc
        // tests' prewarming boots did; rounds run apart so one's timing cannot shade the next.
        let counts: Vec<_> = (0..TRIALS)
            .map(|_| {
                (
                    std::sync::atomic::AtomicUsize::new(0),
                    std::sync::atomic::AtomicUsize::new(0),
                )
            })
            .collect();
        for (trial, (inside, most)) in counts.iter().enumerate() {
            let start = std::sync::Barrier::new(WAITERS);
            let outcomes: Vec<Result<(), String>> = std::thread::scope(|scope| {
                let waiters: Vec<_> = (0..WAITERS)
                    .map(|_| {
                        scope.spawn(|| {
                            start.wait();
                            let _lock = acquire_bootstrap_lock(&venv(&root, trial))?;
                            let now = inside.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            most.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                            std::thread::sleep(std::time::Duration::from_millis(20));
                            inside.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            Ok(())
                        })
                    })
                    .collect();
                waiters
                    .into_iter()
                    .map(|waiter| waiter.join().unwrap_or_else(|_| Err("panicked".to_owned())))
                    .collect()
            });
            for outcome in outcomes {
                outcome?;
            }
        }
        let most: Vec<usize> = counts
            .iter()
            .map(|(_, most)| most.load(std::sync::atomic::Ordering::SeqCst))
            .collect();
        assert_eq!(
            most, [1; TRIALS],
            "two boots held one bootstrap lock at once"
        );
        Ok(())
    }
}
