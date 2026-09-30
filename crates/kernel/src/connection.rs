use std::io::Read;
use std::path::{Path, PathBuf};

use yi_types::kernel::ConnectionInfo;

pub fn has_resolved_ports(info: &ConnectionInfo) -> bool {
    [
        info.shell_port,
        info.iopub_port,
        info.stdin_port,
        info.control_port,
        info.hb_port,
    ]
    .iter()
    .all(|port| *port > 0)
}

pub fn parse_connection_info(value: &serde_json::Value) -> Option<ConnectionInfo> {
    let info: ConnectionInfo = serde_json::from_value(value.clone()).ok()?;
    if info.ip != "127.0.0.1" {
        return None;
    }
    if info.transport != "tcp" {
        return None;
    }
    if info.signature_scheme != "hmac-sha256" {
        return None;
    }
    Some(info)
}

pub fn read_connection_info(path: &Path) -> Option<ConnectionInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_connection_info(&serde_json::from_str(&text).ok()?)
}

pub struct Connection {
    pub info: ConnectionInfo,
    pub path: PathBuf,
    pub temp_dir: PathBuf,
}

pub fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0_u8; bytes];
    let mut source = std::fs::File::open("/dev/urandom")
        .map_err(|error| format!("open /dev/urandom: {error}"))?;
    source
        .read_exact(&mut buffer)
        .map_err(|error| format!("read /dev/urandom: {error}"))?;
    let mut out = String::with_capacity(bytes.saturating_mul(2));
    for byte in &buffer {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

/// One directory per kernel, each hidden from every sandbox profile but its own kernel's: a
/// connection file's key runs code in that kernel, and a contained process reaches loopback.
pub fn connection_root(home: &Path) -> PathBuf {
    home.join(".yi").join("kernel-connections")
}

/// A connection file in `dir`, made afresh with whatever a dead kernel left there removed
/// unfollowed, or with no `dir` in a new directory under [`connection_root`].
pub fn make_connection(home: &Path, dir: Option<&Path>) -> Result<Connection, String> {
    let info = ConnectionInfo {
        ip: "127.0.0.1".to_owned(),
        transport: "tcp".to_owned(),
        shell_port: 0,
        iopub_port: 0,
        stdin_port: 0,
        control_port: 0,
        hb_port: 0,
        signature_scheme: "hmac-sha256".to_owned(),
        key: random_hex(16)?,
        kernel_name: "python3".to_owned(),
    };
    let root = connection_root(home);
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("create {}: {error}", root.display()))?;
    set_mode(&root, 0o700)?;
    sweep_dead(&root);
    let temp_dir = match dir {
        Some(dir) => {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir(dir)
                .map_err(|error| format!("create {}: {error}", dir.display()))?;
            set_mode(dir, 0o700)?;
            dir.to_path_buf()
        }
        None => mkdtemp(&root, &format!("{}-k", std::process::id()))?,
    };
    let path = temp_dir.join("connection.json");
    let text = serde_json::to_string_pretty(&info).map_err(|error| error.to_string())?;
    write_new_600(&path, text.as_bytes())?;
    Ok(Connection {
        info,
        path,
        temp_dir,
    })
}

/// Directories named `<pid>-…` whose yi died without disposing its kernel; a live pid's stay.
fn sweep_dead(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = (name.to_str().and_then(|name| name.split_once('-')))
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        let dead = |pid: u32| matches!(crate::lock::process_is_running(pid), Ok(false));
        if pid.is_some_and(|pid| pid != std::process::id() && dead(pid)) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn mkdtemp(root: &Path, prefix: &str) -> Result<PathBuf, String> {
    for _ in 0..16 {
        let dir = root.join(format!("{prefix}{}", random_hex(6)?));
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                set_mode(&dir, 0o700)?;
                return Ok(dir);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("create {}: {error}", dir.display())),
        }
    }
    Err("mkdtemp: exhausted retries".to_owned())
}

/// Created, never opened through a link: `create_new` refuses one planted at `path`.
fn write_new_600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    (options.open(path))
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|error| format!("write {}: {error}", path.display()))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| format!("chmod {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use crate::scratch::Scratch;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Review of #925 (F5): the file is created, never opened through whatever sits at its name.
    #[test]
    fn a_link_or_file_planted_at_the_connection_file_is_refused() -> TestResult {
        let root = Scratch::new("yi-connection-plant")?;
        let target = root.join("target.txt");
        std::fs::write(&target, "KEEP")?;
        let linked = root.join("linked.json");
        std::os::unix::fs::symlink(&target, &linked)?;
        let planted = root.join("planted.json");
        std::fs::write(&planted, "PLANTED")?;
        let refused = [&linked, &planted].map(|path| super::write_new_600(path, b"{}").is_err());
        assert_eq!(refused, [true, true]);
        assert_eq!(std::fs::read_to_string(&target)?, "KEEP");
        assert_eq!(std::fs::read_to_string(&planted)?, "PLANTED");
        Ok(())
    }

    /// Review of #925 (F6): a crashed yi's directories go at the next start; a live pid's stay.
    #[test]
    fn a_start_sweeps_the_directories_of_dead_pids() -> TestResult {
        let home = Scratch::new("yi-connection-sweep")?;
        let mut exited =
            crate::bootstrap::command(std::path::Path::new("/usr/bin/true")).spawn()?;
        let dead = exited.id();
        exited.wait()?;
        let mut running = (crate::bootstrap::command(std::path::Path::new("/bin/sleep")))
            .arg("30")
            .spawn()?;
        let root = super::connection_root(&home);
        let stale = root.join(format!("{dead}-0"));
        let live = root.join(format!("{}-0", running.id()));
        std::fs::create_dir_all(&stale)?;
        std::fs::create_dir_all(&live)?;
        let connection = super::make_connection(&home, None);
        running.kill()?;
        running.wait()?;
        let connection = connection?;
        assert_eq!((stale.exists(), live.exists()), (false, true));
        assert!(connection.temp_dir.starts_with(&root));
        Ok(())
    }
}
