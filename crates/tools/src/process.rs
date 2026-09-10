use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::jobs::LiveOutput;
use crate::tool::CancelFlag;

pub const OUTPUT_CAP: usize = 30_000;

pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[expect(
        clippy::disallowed_methods,
        reason = "process spawns live in yi-tools; every spawn routes through run_captured via this constructor"
    )]
    Command::new(program)
}

#[derive(Debug, Clone)]
pub struct CommandCapture {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
}

/// Kill the shell, then its process group: a grandchild outliving the shell holds the capture
/// pipes, so [`drain_capped`] waits out the cancelled command. `kill(1)`, no `libc` (§13.1).
fn kill_tree(child: &mut Child) {
    // Also the guard on the group kill below: `Child::kill` alone knows whether the child was
    // reaped, and a reaped pid can already name somebody else's group.
    if child.kill().is_err() {
        return;
    }
    #[cfg(unix)]
    {
        // Incident: `--` is load-bearing. BSD kill(1) reads a bare `-<pgid>` as a negative
        // pid, procps as a signal option, so on Linux the group survived silently.
        let _group_kill_best_effort = command("kill")
            .arg("-KILL")
            .arg("--")
            .arg(format!("-{}", child.id()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn drain_capped(mut reader: impl Read, cap: usize, live: Option<&LiveOutput>) -> (String, bool) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Some(live) = live {
                    live.push(chunk.get(..n).unwrap_or(&[]));
                }
                if buffer.len() < cap {
                    let take = n.min(cap.saturating_sub(buffer.len()));
                    buffer.extend_from_slice(chunk.get(..take).unwrap_or(&[]));
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
        }
    }
    (String::from_utf8_lossy(&buffer).into_owned(), truncated)
}

/// Incident: waiting under the guard parked the cancel watchdog on the same lock for the
/// child's whole life, so an early pipe-closer could not be killed. Released between polls.
fn wait_polled(child: &Mutex<Child>) -> Result<Option<i32>, String> {
    loop {
        let polled = child
            .lock()
            .map_err(|_| "child lock poisoned".to_owned())?
            .try_wait()
            .map_err(|error| format!("wait failed: {error}"))?;
        if let Some(status) = polled {
            return Ok(status.code());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn run_captured(
    command: Command,
    stdin: Option<Vec<u8>>,
    cancelled: &CancelFlag,
    cap: usize,
) -> Result<CommandCapture, String> {
    run_captured_live(command, stdin, cancelled, cap, None)
}

pub(crate) fn run_captured_live(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    cancelled: &CancelFlag,
    cap: usize,
    live: Option<Arc<LiveOutput>>,
) -> Result<CommandCapture, String> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The group `kill_tree` signals: descendants inherit it, and killing the
    // pid alone would leave them holding the pipes drained below.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn: {error}"))?;

    let stdin_writer = match (stdin, child.stdin.take()) {
        (Some(bytes), Some(mut pipe)) => Some(std::thread::spawn(move || {
            let _ignored_broken_pipe = pipe.write_all(&bytes);
        })),
        _ => None,
    };
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let child: Arc<Mutex<Child>> = Arc::new(Mutex::new(child));
    let done = Arc::new(AtomicBool::new(false));
    let was_cancelled = Arc::new(AtomicBool::new(false));

    let watchdog = {
        let child = Arc::clone(&child);
        let done = Arc::clone(&done);
        let was_cancelled = Arc::clone(&was_cancelled);
        let cancelled = Arc::clone(cancelled);
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if cancelled() {
                    was_cancelled.store(true, Ordering::SeqCst);
                    if let Ok(mut child) = child.lock() {
                        kill_tree(&mut child);
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };

    let stderr_live = live.clone();
    let stderr_reader = std::thread::spawn(move || match stderr_pipe {
        Some(pipe) => drain_capped(pipe, cap, stderr_live.as_deref()),
        None => (String::new(), false),
    });
    let (stdout, stdout_truncated) = match stdout_pipe {
        Some(pipe) => drain_capped(pipe, cap, live.as_deref()),
        None => (String::new(), false),
    };
    let (stderr, stderr_truncated) = stderr_reader
        .join()
        .unwrap_or_else(|_| (String::new(), false));

    let waited = wait_polled(&child);
    done.store(true, Ordering::SeqCst);
    let _watchdog_exits_on_done = watchdog.join();
    if let Some(writer) = stdin_writer {
        let _writer_done = writer.join();
    }

    Ok(CommandCapture {
        stdout,
        stderr,
        exit_code: waited?,
        cancelled: was_cancelled.load(Ordering::SeqCst),
        truncated: stdout_truncated || stderr_truncated,
    })
}

/// `$VISUAL`/`$EDITOR`, default `vi`, with the tty inherited.
pub fn edit_file(path: &std::path::Path) -> (String, std::io::Result<std::process::ExitStatus>) {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_owned());
    let status = command(&editor)
        .arg(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    (editor, status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn cancel_kills_a_child_that_closed_its_own_pipes() -> Fallible {
        let mut shell = command("sh");
        shell.arg("-c").arg("exec >/dev/null 2>&1; sleep 5");
        let deadline = Instant::now() + Duration::from_millis(300);
        let cancelled: CancelFlag = Arc::new(move || Instant::now() >= deadline);
        let started = Instant::now();
        let capture = run_captured(shell, None, &cancelled, OUTPUT_CAP)?;
        let elapsed = started.elapsed();
        assert!(capture.cancelled);
        assert_eq!(
            capture.exit_code, None,
            "the sleep exited on its own, so nothing was signalled"
        );
        assert!(elapsed < Duration::from_millis(2500), "waited {elapsed:?}");
        Ok(())
    }

    #[test]
    fn a_process_group_dies_with_no_kill_binary_on_path() -> Fallible {
        // The group kill runs in this process, so the test reruns itself under a PATH with no
        // `kill` on it, as in the benchmark images; the rerun makes the assertions.
        if std::env::var_os("PATH").is_none_or(|path| path != "/nonexistent") {
            let status = command(std::env::current_exe()?)
                .args([
                    "--exact",
                    "process::tests::a_process_group_dies_with_no_kill_binary_on_path",
                ])
                .env("PATH", "/nonexistent")
                .status()?;
            assert!(status.success(), "the rerun with no kill binary: {status}");
            return Ok(());
        }
        let mut pipeline = command("/bin/sh");
        pipeline.arg("-c").arg("/bin/sleep 30 | /bin/cat");
        let deadline = Instant::now() + Duration::from_millis(300);
        let cancelled: CancelFlag = Arc::new(move || Instant::now() >= deadline);
        let started = Instant::now();
        let capture = run_captured(pipeline, None, &cancelled, OUTPUT_CAP)?;
        let elapsed = started.elapsed();
        assert!(capture.cancelled);
        assert!(
            elapsed < Duration::from_secs(2),
            "the group outlived the kill and held stdout for {elapsed:?}"
        );
        Ok(())
    }
}
