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
        // The wait is for the rail's cursor row, which exists only once the
        // session list has landed: `no sessions yet` is a Full-sidebar string
        // the default rail never draws, so a wait on its absence passed at the
        // first frame and `enter` resumed nothing (the 23 s CI failure).
        run_console(
            &dir,
            &socket,
            &root,
            "wait-frame 8000 !connecting…\n\
             wait-frame 8000 ▸\n\
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

fn frames_of(dir: &Path) -> Result<String, Box<dyn Error>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut all = String::new();
    for entry in entries {
        all.push_str(&std::fs::read_to_string(entry.path())?);
    }
    Ok(all)
}

/// One script, two hosts: the console pane and solo render the same prompt, reply and
/// popup, because they run the same chat.
#[test]
fn the_console_pane_and_solo_render_the_same_chat() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-parity-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let root = dir.join("repo");
    std::fs::create_dir_all(&root)?;
    let body = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/proof/parity.drive"
    ))?;
    let needles = ["┃   hello parity", "faux:", "plantree", "faux-1"];

    // Solo: the script as it stands.
    let solo_keys = dir.join("solo.keys");
    std::fs::write(&solo_keys, &body)?;
    let solo_frames = dir.join("solo-frames");
    #[expect(
        clippy::disallowed_methods,
        reason = "true E2E: the shipped binary in headless drive mode"
    )]
    let status = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("solo-sessions").display().to_string(),
            "--keys",
            &solo_keys.display().to_string(),
            "--frames",
            &solo_frames.display().to_string(),
        ])
        .env("HOME", &dir)
        .current_dir(&root)
        .status()?;
    if !status.success() {
        return Err(format!("solo exited {status}").into());
    }
    let solo = frames_of(&solo_frames)?;

    // The console: the same script once a session is open in the pane.
    let (mut daemon, socket) = spawn_daemon(&dir)?;
    let console_frames = dir.join("console-frames");
    let outcome = (|| -> TestResult {
        let keys = dir.join("console.keys");
        std::fs::write(
            &keys,
            format!("wait-frame 8000 !connecting…\nkey alt-n\n{body}"),
        )?;
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
                "--frames",
                &console_frames.display().to_string(),
            ])
            .env("HOME", &dir)
            .status()?;
        if !status.success() {
            return Err(format!("console exited {status}").into());
        }
        Ok(())
    })();
    let _ = daemon.kill();
    let _ = daemon.wait();
    outcome?;
    let console = frames_of(&console_frames)?;

    for needle in needles {
        assert!(solo.contains(needle), "solo frames lack {needle:?}");
        assert!(console.contains(needle), "console frames lack {needle:?}");
    }
    Ok(())
}
