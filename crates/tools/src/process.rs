use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// SIGKILL the whole process group: killing the shell alone leaves grandchildren
/// holding the capture pipes, so `drain_capped` — and the interrupted turn —
/// waits out the very command it cancelled. `kill(1)`, not a `libc` dep (§13.1).
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    let group_killed = command("kill")
        .arg("-KILL")
        .arg(format!("-{}", child.id()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    #[cfg(not(unix))]
    let group_killed = false;
    if !group_killed {
        let _kill_best_effort = child.kill();
    }
}

fn drain_capped(mut reader: impl Read, cap: usize) -> (String, bool) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
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

pub fn run_captured(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    cancelled: &CancelFlag,
    cap: usize,
) -> Result<CommandCapture, String> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The group `kill_tree` signals; without it the kill reaches only the shell.
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

    let stderr_reader = std::thread::spawn(move || match stderr_pipe {
        Some(pipe) => drain_capped(pipe, cap),
        None => (String::new(), false),
    });
    let (stdout, stdout_truncated) = match stdout_pipe {
        Some(pipe) => drain_capped(pipe, cap),
        None => (String::new(), false),
    };
    let (stderr, stderr_truncated) = stderr_reader
        .join()
        .unwrap_or_else(|_| (String::new(), false));

    let exit_code = child
        .lock()
        .map_err(|_| "child lock poisoned".to_owned())
        .and_then(|mut child| {
            child
                .wait()
                .map_err(|error| format!("wait failed: {error}"))
        })
        .map(|status| status.code())?;
    done.store(true, Ordering::SeqCst);
    let _watchdog_exits_on_done = watchdog.join();
    if let Some(writer) = stdin_writer {
        let _writer_done = writer.join();
    }

    Ok(CommandCapture {
        stdout,
        stderr,
        exit_code,
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
