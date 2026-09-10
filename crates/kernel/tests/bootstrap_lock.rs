//! The bootstrap lock's holder probe, where the benchmark images run it: with no `kill(1)`.

#[expect(
    clippy::disallowed_methods,
    reason = "the probe reads this process's PATH, so the test reruns its own binary without one"
)]
#[test]
fn a_live_holder_is_seen_with_no_kill_binary_on_path() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PATH").is_none_or(|path| path != "/nonexistent") {
        let status = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "a_live_holder_is_seen_with_no_kill_binary_on_path",
            ])
            .env("PATH", "/nonexistent")
            .status()?;
        assert!(status.success(), "the rerun with no kill binary: {status}");
        return Ok(());
    }
    assert!(
        yi_kernel::bootstrap::process_is_running(std::process::id()),
        "a failed probe read this live process as a dead lock holder"
    );
    Ok(())
}
