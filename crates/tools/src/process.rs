use std::collections::VecDeque;
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
    pub kill_error: Option<String>,
}

/// Walk the descendants, then kill the group, the shell and each walked pid: a live grandchild
/// holds the capture pipes. The group dies before the shell: macOS refuses a zombie group (EPERM).
fn kill_tree(child: &mut Child) -> Result<(), String> {
    // Walked before any signal, while a re-grouped child (GNU timeout's) still has its parent.
    // ponytail: one orphaned before the cancel escapes; a subreaper or cgroup needs libc (§18.3).
    #[cfg(unix)]
    let tree = descendants(child.id());
    #[cfg(unix)]
    let group = group_kill(child.id());
    let shell = child
        .kill()
        .map_err(|error| format!("kill {}: {error}", child.id()));
    #[cfg(unix)]
    let tree = tree.and_then(|pids| pid_kill(&pids));
    #[cfg(unix)]
    group?;
    #[cfg(unix)]
    tree?;
    shell
}

/// Every pid whose parent chain reaches `root`; a fork mid-walk is left to the group kill.
/// ponytail: linear scans over a table of hundreds; a HashSet if a tree reaches thousands.
#[cfg(unix)]
fn descendants(root: u32) -> Result<Vec<u32>, String> {
    let table = process_table()?;
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid) in &table {
            if ppid == parent && !found.contains(&pid) {
                found.push(pid);
                frontier.push(pid);
            }
        }
    }
    Ok(found)
}

/// `(pid, ppid)` for every visible process. `/proc/<pid>/stat` comm may hold
/// spaces and parens, so the fields are split right of the closing paren.
#[cfg(target_os = "linux")]
fn process_table() -> Result<Vec<(u32, u32)>, String> {
    let entries = std::fs::read_dir("/proc").map_err(|error| format!("/proc: {error}"))?;
    Ok(entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let (_, rest) = stat.rsplit_once(") ")?;
            let ppid = rest.split_whitespace().nth(1)?.parse::<u32>().ok()?;
            Some((pid, ppid))
        })
        .collect())
}

/// macOS has no `/proc`; ps(1) is on every macOS. Slim Linux images that lack
/// ps are the benchmark target, and those read `/proc` above.
#[cfg(all(unix, not(target_os = "linux")))]
fn process_table() -> Result<Vec<(u32, u32)>, String> {
    let output = command("/bin/ps")
        .args(["-eo", "pid=,ppid="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("/bin/ps: {error}"))?;
    if !output.status.success() {
        return Err(format!("/bin/ps -eo pid=,ppid=: {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect())
}

/// A walked pid already killed and reaped fails its own operand, so the exit status says nothing.
/// ponytail: a pid reused inside the walk-to-kill window takes the KILL; pids allocate upward.
#[cfg(unix)]
fn pid_kill(pids: &[u32]) -> Result<(), String> {
    if pids.is_empty() {
        return Ok(());
    }
    command("/bin/sh")
        .arg("-c")
        .arg(r#"kill -KILL "$@""#)
        .arg("kill")
        .args(pids.iter().map(u32::to_string))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(drop)
        .map_err(|error| format!("/bin/sh: {error}"))
}

/// Incident: slim images ship no `kill(1)`, and its dropped ENOENT left timed-out `python3`
/// jobs running. No `libc` (§18.3): the shell's builtin, in the one form dash and bash parse.
#[cfg(unix)]
fn group_kill(pgid: u32) -> Result<(), String> {
    let status = command("/bin/sh")
        .args(["-c", r#"kill -KILL "-$1""#, "kill", &pgid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("/bin/sh: {error}"))?;
    if status.success() {
        return Ok(());
    }
    Err(format!("kill -KILL -{pgid}: {status}"))
}

/// Incident: keeping only the first `cap` bytes lost a long build's last lines, where its verdict
/// is. The first half and a rolling last half are kept, and the middle is counted.
fn drain_capped(mut reader: impl Read, cap: usize, live: Option<&LiveOutput>) -> (String, bool) {
    let head_cap = cap / 2;
    let tail_cap = cap.saturating_sub(head_cap);
    let mut head = Vec::new();
    let mut tail = VecDeque::new();
    let mut omitted: usize = 0;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let bytes = chunk.get(..n).unwrap_or(&[]);
                if let Some(live) = live {
                    live.push(bytes);
                }
                let room = head_cap.saturating_sub(head.len());
                head.extend(bytes.iter().take(room));
                tail.extend(bytes.iter().skip(room));
                let excess = tail.len().saturating_sub(tail_cap);
                tail.drain(..excess);
                omitted = omitted.saturating_add(excess);
            }
        }
    }
    let tail = tail.make_contiguous();
    if omitted == 0 {
        head.extend_from_slice(tail);
        return (String::from_utf8_lossy(&head).into_owned(), false);
    }
    let head = String::from_utf8_lossy(&head);
    let tail = String::from_utf8_lossy(tail);
    (
        format!("{head}\n[{omitted} bytes omitted from the middle]\n{tail}"),
        true,
    )
}

/// Incident: waiting under the guard parked the cancel watchdog on the same lock, so an early
/// pipe-closer could not be killed. Released between polls; a reap sets `done` under it.
fn wait_polled(child: &Mutex<Child>, done: &AtomicBool) -> Result<Option<i32>, String> {
    let mut interval = Duration::from_millis(1);
    loop {
        let mut guard = child.lock().map_err(|_| "child lock poisoned".to_owned())?;
        let polled = guard
            .try_wait()
            .map_err(|error| format!("wait failed: {error}"))?;
        if let Some(status) = polled {
            done.store(true, Ordering::SeqCst);
            return Ok(status.code());
        }
        drop(guard);
        std::thread::sleep(interval);
        interval = interval.saturating_mul(2).min(Duration::from_millis(20));
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

    // Incident: a 50 ms watchdog sleep, joined after the reap, floored every command at 50 ms.
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let watchdog = {
        let child = Arc::clone(&child);
        let done = Arc::clone(&done);
        let was_cancelled = Arc::clone(&was_cancelled);
        let cancelled = Arc::clone(cancelled);
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if cancelled() {
                    was_cancelled.store(true, Ordering::SeqCst);
                    // A poisoned lock surfaces from `wait_polled` below; a reaped pid is not ours.
                    return child
                        .lock()
                        .ok()
                        .filter(|_| !done.load(Ordering::SeqCst))
                        .and_then(|mut child| kill_tree(&mut child).err());
                }
                match stopped.recv_timeout(Duration::from_millis(50)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    _ => return None,
                }
            }
            None
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

    let waited = wait_polled(&child, &done);
    done.store(true, Ordering::SeqCst);
    drop(stop);
    let kill_error = watchdog
        .join()
        .unwrap_or_else(|_| Some("the cancel watchdog panicked".to_owned()));
    if let Some(writer) = stdin_writer {
        let _writer_done = writer.join();
    }

    Ok(CommandCapture {
        stdout,
        stderr,
        exit_code: waited?,
        cancelled: was_cancelled.load(Ordering::SeqCst),
        truncated: stdout_truncated || stderr_truncated,
        kill_error,
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
    fn an_uncut_stream_is_decoded_whole() {
        // Twelve two-byte characters: the 17-byte head of a 35-byte cap ends inside the ninth.
        let text = "é".repeat(12);
        assert_eq!(drain_capped(text.as_bytes(), 35, None), (text, false));
    }

    #[test]
    fn a_quick_command_is_not_held_by_the_watchdog() -> Fallible {
        let never: CancelFlag = Arc::new(|| false);
        let started = Instant::now();
        for _ in 0..5 {
            let capture = run_captured(command("true"), None, &never, OUTPUT_CAP)?;
            assert_eq!(capture.exit_code, Some(0));
        }
        // The watchdog's 50 ms sleep used to floor each of these at 50 ms.
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(200),
            "five runs took {elapsed:?}"
        );
        Ok(())
    }

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
            let rerun = command(std::env::current_exe()?)
                .args([
                    "--exact",
                    "process::tests::a_process_group_dies_with_no_kill_binary_on_path",
                ])
                .env("PATH", "/nonexistent")
                .output()?;
            // libtest exits 0 on a filter that matches nothing, so the rerun must say it ran one.
            let stdout = String::from_utf8_lossy(&rerun.stdout);
            assert!(
                rerun.status.success() && stdout.contains("1 passed"),
                "{stdout}"
            );
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

    #[test]
    fn a_grandchild_that_left_the_group_dies_with_the_tree() -> Fallible {
        // GNU timeout's setpgid, as python 3.2+ spells it. The grandchild inherits stdout, so
        // while it lives the drain cannot finish; no python3 means no "ready", a failure.
        let mut shell = command("sh");
        shell.arg("-c").arg(
            "python3 -c 'import os, subprocess, time; subprocess.Popen([\"sleep\", \"30\"], preexec_fn=os.setpgrp); print(\"ready\", flush=True); time.sleep(30)'",
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let cancelled: CancelFlag = Arc::new(move || Instant::now() >= deadline);
        let started = Instant::now();
        let capture = run_captured(shell, None, &cancelled, OUTPUT_CAP)?;
        let elapsed = started.elapsed();
        assert!(capture.stdout.contains("ready"), "{}", capture.stderr);
        assert!(capture.cancelled);
        assert!(
            elapsed < Duration::from_secs(6),
            "the re-grouped grandchild outlived the kill and held stdout for {elapsed:?}"
        );
        Ok(())
    }
}
