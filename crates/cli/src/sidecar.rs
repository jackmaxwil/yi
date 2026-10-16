use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

pub(crate) fn start(home: &Path) {
    let Some(yi_runtime::classifier::Endpoint { checkpoint, url }) =
        yi_runtime::classifier::endpoint(&crate::config())
    else {
        return;
    };
    let Some((host, port)) = loopback(&url) else {
        return note(
            home,
            &format!(
                "classifier.url {url} is not on this machine's loopback, so no sidecar is started"
            ),
        );
    };
    // Minted once here; the sidecar daemon reuses this same key.
    let key = yi_runtime::auth::key_in(home, "laya");
    if let Err(error) = &key {
        note(
            home,
            &format!("no laya key, so classifier approval stays off: {error}"),
        );
    }
    let home = home.to_path_buf();
    std::thread::spawn(
        move || match own(&home, &checkpoint, &host, port, key.ok()) {
            Ok(Some(_child)) => loop {
                std::thread::park();
            },
            Ok(None) => {}
            Err(error) => note(
                &home,
                &format!("the classifier sidecar did not start: {error}"),
            ),
        },
    );
}

/// Invariant: userinfo and an IPv6 loopback never start the sidecar, as before the shared
/// parser, which on its own accepts both (`http://localhost@localhost:8080`).
fn loopback(url: &str) -> Option<(String, u16)> {
    let authority = url
        .strip_prefix("http://")?
        .split(['/', '?', '#', '\\'])
        .next()?;
    if authority.contains('@') || authority.starts_with('[') {
        return None;
    }
    let (host, port) = yi_types::url::http_loopback_host_and_port(url)?;
    Some((host.to_owned(), port?.parse().ok()?))
}

/// A note made off the lock, so a daemon with no sidecar to start never waits on another's.
fn note(home: &Path, line: &str) {
    eprintln!("yi serve: {line}");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join(".yi/laya-serve.log"));
    if let Ok(mut log) = log {
        let _said = log.write_all(format!("yi serve: {line}\n").as_bytes());
    }
}

/// Invariant: the log's lock rides the sidecar's stdout and its stdin pipe is held only by this
/// daemon, so the lock frees only once the sidecar is gone, however the daemon ended.
#[expect(
    clippy::disallowed_methods,
    reason = "yi serve owns the classifier sidecar's lifetime"
)]
fn own(
    home: &Path,
    checkpoint: &str,
    host: &str,
    port: u16,
    minted: Option<yi_runtime::auth::Secret>,
) -> std::io::Result<Option<std::process::Child>> {
    let mut log = yi_runtime::session_store::lock_file(&home.join(".yi/laya-serve.log"))?;
    log.seek(std::io::SeekFrom::End(0))?;
    let answering = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))?.any(|addr| {
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
    });
    if answering {
        writeln!(
            log,
            "yi serve: something already answers on {host}:{port}, so no sidecar is started"
        )?;
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
        .env("LAYA_HOST", host)
        .env("LAYA_PORT", port.to_string())
        .env("LAYA_MODELS", checkpoint)
        .stdin(std::process::Stdio::piped())
        .stdout(log)
        .stderr(stderr);
    let key = minted.or_else(|| yi_runtime::auth::key_in(home, "laya").ok());
    if let Some(key) = key {
        command.env("LAYA_API_KEY", key.expose());
    }
    command.spawn().map(Some)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_plain_loopback_host_with_a_port_starts_the_sidecar() {
        let ok = [
            ("http://127.0.0.1:8080/x", ("127.0.0.1", 8080)),
            ("http://localhost:9090", ("localhost", 9090)),
        ];
        for (url, (host, port)) in ok {
            assert_eq!(super::loopback(url), Some((host.to_owned(), port)), "{url}");
        }
        for url in [
            "http://localhost@localhost:8080",
            "http://localhost:1@localhost:9090",
            "http://user@localhost:8080",
            "http://[::1]:8080",
            "http://localhost/",
            "http://evil.com\\@localhost:8080/",
            "https://localhost:8080",
        ] {
            assert_eq!(super::loopback(url), None, "{url}");
        }
    }
}
