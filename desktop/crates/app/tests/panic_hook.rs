//! A panic in the shipped app says itself in one line in the app's own log, which the Logs
//! pane tails; its own process, since the hook is global.

use std::error::Error;
use std::path::PathBuf;

use yi_desktop::start;

#[test]
#[allow(clippy::panic, reason = "a panic is what the hook is handed")]
fn a_panic_is_one_line_in_the_apps_own_log() -> Result<(), Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/panic-hook")
        .join(std::process::id().to_string());
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    let log = dir.join("Logs/yi-desktop.log");
    start::on_panic(Some(log.clone()));
    let died = std::thread::Builder::new()
        .name("worker".into())
        .spawn(|| std::panic::panic_any("the grid\nhas a hole".to_owned()))?
        .join();
    assert!(died.is_err(), "the thread did not panic");
    let text = std::fs::read_to_string(&log)?;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text:?}");
    let line = lines.first().copied().unwrap_or_default();
    assert!(line.starts_with("yi-desktop: panicked at "), "{line}");
    assert!(line.contains("panic_hook.rs:"), "{line}");
    assert!(line.ends_with(": the grid has a hole"), "{line}");
    Ok(())
}
