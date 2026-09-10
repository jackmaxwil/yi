//! The bootstrap lock's holder probe, where the benchmark images run it: with no `kill(1)`.

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
    let lock_dir = std::env::temp_dir().join(format!("yi-kernel-probe-{}", std::process::id()));
    std::fs::create_dir_all(&lock_dir)?;
    let unanswered = Some(Err(std::io::Error::other("no shell")));
    let taken = yi_kernel::bootstrap::lock_is_stale(&lock_dir, unanswered);
    let dead = yi_kernel::bootstrap::lock_is_stale(&lock_dir, Some(Ok(false)));
    std::fs::remove_dir_all(&lock_dir)?;
    assert!(
        !taken,
        "a fresh lock went stale on a probe that could not run"
    );
    assert!(dead, "a dead holder kept its lock");
    Ok(())
}
