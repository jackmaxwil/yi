use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

pub(crate) fn start(home: &Path) {
    let Some(yi_runtime::classifier::Endpoint { checkpoint, url }) =
        yi_runtime::classifier::endpoint(crate::config())
    else {
        return;
    };
    let Some(port) = loopback_port(&url) else {
        return;
    };
    let home = home.to_path_buf();
    std::thread::spawn(move || match own(&home, &checkpoint, port) {
        Ok(Some(_child)) => loop {
            std::thread::park();
        },
        Ok(None) => {}
        Err(error) => eprintln!("warning: the classifier sidecar did not start: {error}"),
    });
}

/// Invariant: the log's lock rides the sidecar's stdout and its stdin pipe is held only by this
/// daemon, so the lock frees only once the sidecar is gone, however the daemon ended.
#[expect(
    clippy::disallowed_methods,
    reason = "yi serve owns the classifier sidecar's lifetime"
)]
fn own(home: &Path, checkpoint: &str, port: u16) -> std::io::Result<Option<std::process::Child>> {
    let mut log = yi_runtime::session_store::lock_file(&home.join(".yi/laya-serve.log"))?;
    log.seek(std::io::SeekFrom::End(0))?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok() {
        return Ok(None);
    }
    let binary: PathBuf = home.join(crate::setup::LAYA_VENV).join("bin/laya-serve");
    if !binary.is_file() {
        writeln!(
            log,
            "yi serve: {} is missing; `yi setup` installs it",
            binary.display()
        )?;
        return Ok(None);
    }
    let stderr = log.try_clone()?;
    let mut command = std::process::Command::new("sh");
    command
        .args([
            "-c",
            r#""$0" & pid=$!; read -r _; kill "$pid"; wait "$pid""#,
        ])
        .arg(&binary)
        .env("LAYA_HOST", "127.0.0.1")
        .env("LAYA_PORT", port.to_string())
        .env("LAYA_MODELS", checkpoint)
        .stdin(std::process::Stdio::piped())
        .stdout(log)
        .stderr(stderr);
    if let Some(key) = yi_runtime::auth::api_key("laya") {
        command.env("LAYA_API_KEY", key.expose());
    }
    command.spawn().map(Some)
}

fn loopback_port(url: &str) -> Option<u16> {
    url.strip_prefix("http://127.0.0.1:")?
        .split('/')
        .next()?
        .parse()
        .ok()
}
