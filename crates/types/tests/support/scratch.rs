//! Incident: per-test dirs under the system temp dir were never removed; by 2026-09-11 ~38,000
//! had piled up, hundreds of GB of them kernel venvs prewarmed into fake HOMEs. Every test
//! scratch dir goes through [`Scratch`], included by path into each binary that needs one.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> std::io::Result<Self> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    /// Incident: the default `kernel.prewarm` built a 380 MB venv into every fresh HOME a
    /// spawned `yi` saw. A test that needs the kernel still boots it on the first cell.
    #[allow(dead_code, reason = "the shared harness serves several test binaries")]
    pub fn home(&self) -> std::io::Result<PathBuf> {
        let home = self.0.join("home");
        std::fs::create_dir_all(home.join(".yi"))?;
        std::fs::write(
            home.join(".yi/config.json"),
            r#"{"kernel":{"prewarm":false}}"#,
        )?;
        Ok(home)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Incident: a drive test that flaked on the forge left nothing to read, its frames and
        // session gone with this dir; a panicking integration test keeps a copy under target/tmp.
        if std::thread::panicking()
            && let Some(kept) = keep(&self.0)
        {
            eprintln!("kept the failing test's scratch dir: {}", kept.display());
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A copy, not a rename: the system temp dir and the target dir are different filesystems on
/// the forge's runners. Sockets and symlinks are skipped.
fn keep(dir: &Path) -> Option<PathBuf> {
    let kept = Path::new(option_env!("CARGO_TARGET_TMPDIR")?)
        .join("failed")
        .join(dir.file_name()?);
    let mut stack = vec![(dir.to_path_buf(), kept.clone())];
    while let Some((from, to)) = stack.pop() {
        std::fs::create_dir_all(&to).ok()?;
        for entry in std::fs::read_dir(&from).ok()?.flatten() {
            let kind = entry.file_type().ok()?;
            let dest = to.join(entry.file_name());
            if kind.is_dir() {
                stack.push((entry.path(), dest));
            } else if kind.is_file() {
                std::fs::copy(entry.path(), dest).ok()?;
            }
        }
    }
    Some(kept)
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for Scratch {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}
