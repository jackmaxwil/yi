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
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
