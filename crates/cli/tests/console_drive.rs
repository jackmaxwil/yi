//! True E2E for `yi console`: the real binary in headless drive mode against
//! a real `yi serve` daemon on the faux model. First run creates a session
//! and prompts it; second run reattaches from nothing and replays.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn Error>>;

fn spawn_daemon(dir: &Path) -> Result<(Child, PathBuf), Box<dyn Error>> {
    let socket = dir.join("yi.sock");
    #[expect(
        clippy::disallowed_methods,
        reason = "the daemon contract is the spawned binary's socket; tests must drive the real process"
    )]
    let child = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "serve",
            "--socket",
            &socket.display().to_string(),
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        if Instant::now() > deadline {
            return Err("daemon socket never appeared".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok((child, socket))
}

fn run_console(dir: &Path, socket: &Path, root: &Path, script: &str) -> TestResult {
    let keys = dir.join("script.keys");
    std::fs::write(&keys, script)?;
    #[expect(
        clippy::disallowed_methods,
        reason = "true E2E: the shipped binary drives the shipped daemon"
    )]
    let status = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "console",
            "--headless",
            "--socket",
            &socket.display().to_string(),
            "--cwd",
            &root.display().to_string(),
            "--keys",
            &keys.display().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(format!("console exited {status}").into());
    }
    Ok(())
}

#[test]
fn console_creates_prompts_detaches_and_replays() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-console-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let root = dir.join("repo");
    std::fs::create_dir_all(&root)?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;

    let outcome = (|| -> TestResult {
        // Run 1: attach, create a session, prompt it, watch faux stream.
        run_console(
            &dir,
            &socket,
            &root,
            "wait-frame 8000 !connecting…\n\
             key alt-n\n\
             wait-frame 8000 ○\n\
             type hello daemon\n\
             key enter\n\
             wait-frame 15000 faux:\n\
             wait-idle 15000\n\
             quit\n",
        )?;
        // Run 2: a fresh console reattaches and replays the stored branch.
        // The wait is for the sidebar's empty placeholder to go, not for the
        // stored-session dot: `·` is also the pane's own "· no session" title,
        // which is on screen from the first draw. On a loaded runner that draw
        // lands before the session list does, so a `·` wait passed instantly
        // and `enter` resumed nothing — the whole run then sat out both later
        // waits (15 s + 8 s = the 23 s this failed in on CI).
        run_console(
            &dir,
            &socket,
            &root,
            "wait-frame 8000 !connecting…\n\
             wait-frame 8000 !no sessions yet\n\
             key enter\n\
             wait-frame 15000 faux:\n\
             wait-frame 8000 hello daemon\n\
             quit\n",
        )?;
        Ok(())
    })();

    let _ = daemon.kill();
    let _ = daemon.wait();
    outcome
}
