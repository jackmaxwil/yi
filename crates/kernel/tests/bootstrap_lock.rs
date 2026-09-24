//! The bootstrap lock's holder probe, where the benchmark images run it: with no `kill(1)`.
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

#[expect(
    clippy::disallowed_methods,
    reason = "the probe reads this process's PATH, so the test reruns its own binary without one"
)]
#[test]
fn a_live_holder_is_seen_with_no_kill_binary_on_path() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PATH").is_none_or(|path| path != "/nonexistent") {
        let rerun = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "a_live_holder_is_seen_with_no_kill_binary_on_path",
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
    assert!(
        yi_kernel::bootstrap::process_is_running(std::process::id())?,
        "a failed probe read this live process as a dead lock holder"
    );
    Ok(())
}

#[test]
fn a_probe_error_does_not_mark_a_lock_stale() -> Result<(), Box<dyn std::error::Error>> {
    let lock_dir = Scratch::new("yi-kernel-probe")?;
    let unanswered = Some(Err(std::io::Error::other("no shell")));
    let taken = yi_kernel::bootstrap::lock_is_stale(&lock_dir, unanswered);
    let dead = yi_kernel::bootstrap::lock_is_stale(&lock_dir, Some(Ok(false)));
    assert!(
        !taken,
        "a fresh lock went stale on a probe that could not run"
    );
    assert!(dead, "a dead holder kept its lock");
    Ok(())
}

/// Parallel first boots on one HOME: every unpack answers with the whole tree, and a reader
/// between them never finds it missing.
#[test]
fn parallel_first_boots_share_one_unpacked_runtime() -> Result<(), Box<dyn std::error::Error>> {
    let home = Scratch::new("yi-kernel-unpack")?;
    let root = home.join(".yi").join("python");
    let outcomes: Vec<Result<std::path::PathBuf, String>> = std::thread::scope(|scope| {
        let boots: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| yi_kernel::bootstrap::unpack_embedded_python(&home)))
            .collect();
        boots
            .into_iter()
            .map(|boot| boot.join().unwrap_or_else(|_| Err("panicked".to_owned())))
            .collect()
    });
    for outcome in outcomes {
        assert_eq!(outcome?, root);
    }
    assert!(root.join("yi_runtime").join("pyproject.toml").is_file());
    let entries: Vec<String> = std::fs::read_dir(home.join(".yi"))?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, ["python"], "staging or lock left behind");
    Ok(())
}
