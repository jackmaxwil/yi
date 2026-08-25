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
        // `wait-idle` only blocks once the turn is actually running, and the
        // prompt reaches the runtime over a channel, so the short wait is what
        // makes the second wait-idle wait for the turn instead of skipping it.
        "wait-idle 10000\ntype follow-up\nkey enter\nwait 200\nwait-idle 10000\nquit\n",
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
