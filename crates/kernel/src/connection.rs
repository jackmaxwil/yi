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

pub fn make_connection(tmp_root: &Path) -> Result<Connection, String> {
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
    let temp_dir = mkdtemp(tmp_root, "yi-kernel-")?;
    let path = temp_dir.join("connection.json");
    let text = serde_json::to_string_pretty(&info).map_err(|error| error.to_string())?;
    write_mode_600(&path, text.as_bytes())?;
    Ok(Connection {
        info,
        path,
        temp_dir,
    })
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

fn write_mode_600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| format!("write {}: {error}", path.display()))?;
    set_mode(path, 0o600)
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
