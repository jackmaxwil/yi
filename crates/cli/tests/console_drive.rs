//! True E2E for `yi console`: the real binary in headless drive mode against
//! a real `yi serve` daemon on the faux model. First run creates a session
//! and prompts it; second run reattaches from nothing and replays.

use std::error::Error;
use std::path::Path;
use std::process::{Command, Stdio};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

#[path = "support/daemon.rs"]
mod daemon;
use daemon::spawn_daemon;

type TestResult = Result<(), Box<dyn Error>>;

fn run_console(dir: &Path, socket: &Path, root: &Path, script: &str, extra: &[&str]) -> TestResult {
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
        .args(extra)
        .env("HOME", dir.join("home"))
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
    let dir = Scratch::new("yi-console-e2e")?;
    let root = dir.join("repo");
    std::fs::create_dir_all(&root)?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;

    let outcome = (|| -> TestResult {
        // Run 1: attach, create a session, prompt it, watch faux stream.
        run_console(
            &dir,
            &socket,
            &root,
            "wait-frame 8000 no sessions for this root\n\
             key alt-n\n\
             wait-frame 8000 untitled\n\
             type hello daemon\n\
             key enter\n\
             wait-frame 15000 faux:\n\
             wait-idle 15000\n\
             quit\n",
            &[],
        )?;
        // Run 2: a fresh console reattaches and replays the stored branch.
        // The wait is for the rail's first slot, which exists only once the
        // session list has landed: `no sessions yet` is a Full-sidebar string
        // the default rail never draws, so a wait on its absence passed at the
        // first frame and `enter` resumed nothing (the 23 s CI failure).
        run_console(
            &dir,
            &socket,
            &root,
            "wait-frame 8000 !connecting…\n\
             wait-frame 8000 1 HE\n\
             key enter\n\
             wait-frame 15000 faux:\n\
             wait-frame 8000 hello daemon\n\
             quit\n",
            &[],
        )?;
        Ok(())
    })();

    let _ = daemon.kill();
    let _ = daemon.wait();
    outcome
}

/// The reply rows a frame dump shows, as the chat pane draws them after its `•`.
fn reply_rows(frames: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    Ok(frames_of(frames)?
        .lines()
        .filter_map(|line| line.split_once("│ • "))
        .map(|(_, row)| row.trim_end_matches('"').trim().to_owned())
        .collect())
}

/// Dies with `yi console` painting each network burst whole again: `tui.pace` dropped on its way
/// through `ConsoleOptions` to a chat, which no other test sees as the faux reply is one short row.
#[test]
fn a_recorded_console_reveals_a_reply_over_frames_and_a_headless_one_paints_it_whole() -> TestResult
{
    // Incident: a loaded console took the reply and its end in one drain, and drain mode showed
    // "faux:" whole in one 50 ms step; two paused `type` steps replay the batch, then a slow frame.
    let script = "wait-frame 8000 no sessions for this root\n\
                  key alt-n\n\
                  wait-frame 8000 untitled\n\
                  type hello daemon\n\
                  key enter\n\
                  type-ms 1500\n\
                  type x\n\
                  type y\n\
                  wait-frame 15000 faux:\n\
                  wait-idle 15000\n\
                  quit\n";
    let mut rows = Vec::new();
    // Invariant: a daemon per run, so the rail starts empty and `untitled` can only be this run's.
    for (name, record) in [("plain", false), ("recorded", true)] {
        let dir = Scratch::new(&format!("yi-console-pace-{name}"))?;
        // At 10% pace each character outlasts a 50 ms step even in drain mode, so a frame shows part.
        let config = r#"{"kernel":{"prewarm":false},"tui":{"pace":10}}"#;
        std::fs::write(dir.home()?.join(".yi/config.json"), config)?;
        let root = dir.join("repo");
        std::fs::create_dir_all(&root)?;
        let (mut daemon, socket) = spawn_daemon(&dir)?;
        let (frames, cast) = (dir.join("frames"), dir.join("run.cast"));
        let mut extra = vec!["--frames".to_owned(), frames.display().to_string()];
        if record {
            extra.extend(["--record".to_owned(), cast.display().to_string()]);
        }
        let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
        let outcome = run_console(&dir, &socket, &root, script, &extra);
        let _ = daemon.kill();
        let _ = daemon.wait();
        outcome?;
        rows.push(reply_rows(&frames)?);
    }
    let partial = |rows: &[String]| {
        rows.iter()
            .any(|row| !row.is_empty() && row != "faux:" && "faux:".starts_with(row.as_str()))
    };
    let [plain, recorded] = rows.as_slice() else {
        return Err("two runs".into());
    };
    assert!(plain.iter().any(|row| row == "faux:"), "{plain:?}");
    assert!(
        !partial(plain),
        "a headless run paints the reply whole: {plain:?}"
    );
    assert!(
        partial(recorded),
        "a recorded run keeps the configured pace, so a frame shows the reply half-revealed: {recorded:?}"
    );
    Ok(())
}

/// Dies with 80-column frames: the console's headless screen ignored `--size`, so no console
/// proof could show a wide pane.
#[test]
fn a_headless_console_renders_at_the_size_it_is_given() -> TestResult {
    let dir = Scratch::new("yi-console-size")?;
    let root = dir.join("repo");
    std::fs::create_dir_all(&root)?;
    let frames = dir.join("frames");
    let (mut daemon, socket) = spawn_daemon(&dir)?;
    let outcome = run_console(
        &dir,
        &socket,
        &root,
        "wait-frame 8000 !connecting…\nquit\n",
        &[
            "--size",
            "254x40",
            "--frames",
            &frames.display().to_string(),
        ],
    );
    let _ = daemon.kill();
    let _ = daemon.wait();
    outcome?;
    let all = frames_of(&frames)?;
    let widest = all
        .lines()
        .map(|line| line.trim_matches('"').chars().count())
        .max();
    assert_eq!(widest, Some(254), "{all}");
    Ok(())
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
    let dir = Scratch::new("yi-parity-e2e")?;
    let home = dir.home()?;
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
        .env("HOME", &home)
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
            .env("HOME", &home)
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

/// A paced console scene outlives the 60 s an assertion run needs; `--deadline` is its wall.
#[test]
fn a_console_drive_honours_its_deadline() -> TestResult {
    let dir = Scratch::new("yi-console-deadline")?;
    let keys = dir.join("script.keys");
    std::fs::write(&keys, "wait 3000\nquit\n")?;
    #[expect(
        clippy::disallowed_methods,
        reason = "true E2E: the shipped binary's own drive clock"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args(["console", "--headless", "--deadline", "1", "--keys"])
        .arg(&keys)
        .arg("--socket")
        .arg(dir.join("no-daemon.sock"))
        .env("HOME", dir.home()?)
        .stdin(Stdio::null())
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("drive script timed out (1 s wall)"),
        "a 3 s script must stop at the 1 s deadline: {stderr}"
    );
    Ok(())
}
