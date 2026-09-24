use std::path::{Path, PathBuf};

const BOOTSTRAP_LOCK_NAME: &str = ".bootstrap.lock";
const BOOTSTRAP_LOCK_RETRY_MS: u64 = 100;
const BOOTSTRAP_LOCK_STALE_WITHOUT_PID_MS: u128 = 30_000;
const SWEEP_AFTER_SECS: u64 = 3_600;

pub(crate) fn bootstrap_lock_dir(venv: &Path) -> PathBuf {
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

pub(crate) struct BootstrapLock {
    dir: PathBuf,
}

impl Drop for BootstrapLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(crate) fn acquire_bootstrap_lock(venv: &Path) -> Result<BootstrapLock, String> {
    let lock_dir = bootstrap_lock_dir(venv);
    if let Some(parent) = lock_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    loop {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => {
                let _ = std::fs::write(lock_dir.join("pid"), format!("{}\n", std::process::id()));
                return Ok(BootstrapLock { dir: lock_dir });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&lock_dir, read_lock_pid(&lock_dir).map(process_is_running)) {
                    let _ = std::fs::remove_dir_all(&lock_dir);
                    continue;
                }
                std::thread::sleep(std::time::Duration::from_millis(BOOTSTRAP_LOCK_RETRY_MS));
            }
            Err(error) => return Err(format!("{}: {error}", lock_dir.display())),
        }
    }
}

fn try_bootstrap_lock(venv: &Path) -> Option<BootstrapLock> {
    let lock_dir = bootstrap_lock_dir(venv);
    for _ in 0..2 {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => {
                let _ = std::fs::write(lock_dir.join("pid"), format!("{}\n", std::process::id()));
                return Some(BootstrapLock { dir: lock_dir });
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && lock_is_stale(
                        &lock_dir,
                        read_lock_pid(&lock_dir).map(process_is_running),
                    ) =>
            {
                let _ = std::fs::remove_dir_all(&lock_dir);
            }
            Err(_) => return None,
        }
    }
    None
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
