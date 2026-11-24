//! Starting `yi serve` when its socket is dead, and the wall clock the wire stamps with.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use yd_core::ids::Millis;
use yd_core::wire::client::{Clock, Starter};

/// The flags a dead daemon was serving with, one per line; Yi writes it beside the socket.
const INVOCATION: &str = "invocation";
/// Bytes of the flags file read: it is another program's to grow.
const INVOCATION_MAX: u64 = 64 * 1024;

/// `PATH` folders looked in for `yi`: a `PATH` is the user's to grow, and each look is a stat.
const LOOKS: usize = 64;

/// `yi` is in none of the places a shell would find it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("yi is not installed: not on PATH ({}) and not at {}", listed(.folders), .fallback.display())]
pub struct NoYi {
    pub folders: Vec<PathBuf>,
    pub fallback: PathBuf,
}

fn listed(folders: &[PathBuf]) -> String {
    let named: Vec<String> = folders.iter().map(|f| f.display().to_string()).collect();
    if named.is_empty() {
        "empty".to_owned()
    } else {
        named.join(":")
    }
}

/// The installed `yi`: the first absolute `PATH` entry that holds one, else `~/.local/bin/yi`.
/// A GUI launch has a bare `PATH`, so the fallback is what an app from the Dock finds.
pub fn installed_yi() -> Result<PathBuf, NoYi> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    find_yi(std::env::var_os("PATH").as_deref(), home.as_deref())
}

/// [`installed_yi`] over a given `PATH` and home.
pub fn find_yi(path: Option<&std::ffi::OsStr>, home: Option<&Path>) -> Result<PathBuf, NoYi> {
    let folders: Vec<PathBuf> = path
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|dir| dir.is_absolute())
        .take(LOOKS)
        .collect();
    let fallback = home.unwrap_or(Path::new("")).join(".local/bin/yi");
    let found = folders
        .iter()
        .map(|dir| dir.join("yi"))
        .find(|yi| yi.is_file());
    match found.or_else(|| fallback.is_file().then(|| fallback.clone())) {
        Some(yi) => Ok(yi),
        None => Err(NoYi { folders, fallback }),
    }
}

/// `serve --socket <socket>` and the flags the last daemon on it recorded.
pub fn arguments(socket: &Path) -> Vec<OsString> {
    use std::io::Read;
    let mut flags = Vec::new();
    if let Ok(file) = std::fs::File::open(socket.with_extension(INVOCATION)) {
        let _ = file.take(INVOCATION_MAX).read_to_end(&mut flags);
    }
    let flags = String::from_utf8_lossy(&flags).into_owned();
    let mut args: Vec<OsString> = vec!["serve".into(), "--socket".into(), socket.into()];
    args.extend(
        flags
            .lines()
            .map(str::trim)
            .filter(|flag| !flag.is_empty())
            .map(OsString::from),
    );
    args
}

/// Starts `yi serve` on a socket, detached in its own process group so it outlives the app.
/// The client never calls it while the stop marker is there.
pub fn starter(yi: PathBuf) -> Starter {
    Box::new(move |socket: &Path| spawn(&yi, socket))
}

#[allow(
    clippy::disallowed_methods,
    reason = "the daemon's one spawn: yi by absolute path with an argument list"
)]
fn spawn(yi: &Path, socket: &Path) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    // Invariant: a relative path would be looked up from the app's own folder.
    if !yi.is_absolute() {
        return Err(io::Error::other(format!(
            "{} is not an absolute path",
            yi.display()
        )));
    }
    let mut child = Command::new(yi)
        .args(arguments(socket))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    // The daemon outlives a wait here; this thread only reaps it if it ends first.
    std::thread::Builder::new()
        .name("yi-serve-reaper".to_owned())
        .spawn(move || child.wait().map(drop))?;
    Ok(())
}

/// Unix epoch milliseconds: the server's `lastEventMs` and entry times are on this clock.
pub fn clock() -> Clock {
    Arc::new(now)
}

#[allow(
    clippy::disallowed_methods,
    reason = "the one wall clock the wire client is given"
)]
fn now() -> Millis {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Millis(u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/server-tests")
            .join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[test]
    fn yi_is_found_on_path_before_the_fallback() -> Result<(), Box<dyn Error>> {
        let (bin, home) = (folder("path-bin")?, folder("path-home")?);
        std::fs::write(bin.join("yi"), "")?;
        let path = std::env::join_paths([Path::new("relative"), &bin])?;
        assert_eq!(find_yi(Some(&path), Some(&home)), Ok(bin.join("yi")));
        Ok(())
    }

    #[test]
    fn a_missing_yi_names_every_place_looked() -> Result<(), Box<dyn Error>> {
        let (bin, home) = (folder("none-bin")?, folder("none-home")?);
        let path = std::env::join_paths([&bin])?;
        let missing = find_yi(Some(&path), Some(&home))
            .err()
            .ok_or("found a yi")?;
        let said = missing.to_string();
        assert!(said.contains(&bin.display().to_string()), "{said}");
        assert!(
            said.contains(&home.join(".local/bin/yi").display().to_string()),
            "{said}"
        );
        Ok(())
    }

    #[test]
    fn a_restart_reuses_the_flags_the_dead_daemon_recorded() -> Result<(), Box<dyn Error>> {
        let socket = folder("flags")?.join("daemon.sock");
        std::fs::write(
            socket.with_extension("invocation"),
            "--model\nfaux/faux-1\n--confirm",
        )?;
        let args = arguments(&socket);
        let want: Vec<OsString> = ["serve", "--socket"]
            .into_iter()
            .map(OsString::from)
            .chain([socket.clone().into_os_string()])
            .chain(["--model", "faux/faux-1", "--confirm"].map(OsString::from))
            .collect();
        assert_eq!(args, want);
        Ok(())
    }

    #[test]
    fn with_no_record_the_daemon_starts_on_its_own_defaults() -> Result<(), Box<dyn Error>> {
        let socket = folder("bare")?.join("daemon.sock");
        assert_eq!(arguments(&socket).len(), 3, "serve --socket <path> alone");
        Ok(())
    }

    #[test]
    fn the_clock_reads_unix_milliseconds() {
        // 2026-01-01 in Unix milliseconds: a seconds or monotonic clock reads far below it.
        assert!(now().0 > 1_767_225_600_000);
    }

    #[test]
    fn yi_is_found_by_an_absolute_path() {
        let yi = installed_yi().unwrap_or_else(|missing| missing.fallback);
        assert!(yi.is_absolute());
    }
}
