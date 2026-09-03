use std::path::{Path, PathBuf};

use yi_types::mcp::McpTokenSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenStore {
    Keychain,
    File,
}

impl TokenStore {
    /// `mcp.tokenStore` config value; the darwin default is the OS keychain.
    pub fn from_config(value: Option<&str>) -> Self {
        match value {
            Some("file") => Self::File,
            Some("keychain") => Self::Keychain,
            _ if cfg!(target_os = "macos") => Self::Keychain,
            _ => Self::File,
        }
    }
}

const KEYCHAIN_SERVICE: &str = "yi-mcp";

fn security(args: &[&str]) -> Result<std::process::Output, String> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the darwin keychain is only reachable through the security(1) subprocess"
    )]
    std::process::Command::new("security")
        .args(args)
        .output()
        .map_err(|error| format!("security: {error}"))
}

fn token_file(root: &Path, key: &str) -> PathBuf {
    let safe: String = key
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    root.join("tokens").join(format!("{safe}.json"))
}

pub struct Tokens {
    store: TokenStore,
    root: PathBuf,
}

impl Tokens {
    pub fn new(store: TokenStore, root: PathBuf) -> Self {
        Self { store, root }
    }

    pub fn save(&self, key: &str, tokens: &McpTokenSet) -> Result<(), String> {
        let json = serde_json::to_string(tokens).map_err(|error| error.to_string())?;
        match self.store {
            TokenStore::Keychain => {
                let output = security(&[
                    "add-generic-password",
                    "-U",
                    "-a",
                    key,
                    "-s",
                    KEYCHAIN_SERVICE,
                    "-w",
                    &json,
                ])?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "keychain write failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ))
                }
            }
            TokenStore::File => {
                let path = token_file(&self.root, key);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&path, &json).map_err(|error| error.to_string())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
                Ok(())
            }
        }
    }

    pub fn load(&self, key: &str) -> Option<McpTokenSet> {
        let json = match self.store {
            TokenStore::Keychain => {
                let output = security(&[
                    "find-generic-password",
                    "-a",
                    key,
                    "-s",
                    KEYCHAIN_SERVICE,
                    "-w",
                ])
                .ok()?;
                if !output.status.success() {
                    return None;
                }
                String::from_utf8_lossy(&output.stdout).trim().to_owned()
            }
            TokenStore::File => std::fs::read_to_string(token_file(&self.root, key)).ok()?,
        };
        serde_json::from_str(&json).ok()
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        match self.store {
            TokenStore::Keychain => {
                let _ = security(&["delete-generic-password", "-a", key, "-s", KEYCHAIN_SERVICE])?;
                Ok(())
            }
            TokenStore::File => {
                let path = token_file(&self.root, key);
                if path.exists() {
                    std::fs::remove_file(&path).map_err(|error| error.to_string())?;
                }
                Ok(())
            }
        }
    }

    /// Invariant: a rotating refresh token is spent by exactly one process
    /// (D37). O_EXCL lockfile; stale locks (> 60 s) are broken.
    pub fn with_refresh_lock<T>(
        &self,
        key: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let dir = self.root.join("locks");
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let lock = token_file(&self.root, key).with_extension("lock");
        let lock = dir.join(lock.file_name().unwrap_or_default());
        let mut waited = 0u64;
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock)
            {
                Ok(_) => break,
                Err(_) => {
                    let stale = std::fs::metadata(&lock)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age.as_secs() > 60);
                    if stale {
                        let _ = std::fs::remove_file(&lock);
                        continue;
                    }
                    if waited >= 10_000 {
                        return Err("refresh lock held too long by another process".to_owned());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    waited += 100;
                }
            }
        }
        let outcome = action();
        let _ = std::fs::remove_file(&lock);
        outcome
    }
}
