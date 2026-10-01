use std::error::Error;
use std::process::Command;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn headless_drive_renders_a_turn_and_dumps_frames() -> TestResult {
    let dir = Scratch::new("yi-tui-drive")?;
    let home = dir.home()?;
    let keys = dir.join("script.keys");
    std::fs::write(
        &keys,
        // No sleep between Enter and the second `wait-idle`: 0.43.2 made
        // `wait-idle` count submissions against `AgentStart`, so the gap the
        // sleep covered is closed by the barrier itself.
        "wait-idle 10000\ntype follow-up\nkey enter\nwait-idle 10000\nquit\n",
    )?;
    let frames = dir.join("frames");

    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "ping",
        ])
        // Incident: the drive gate read the developer's own ~/.yi/config.json,
        // so a `keys` entry there decided whether it passed.
        .env("HOME", &home)
        .output()?;
    assert!(
        output.status.success(),
        "drive run must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut all_frames = String::new();
    let mut entries: Vec<_> = std::fs::read_dir(&frames)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    assert!(!entries.is_empty(), "at least one frame must be dumped");
    for entry in entries {
        all_frames.push_str(&std::fs::read_to_string(entry.path())?);
    }
    for needle in ["┃   ping", "faux: ping", "┃   follow-up", "╭", "faux-1"] {
        assert!(
            all_frames.contains(needle),
            "frames must show the rendered UI ({needle} missing)"
        );
    }
    Ok(())
}

/// A rewind has to be visible: the undone exchange leaves the transcript and
/// the message it undid comes back to the composer unsent.
#[test]
fn rewinding_removes_the_exchange_and_restores_the_message() -> TestResult {
    let dir = Scratch::new("yi-tui-rewind")?;
    let home = dir.home()?;
    let keys = dir.join("script.keys");
    std::fs::write(
        &keys,
        // The two remaining barriers are rendered facts — the tree overlay is
        // open, the rewound text is back in the composer. As fixed sleeps they
        // rewound the wrong exchange whenever the runner was loaded.
        "wait-idle 10000\ntype second question\nkey enter\nwait-idle 10000\n\
         key esc\nkey esc\nwait-frame 10000 Session Tree\n\
         key up\nkey enter\nwait-frame 10000 \u{2502}second question\nquit\n",
    )?;
    let frames = dir.join("frames");

    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--cwd",
            &dir.display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "first question",
        ])
        .env("HOME", &home)
        .output()?;
    assert!(
        output.status.success(),
        "drive run must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut entries: Vec<_> = std::fs::read_dir(&frames)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let last = entries.last().ok_or("no frames dumped")?;
    let final_frame = std::fs::read_to_string(last.path())?;

    assert!(
        final_frame.contains("first question"),
        "the kept turn stays: {final_frame}"
    );
    assert!(
        !final_frame.contains("┃   second question"),
        "the rewound user turn leaves the transcript: {final_frame}"
    );
    assert!(
        final_frame.contains("│second question"),
        "the rewound message returns to the composer: {final_frame}"
    );
    Ok(())
}

#[test]
fn slash_new_swaps_the_session_and_clears_the_transcript() -> TestResult {
    let dir = Scratch::new("yi-tui-new")?;
    let home = dir.home()?;
    let keys = dir.join("script.keys");
    std::fs::write(
        &keys,
        // `!` waits for the text to leave: /new is done exactly when the first
        // turn is off the screen, which is also what the assertion checks.
        "wait-idle 10000\nkey /\ntype new\nkey enter\n\
         wait-frame 10000 !faux: ping\nquit\n",
    )?;
    let frames = dir.join("frames");
    let sessions = dir.join("sessions");

    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &sessions.display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "ping",
        ])
        .env("HOME", &home)
        .output()?;
    assert!(
        output.status.success(),
        "drive run must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut entries: Vec<_> = std::fs::read_dir(&frames)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let rendered: Vec<String> = entries
        .iter()
        .map(|entry| std::fs::read_to_string(entry.path()))
        .collect::<Result<_, _>>()?;
    assert!(
        rendered.iter().any(|frame| frame.contains("faux: ping")),
        "the first turn must reach the screen before /new"
    );
    let last = rendered
        .last()
        .ok_or("the drive dumps at least one frame")?;
    assert!(
        !last.contains("faux: ping") && !last.contains("┃   ping"),
        "/new leaves the transcript empty: {last}"
    );

    let mut files = 0;
    for project in std::fs::read_dir(&sessions)?.filter_map(Result::ok) {
        for entry in std::fs::read_dir(project.path())?.filter_map(Result::ok) {
            if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                files += 1;
            }
        }
    }
    assert_eq!(
        files, 2,
        "/new writes a second session file beside the first"
    );
    Ok(())
}

/// Plan, undo, permissions, compact and sessions all answer for this session.
#[test]
fn plan_and_undo_answer_for_the_session_the_turn_ran_in() -> TestResult {
    let dir = Scratch::new("yi-tui-surfaces")?;
    let home = dir.home()?;
    let work = dir.join("work");
    std::fs::create_dir_all(&work)?;
    let keys = dir.join("script.keys");
    std::fs::write(
        &keys,
        "wait-idle 10000\nkey /\ntype plan\nkey enter\nwait-frame 10000 /plan: no plan\n\
         key /\ntype undo\nkey enter\nwait-frame 10000 /undo: nothing to restore\n\
         key /\ntype permissions\nkey enter\nwait-frame 10000 permission mode:\nkey /\n\
         type permissions yolo\nkey enter\nwait-frame 10000 permission mode: yolo\nkey /\n\
         type compact\nkey enter\nwait-frame 10000 compaction scheduled\nkey /\n\
         type sessions\nkey enter\nwait-frame 10000 ago\nquit\n",
    )?;
    let frames = dir.join("frames");

    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--cwd",
            &work.display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "ping",
        ])
        .env("HOME", &home)
        .output()?;
    assert!(
        output.status.success(),
        "drive run must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut entries: Vec<_> = std::fs::read_dir(&frames)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let last = entries.last().ok_or("no frames dumped")?;
    let final_frame = std::fs::read_to_string(last.path())?;
    for needle in [
        "/plan: no plan is open",
        "/undo: nothing to restore",
        "permission mode: yolo",
        "compaction scheduled",
    ] {
        assert!(
            final_frame.contains(needle),
            "{needle} must reach this session: {final_frame}"
        );
    }
    // The age label counts wall-clock seconds, so a loaded run reads "1s ago" or more: the
    // /sessions row is matched by this session's id, read from its file, not by its age.
    let mut ids = Vec::new();
    for project in std::fs::read_dir(dir.join("sessions"))?.filter_map(Result::ok) {
        for entry in std::fs::read_dir(project.path())?.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some((_, id)) = name.strip_suffix(".jsonl").and_then(|n| n.split_once('_')) {
                ids.push(id.to_owned());
            }
        }
    }
    let [id] = ids.as_slice() else {
        return Err(format!("one session file expected: {ids:?}").into());
    };
    assert!(
        final_frame.lines().any(|line| line
            .split_once(id.as_str())
            .is_some_and(|(_, age)| age.contains(" ago"))),
        "/sessions must list {id} with its age: {final_frame}"
    );
    Ok(())
}

/// Ctrl+R searches submitted prompts; Enter accepts a match as a draft and
/// does not start another turn.
#[test]
fn reverse_search_enter_accepts_a_match_without_submitting() -> TestResult {
    let dir = Scratch::new("yi-tui-isearch")?;
    let home = dir.home()?;
    let keys = dir.join("script.keys");
    std::fs::write(
        &keys,
        "wait-idle 10000\ntype findme\nkey enter\nwait-idle 10000\n\
         type draft\nkey ctrl-r\nwait-frame 10000 reverse-i-search\n\
         type findme\nwait-frame 10000 reverse-i-search: findme\nkey enter\n\
         wait-frame 10000 !reverse-i-search\nquit\n",
    )?;
    let frames = dir.join("frames");

    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "ping",
        ])
        .env("HOME", &home)
        .output()?;
    assert!(
        output.status.success(),
        "drive run must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut entries: Vec<_> = std::fs::read_dir(&frames)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut all_frames = String::new();
    for entry in &entries {
        all_frames.push_str(&std::fs::read_to_string(entry.path())?);
    }
    assert!(
        all_frames.contains("reverse-i-search: findme"),
        "frames must show the query on a hit: {all_frames}"
    );
    let last = entries.last().ok_or("no frames dumped")?;
    let final_frame = std::fs::read_to_string(last.path())?;
    assert!(
        !final_frame.contains("reverse-i-search"),
        "Enter must leave search: {final_frame}"
    );
    assert_eq!(
        final_frame.matches("┃   findme").count(),
        1,
        "Enter accepts the preview; it must not submit a third turn: {final_frame}"
    );
    assert!(
        final_frame.contains("│findme"),
        "the accepted match stays in the composer: {final_frame}"
    );
    Ok(())
}

/// `--deadline` seconds arrive straight off the command line, and
/// `Instant + Duration` panics rather than saturating: an unclamped value
/// aborted the process at 101 before the loop ran a single step.
#[test]
fn an_absurd_deadline_is_clamped_rather_than_panicking() -> TestResult {
    let dir = Scratch::new("yi-tui-deadline")?;
    let home = dir.home()?;
    let keys = dir.join("keys");
    std::fs::write(&keys, "quit\n")?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--deadline",
            &u64::MAX.to_string(),
        ])
        .env("HOME", &home)
        .output()?;
    assert_ne!(
        output.status.code(),
        Some(101),
        "a panic, not a clamp: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Dies with every headless frame 80 columns wide: the size was fixed, so no proof could show
/// how the status row lays out on a wide screen.
#[test]
fn a_headless_drive_renders_at_the_size_it_is_given() -> TestResult {
    let dir = Scratch::new("yi-tui-size")?;
    let home = dir.home()?;
    let keys = dir.join("keys");
    std::fs::write(&keys, "wait-frame 5000 faux-1\nquit\n")?;
    let frames = dir.join("frames");
    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "--size",
            "254x30",
        ])
        .env("HOME", &home)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut entries: Vec<_> = std::fs::read_dir(&frames)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let last = std::fs::read_to_string(entries.last().ok_or("no frames dumped")?.path())?;
    let widest = last
        .lines()
        .map(|line| line.trim_matches('"').chars().count())
        .max();
    assert_eq!(widest, Some(254), "{last}");
    assert_eq!(last.lines().count(), 30, "{last}");
    for refused in ["1x1", "1001x30", "254x501"] {
        #[expect(
            clippy::disallowed_methods,
            reason = "the flag is parsed by the spawned binary; tests must run the real process"
        )]
        let output = Command::new(env!("CARGO_BIN_EXE_yi"))
            .args([
                "tui",
                "--headless",
                "--model",
                "faux/faux-1",
                "--size",
                refused,
            ])
            .env("HOME", &home)
            .output()?;
        assert_eq!(
            output.status.code(),
            Some(2),
            "{refused} is refused, not clamped"
        );
    }
    Ok(())
}

/// One path for `--record` and `--snap` left the still truncating the
/// recording still open on it, and the run exited 0 having lost it.
#[test]
fn one_path_for_both_capture_sinks_is_refused() -> TestResult {
    let dir = Scratch::new("yi-tui-samepath")?;
    let home = dir.home()?;
    let keys = dir.join("keys");
    std::fs::write(&keys, "quit\n")?;
    let both = dir.join("both.cast");
    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--record",
            &both.display().to_string(),
            "--snap",
            &both.display().to_string(),
        ])
        .env("HOME", &home)
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("different files"), "{stderr}");
    Ok(())
}

/// Dies with the cassette dropped on the console road: `yi console --faux` reused a listening
/// daemon that never saw the cassette, so the scripted family met a real model. A headless
/// console is a client of the same daemon, and `yi serve` never hands the cassette to its
/// workers (#943).
#[test]
fn a_cassette_is_refused_where_it_cannot_reach_the_model() -> TestResult {
    let dir = Scratch::new("yi-faux-console")?;
    let cassette = dir.join("cassette.jsonl");
    std::fs::write(&cassette, "")?;
    let cassette = cassette.display().to_string();
    let socket = dir.join("daemon.sock").display().to_string();
    for road in [
        vec!["console", "--faux", &cassette],
        vec!["console", "--headless", "--faux", &cassette],
        vec!["serve", "--socket", &socket, "--faux", &cassette],
    ] {
        #[expect(
            clippy::disallowed_methods,
            reason = "the flag contract is the spawned binary's argument parser"
        )]
        let mut child = Command::new(env!("CARGO_BIN_EXE_yi"))
            .args(&road)
            .env("HOME", dir.home()?)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        // An accepted road starts a daemon that never exits; the refusal is immediate.
        let started = std::time::Instant::now();
        while child.try_wait()?.is_none() {
            if started.elapsed() > std::time::Duration::from_secs(5) {
                child.kill()?;
                child.wait()?;
                return Err(format!("{road:?} was accepted: still running after 5 s").into());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let output = child.wait_with_output()?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(output.status.code(), Some(2), "{road:?}: {stderr}");
        assert!(
            stderr.contains("--faux runs in-process only"),
            "{road:?}: {stderr}"
        );
    }
    Ok(())
}

/// Incident (#943): `--faux` with `--model openrouter/…` swapped nothing in, and the drive sent
/// a real request to OpenRouter; only the placeholder key's 401 kept it unbilled. Every egress
/// rides the proxy here, so any connection the listener sees left the process.
#[test]
fn a_cassette_run_with_a_routed_model_stays_offline() -> TestResult {
    let dir = Scratch::new("yi-faux-offline")?;
    // No `dir.home()`: its prewarm-off config would hide the kernel's venv install.
    let home = dir.join("home");
    std::fs::create_dir_all(&home)?;
    let (address, seen) = recorder()?;
    let cassette = dir.join("cassette.jsonl");
    std::fs::write(&cassette, cassette_lines(&[], "scripted and offline"))?;
    let keys = dir.join("script.keys");
    std::fs::write(&keys, "wait-idle 20000\nquit\n")?;
    let frames = dir.join("frames");
    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
    command
        .args([
            "tui",
            "--headless",
            "--model",
            "openrouter/anthropic/claude-opus-5",
            "--faux",
            &cassette.display().to_string(),
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "--deadline",
            "30",
            "ping",
        ])
        .env("HOME", &home);
    let output = offline(&mut command, &address).output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "drive run must exit 0: {stderr}");
    // A refresh thread may still be dialing after the drive quits.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let seen = seen.lock().map_err(|_| "proxy log")?.clone();
    assert!(seen.is_empty(), "--faux left the process: {seen:?}");
    let mut all_frames = String::new();
    for entry in std::fs::read_dir(&frames)?.filter_map(Result::ok) {
        all_frames.push_str(&std::fs::read_to_string(entry.path())?);
    }
    for needle in ["scripted and offline", "claude-opus-5"] {
        assert!(
            all_frames.contains(needle),
            "{needle} missing from the frames"
        );
    }
    Ok(())
}

/// A loopback listener every connection lands on; it logs each one's first line.
fn recorder() -> std::io::Result<(String, std::sync::Arc<std::sync::Mutex<Vec<String>>>)> {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = format!("http://{}", listener.local_addr()?);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&seen);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
            let mut head = [0u8; 256];
            let read = stream.read(&mut head).unwrap_or(0);
            let line = String::from_utf8_lossy(head.get(..read).unwrap_or_default());
            if let Ok(mut log) = log.lock() {
                log.push(line.lines().next().unwrap_or_default().to_owned());
            }
        }
    });
    Ok((address, seen))
}

/// Every proxy variable at the recorder, and a placeholder key so a leak gets as far as a request.
fn offline<'a>(command: &'a mut Command, address: &str) -> &'a mut Command {
    command
        .env("OPENROUTER_API_KEY", "sk-or-placeholder")
        .env("HTTPS_PROXY", address)
        .env("HTTP_PROXY", address)
        .env("ALL_PROXY", address)
        .env_remove("https_proxy")
        .env_remove("http_proxy")
        .env_remove("all_proxy")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
}

/// The review of #960: a `--faux` run outside a drive claimed a lane, and the claim fetched
/// `origin` (over ssh, past any proxy); and a classifier sidecar was a second road out. The
/// origin and the sidecar are the recorder itself, so both would land on it.
#[test]
fn a_cassette_run_in_a_repo_with_an_origin_and_a_classifier_stays_offline() -> TestResult {
    let dir = Scratch::new("yi-faux-origin")?;
    let (address, seen) = recorder()?;
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".yi/skills/land"))?;
    std::fs::write(
        home.join(".yi/config.json"),
        serde_json::json!({
            "kernel": {"prewarm": false},
            "models": {"classifier": "english"},
            "classifier": {"url": address},
        })
        .to_string(),
    )?;
    std::fs::write(
        home.join(".yi/skills/land/SKILL.md"),
        "---\nname: land\ndescription: land a branch\ntrigger: land\n---\nLand it.\n",
    )?;
    let project = dir.join("project");
    std::fs::create_dir_all(&project)?;
    std::fs::write(project.join("README.md"), "trunk\n")?;
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "README.md"],
        &[
            "-c",
            "user.email=yi@example.com",
            "-c",
            "user.name=yi",
            "commit",
            "-q",
            "-m",
            "init",
        ],
        &["remote", "add", "origin", &format!("{address}/r.git")],
    ] {
        #[expect(
            clippy::disallowed_methods,
            reason = "the fixture repository is built with the real git the lane claim runs"
        )]
        let status = Command::new("git")
            .args(args)
            .current_dir(&project)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()?;
        assert!(status.success(), "git {args:?}");
    }
    let cassette = dir.join("cassette.jsonl");
    std::fs::write(&cassette, cassette_lines(&[], "scripted and offline"))?;
    let cassette = cassette.display().to_string();
    let sessions = dir.join("sessions").display().to_string();
    for lanes in [&[][..], &["--lanes"]] {
        #[expect(
            clippy::disallowed_methods,
            reason = "the claim and the classifier run in the spawned binary; tests must run the real process"
        )]
        let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
        command
            .arg("ask")
            .args(lanes)
            .args([
                "--model",
                "openrouter/anthropic/claude-opus-5",
                "--faux",
                &cassette,
                "--session-dir",
                &sessions,
                "land the branch",
            ])
            .current_dir(&project)
            .env("HOME", &home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        let output = offline(&mut command, &address).output()?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "{lanes:?}: {stderr}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("scripted and offline"),
            "{lanes:?}: {stderr}"
        );
        // The lane's background fetch and the classifier's consult run beside the answer.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let seen = seen.lock().map_err(|_| "recorder log")?.clone();
        assert!(
            seen.is_empty(),
            "{lanes:?}: --faux left the process: {seen:?}"
        );
        // Without `--lanes` the run works in place, as a drive does, and claims no lane.
        assert_eq!(
            home.join(".yi/lanes").exists(),
            !lanes.is_empty(),
            "{lanes:?}: lane claimed"
        );
    }
    Ok(())
}

/// A paced `type` step holds the outer loop for its whole duration, and the
/// wall clock is read there: without a check inside the step, a long paced
/// line ran to completion past the deadline meant to bound it. Counting the
/// characters that landed says so without depending on wall time, which here
/// is dominated by the one-time kernel setup.
#[test]
fn a_paced_type_step_still_honours_the_deadline() -> TestResult {
    let dir = Scratch::new("yi-tui-paced")?;
    let home = dir.home()?;
    let keys = dir.join("keys");
    let frames = dir.join("frames");
    // 40 characters at 100 ms is 4 s of typing against a 1 s deadline, so at
    // most a quarter of them may reach the screen.
    let typed = "x".repeat(40);
    std::fs::write(&keys, format!("type-ms 100\ntype {typed}\nquit\n"))?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "--deadline",
            "1",
        ])
        .env("HOME", &home)
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut dumps: Vec<_> = std::fs::read_dir(&frames)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    dumps.sort();
    let last = std::fs::read_to_string(dumps.last().ok_or("no frames dumped")?)?;
    let landed = last.matches('x').count();

    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("timed out"), "{stderr}");
    assert!(
        landed < 30,
        "the step typed {landed} of 40 characters, so it ran past its deadline"
    );
    Ok(())
}

/// One assistant message per bash command, then a closing text reply, as `--faux` reads them.
fn cassette_lines(calls: &[(&str, serde_json::Value)], reply: &str) -> String {
    let message = |content: serde_json::Value, stop: &str| {
        serde_json::json!({
            "role": "assistant", "content": content,
            "api": "faux", "provider": "faux", "model": "faux-1",
            "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                      "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}},
            "stopReason": stop, "timestamp": 0
        })
        .to_string()
    };
    let mut lines: Vec<String> = calls
        .iter()
        .enumerate()
        .map(|(index, (tool, arguments))| {
            message(
                serde_json::json!([{"type": "toolCall", "id": format!("call-{index}"), "name": tool, "arguments": arguments}]),
                "toolUse",
            )
        })
        .collect();
    lines.push(message(
        serde_json::json!([{"type": "text", "text": reply}]),
        "stop",
    ));
    lines.join("\n") + "\n"
}

#[cfg(target_os = "macos")]
fn bash(command: &str) -> (&'static str, serde_json::Value) {
    ("bash", serde_json::json!({ "command": command }))
}

#[cfg(target_os = "macos")]
/// A scratch dir under cargo's target tmp, removed on drop like [`Scratch`].
struct TargetScratch(std::path::PathBuf);

#[cfg(target_os = "macos")]
impl TargetScratch {
    /// A checkout under tmp puts cargo's target there too, inside a root the sandbox writes.
    fn outside_tmp(&self) -> bool {
        let resolved = self.0.canonicalize().unwrap_or_else(|_| self.0.clone());
        ["/private/tmp", "/private/var/folders", "/tmp"]
            .iter()
            .all(|root| !resolved.starts_with(root))
    }

    fn new(name: &str) -> std::io::Result<Self> {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

#[cfg(target_os = "macos")]
impl Drop for TargetScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "macos")]
fn git_in(dir: &std::path::Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the fixture repository is built with the real git the session will run"
    )]
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    if !output.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The incident (D205): a lane's index lives in the trunk's git dir, outside the tree, so a
/// contained `git add` failed on `index.lock`. The real binary runs the commit contained here.
#[cfg(target_os = "macos")]
#[test]
fn a_lane_session_commits_contained() -> TestResult {
    let dir = Scratch::new("yi-tui-lane-commit")?;
    let home = dir.home()?;
    // Outside tmp: every tmp root is writable, which would hide the trunk's git dir.
    let trunk = TargetScratch::new("yi-tui-lane-commit")?;
    if !trunk.outside_tmp() {
        return Ok(());
    }
    let project = trunk.0.join("project");
    std::fs::create_dir_all(&project)?;
    git_in(&project, &["init", "-q", "-b", "main"])?;
    std::fs::write(project.join("README.md"), "trunk\n")?;
    git_in(&project, &["add", "README.md"])?;
    git_in(
        &project,
        &[
            "-c",
            "user.email=yi@example.com",
            "-c",
            "user.name=yi",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    )?;
    let command = "touch lane.txt && git add -A && git -c user.email=yi@example.com -c user.name=yi commit -q -m lane";
    let cassette = dir.join("cassette.jsonl");
    std::fs::write(&cassette, cassette_lines(&[bash(command)], "committed"))?;
    let keys = dir.join("script.keys");
    std::fs::write(&keys, "wait-idle 30000\nquit\n")?;
    let frames = dir.join("frames");

    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--lanes",
            "--model",
            "faux/faux-1",
            "--faux",
            &cassette.display().to_string(),
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "commit it",
        ])
        .current_dir(&project)
        .env("HOME", &home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    assert!(
        output.status.success(),
        "drive run must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut all_frames = String::new();
    for entry in std::fs::read_dir(&frames)?.filter_map(Result::ok) {
        all_frames.push_str(&std::fs::read_to_string(entry.path())?);
    }
    assert!(
        !all_frames.contains("Operation not permitted") && all_frames.contains("committed"),
        "the contained commit must run to the final reply: {all_frames}"
    );
    let subjects = git_in(&project, &["log", "--all", "--format=%s"])?;
    assert!(
        subjects.lines().any(|subject| subject == "lane"),
        "the lane's commit must reach the shared object store: {subjects}"
    );
    Ok(())
}

/// The incident's second half (D206): the retry reshaped the refused command, so an exact-text
/// memory contained it again and no question ever reached the user.
#[cfg(target_os = "macos")]
#[test]
fn a_reshaped_retry_of_a_refused_program_reaches_the_approval_view() -> TestResult {
    let dir = Scratch::new("yi-tui-refused-retry")?;
    let home = dir.home()?;
    let outside = TargetScratch::new("yi-tui-refused-retry")?;
    if !outside.outside_tmp() {
        return Ok(());
    }
    let first = format!("mkdir {}", outside.0.join("a").display());
    let second = format!("mkdir {} && ls", outside.0.join("b").display());
    let cassette = dir.join("cassette.jsonl");
    std::fs::write(
        &cassette,
        cassette_lines(&[bash(&first), bash(&second)], "gave up"),
    )?;
    let keys = dir.join("script.keys");
    std::fs::write(
        &keys,
        "wait-frame 30000 Reject\nkey esc\nwait-idle 30000\nquit\n",
    )?;
    let frames = dir.join("frames");

    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--model",
            "faux/faux-1",
            "--faux",
            &cassette.display().to_string(),
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "make the dirs",
        ])
        .current_dir(&*dir)
        .env("HOME", &home)
        .output()?;
    let mut all_frames = String::new();
    for entry in std::fs::read_dir(&frames)?.filter_map(Result::ok) {
        all_frames.push_str(&std::fs::read_to_string(entry.path())?);
    }
    assert!(
        output.status.success(),
        "drive run must exit 0: {}\n{all_frames}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        all_frames.contains("requires permission"),
        "the reshaped retry must ask: {all_frames}"
    );
    assert!(
        !outside.0.join("a").exists() && !outside.0.join("b").exists(),
        "neither directory may exist: the first was contained, the second rejected"
    );
    Ok(())
}

/// D207: "Always allow" kept a rule keyed on the whole call, patch body and all, so the next
/// edit in the same directory asked again.
#[test]
fn an_always_allowed_directory_edits_without_a_second_prompt() -> TestResult {
    let dir = Scratch::new("yi-tui-grant")?;
    let home = dir.home()?;
    let write = |path: &str, text: &str| {
        (
            "write",
            serde_json::json!({ "path": path, "content": text }),
        )
    };
    let cassette = dir.join("cassette.jsonl");
    std::fs::write(
        &cassette,
        cassette_lines(
            &[
                write("notes/one.md", "first\n"),
                write("notes/two.md", "second\n"),
            ],
            "both written",
        ),
    )?;
    let keys = dir.join("script.keys");
    // The second write may not ask: if it does, nothing answers it and the drive never idles.
    std::fs::write(
        &keys,
        "wait-frame 30000 Always allow\nkey a\nwait-idle 30000\nquit\n",
    )?;
    let frames = dir.join("frames");

    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--confirm",
            "--model",
            "faux/faux-1",
            "--faux",
            &cassette.display().to_string(),
            "--session-dir",
            &dir.join("sessions").display().to_string(),
            "--keys",
            &keys.display().to_string(),
            "--frames",
            &frames.display().to_string(),
            "write the notes",
        ])
        .current_dir(&*dir)
        .env("HOME", &home)
        .output()?;
    let mut all_frames = String::new();
    for entry in std::fs::read_dir(&frames)?.filter_map(Result::ok) {
        all_frames.push_str(&std::fs::read_to_string(entry.path())?);
    }
    assert!(
        output.status.success(),
        "the second write must not ask: {}\n{all_frames}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        all_frames.contains("Always allow edits under notes"),
        "the prompt names the directory the grant covers:\n{all_frames}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("notes/two.md"))?,
        "second\n"
    );
    Ok(())
}

/// Quit while a kernel cell sleeps: the process has to leave before the cell would end.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn quitting_during_a_running_kernel_cell_exits_promptly() -> TestResult {
    let dir = Scratch::new("yi-tui-quit-cell")?;
    let home = dir.home()?;
    let calls = [
        ("ipython", serde_json::json!({ "code": "print('booted')" })),
        (
            "ipython",
            serde_json::json!({ "code": "import time\ntime.sleep(900)" }),
        ),
    ];
    let cassette = dir.join("cassette.jsonl");
    std::fs::write(&cassette, cassette_lines(&calls, "done"))?;
    let keys = dir.join("script.keys");
    std::fs::write(&keys, "wait-frame 600000 time.sleep(900)\nquit\n")?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the drive contract is the spawned binary's headless mode; tests must run the real process"
    )]
    let mut child = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "tui",
            "--headless",
            "--deadline",
            "600",
            "--model",
            "faux/faux-1",
        ])
        .args(["--faux", &cassette.display().to_string()])
        .args(["--session-dir", &dir.join("sessions").display().to_string()])
        .args(["--keys", &keys.display().to_string(), "go"])
        .env("HOME", &home)
        // One venv per target dir, reused by every run rather than rebuilt under each HOME.
        .env(
            "YI_KERNEL_VENV",
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("drive-kernel-venv"),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert!(
        status.is_some_and(|status| status.success()),
        "the drive was still running a 900 s cell 300 s in, after its quit: {status:?}"
    );
    Ok(())
}
