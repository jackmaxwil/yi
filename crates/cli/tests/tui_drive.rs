use std::error::Error;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn headless_drive_renders_a_turn_and_dumps_frames() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-drive-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
        .env("HOME", &dir)
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
    for needle in ["› ping", "faux: ping", "› follow-up", "╭", "faux-1"] {
        assert!(
            all_frames.contains(needle),
            "frames must show the rendered UI ({needle} missing)"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// A rewind has to be visible: the undone exchange leaves the transcript and
/// the message it undid comes back to the composer unsent.
#[test]
fn rewinding_removes_the_exchange_and_restores_the_message() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-rewind-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
        .env("HOME", &dir)
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
        !final_frame.contains("› second question"),
        "the rewound user turn leaves the transcript: {final_frame}"
    );
    assert!(
        final_frame.contains("│second question"),
        "the rewound message returns to the composer: {final_frame}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn slash_new_swaps_the_session_and_clears_the_transcript() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
        .env("HOME", &dir)
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
        !last.contains("faux: ping") && !last.contains("› ping"),
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
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Both surfaces answer for *this* session: a plan service that never attached
/// and a turn that wrote no checkpoint each say something else, and the
/// difference is the whole contract a reader depends on after a turn.
#[test]
fn plan_and_undo_answer_for_the_session_the_turn_ran_in() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-surfaces-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let work = dir.join("work");
    std::fs::create_dir_all(&work)?;
    // Incident: HOME is a fresh directory, so the default kernel prewarm builds
    // a venv on every run and its "setting up python kernel (one-time, ~30s)…"
    // frame outlasted the 10 s waits below. Neither surface here uses a kernel.
    std::fs::create_dir_all(dir.join(".yi"))?;
    std::fs::write(
        dir.join(".yi/config.json"),
        r#"{"kernel":{"prewarm":false}}"#,
    )?;
    let keys = dir.join("script.keys");
    std::fs::write(
        &keys,
        "wait-idle 10000\nkey /\ntype plan\nkey enter\nwait-frame 10000 /plan: no plan\n\
         key /\ntype undo\nkey enter\nwait-frame 10000 /undo: nothing to restore\nquit\n",
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
        .env("HOME", &dir)
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
        final_frame.contains("/plan: no plan in this session"),
        "/plan must reach the session's own plan service: {final_frame}"
    );
    assert!(
        final_frame.contains("/undo: nothing to restore"),
        "/undo must restore from the checkpoint this turn wrote: {final_frame}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
