use std::path::Path;
use std::process::{Command, Stdio};

/// `scripts/tui_pty.py` driving `binary` under a real pty for 60 s; the caller adds its own
/// harness flags, then `--` and the child's arguments.
pub fn pty_command(binary: &Path, home: &Path, cwd: &Path) -> Command {
    #[expect(
        clippy::disallowed_methods,
        reason = "an interactive run (control socket, restart hook) needs a real pty"
    )]
    let mut command = Command::new("python3");
    command
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/tui_pty.py"
        ))
        .args(["--seconds", "60", "--binary"])
        .arg(binary)
        .arg("--home")
        .arg(home)
        .current_dir(cwd)
        .stdout(Stdio::null());
    command
}
