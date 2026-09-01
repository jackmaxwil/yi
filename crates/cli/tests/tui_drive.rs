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

/// Plan, undo, permissions, compact and sessions all answer for this session.
#[test]
fn plan_and_undo_answer_for_the_session_the_turn_ran_in() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-surfaces-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let work = dir.join("work");
    std::fs::create_dir_all(&work)?;
    // Incident: an empty HOME prewarms a venv whose "one-time, ~30s" frame outlasted
    // the waits below. None of these verbs need a kernel.
    std::fs::create_dir_all(dir.join(".yi"))?;
    std::fs::write(
        dir.join(".yi/config.json"),
        r#"{"kernel":{"prewarm":false}}"#,
    )?;
    let keys = dir.join("script.keys");
    std::fs::write(
        &keys,
        "wait-idle 10000\nkey /\ntype plan\nkey enter\nwait-frame 10000 /plan: no plan\n\
         key /\ntype undo\nkey enter\nwait-frame 10000 /undo: nothing to restore\n\
         key /\ntype permissions\nkey enter\nwait-frame 10000 permission mode:\nkey /\n\
         type permissions yolo\nkey enter\nwait-frame 10000 permission mode: yolo\nkey /\n\
         type compact\nkey enter\nwait-frame 10000 compaction scheduled\nkey /\n\
         type sessions\nkey enter\nwait-frame 10000 0s ago\nquit\n",
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
    for needle in [
        "/plan: no plan in this session",
        "/undo: nothing to restore",
        "permission mode: yolo",
        "compaction scheduled",
        "0s ago",
    ] {
        assert!(
            final_frame.contains(needle),
            "{needle} must reach this session: {final_frame}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Ctrl+R searches submitted prompts; Enter accepts a match as a draft and
/// does not start another turn.
#[test]
fn reverse_search_enter_accepts_a_match_without_submitting() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-isearch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(dir.join(".yi"))?;
    std::fs::write(
        dir.join(".yi/config.json"),
        r#"{"kernel":{"prewarm":false}}"#,
    )?;
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
        .env("HOME", &dir)
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
        final_frame.matches("› findme").count(),
        1,
        "Enter accepts the preview; it must not submit a third turn: {final_frame}"
    );
    assert!(
        final_frame.contains("│findme"),
        "the accepted match stays in the composer: {final_frame}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// `--deadline` seconds arrive straight off the command line, and
/// `Instant + Duration` panics rather than saturating: an unclamped value
/// aborted the process at 101 before the loop ran a single step.
#[test]
fn an_absurd_deadline_is_clamped_rather_than_panicking() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-deadline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
        .env("HOME", &dir)
        .output()?;
    let _ = std::fs::remove_dir_all(&dir);
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

/// One path for `--record` and `--snap` left the still truncating the
/// recording still open on it, and the run exited 0 having lost it.
#[test]
fn one_path_for_both_capture_sinks_is_refused() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-samepath-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
        .env("HOME", &dir)
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("different files"), "{stderr}");
    Ok(())
}

/// A paced `type` step holds the outer loop for its whole duration, and the
/// wall clock is read there: without a check inside the step, a long paced
/// line ran to completion past the deadline meant to bound it. Counting the
/// characters that landed says so without depending on wall time, which here
/// is dominated by the one-time kernel setup.
#[test]
fn a_paced_type_step_still_honours_the_deadline() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-paced-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
        .env("HOME", &dir)
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut dumps: Vec<_> = std::fs::read_dir(&frames)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    dumps.sort();
    let last = std::fs::read_to_string(dumps.last().ok_or("no frames dumped")?)?;
    let landed = last.matches('x').count();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("timed out"), "{stderr}");
    assert!(
        landed < 30,
        "the step typed {landed} of 40 characters, so it ran past its deadline"
    );
    Ok(())
}
