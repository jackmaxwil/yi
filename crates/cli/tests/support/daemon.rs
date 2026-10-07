use std::error::Error;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Incident: a daemon on the real HOME started the owner's real classifier sidecar.
pub fn spawn_daemon(dir: &Path) -> Result<(Child, PathBuf), Box<dyn Error>> {
    let home = dir.join("home");
    std::fs::create_dir_all(&home)?;
    spawn_daemon_in(dir, &home)
}

pub fn spawn_daemon_in(dir: &Path, home: &Path) -> Result<(Child, PathBuf), Box<dyn Error>> {
    let socket = dir.join("yi.sock");
    #[expect(
        clippy::disallowed_methods,
        reason = "the daemon contract is the spawned binary's socket; tests must drive the real process"
    )]
    let child = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "serve",
            "--socket",
            &socket.display().to_string(),
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("HOME", home)
        .env_remove("LAYA_API_KEY")
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    // Incident: the socket file exists between bind() and listen(), and a
    // connect in that gap is refused; under load the gap outlasted the poll.
    while UnixStream::connect(&socket).is_err() {
        if Instant::now() > deadline {
            return Err("daemon socket never accepted a connection".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok((child, socket))
}
