use std::path::PathBuf;

use serde_json::{Value, json};

const EXPIRY_SLACK_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Oauth,
    Key,
}

#[derive(Debug, Clone)]
pub struct Credential {
    pub kind: Kind,
    pub access: String,
    pub refresh: Option<String>,
    pub expires_ms: u64,
    pub account: Option<String>,
    pub org: Option<String>,
}

pub struct Store {
    root: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

impl Store {
    pub fn default_root() -> PathBuf {
        home().join(".yi").join("providers")
    }

    pub fn open(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn user() -> Self {
        Self::open(Self::default_root())
    }

    fn path(&self, provider: &str) -> PathBuf {
        let safe: String = provider
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        self.root.join("tokens").join(format!("{safe}.json"))
    }

    pub fn save(&self, provider: &str, credential: &Credential) -> Result<(), String> {
        let path = self.path(provider);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let json = json!({
            "kind": match credential.kind {
                Kind::Oauth => "oauth",
                Kind::Key => "key",
            },
            "access": credential.access,
            "refresh": credential.refresh,
            "expires_ms": credential.expires_ms,
            "account": credential.account,
            "org": credential.org,
        });
        // Atomic + 0600 like the profile generator: the token never sits on disk
        // with wider permissions, and a crash leaves the old file, not a half one.
        let tmp = path.with_extension("json.tmp");
        let bytes = json.to_string();
        write_fresh_0600(&tmp, bytes.as_bytes())
            .or_else(|error| {
                // A stale tmp from a crashed save is ours; remove it and retry once.
                std::fs::remove_file(&tmp).map_err(|_| error.to_string())?;
                write_fresh_0600(&tmp, bytes.as_bytes()).map_err(|e| e.to_string())
            })
            .map_err(|error| format!("{}: {error}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(())
    }

    pub fn load(&self, provider: &str) -> Option<Credential> {
        let json: Value =
            serde_json::from_str(&std::fs::read_to_string(self.path(provider)).ok()?).ok()?;
        let kind = match json.get("kind").and_then(Value::as_str)? {
            "oauth" => Kind::Oauth,
            "key" => Kind::Key,
            _ => return None,
        };
        Some(Credential {
            kind,
            access: json.get("access").and_then(Value::as_str)?.to_owned(),
            refresh: json
                .get("refresh")
                .and_then(Value::as_str)
                .map(str::to_owned),
            expires_ms: json.get("expires_ms").and_then(Value::as_u64).unwrap_or(0),
            account: json
                .get("account")
                .and_then(Value::as_str)
                .map(str::to_owned),
            org: json.get("org").and_then(Value::as_str).map(str::to_owned),
        })
    }

    pub fn delete(&self, provider: &str) -> Result<(), String> {
        let path = self.path(provider);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn expired(credential: &Credential, now_ms: u64) -> bool {
        credential.expires_ms != 0
            && now_ms.saturating_add(EXPIRY_SLACK_MS) >= credential.expires_ms
    }

    pub fn with_refresh_lock<T>(
        &self,
        provider: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let locks = self.root.join("locks");
        std::fs::create_dir_all(&locks).map_err(|error| error.to_string())?;
        let file = locks.join(format!("{provider}.lock"));
        for _ in 0..100 {
            if std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .is_ok()
            {
                let outcome = action();
                let _ = std::fs::remove_file(&file);
                return outcome;
            }
            let aged = std::fs::metadata(&file)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 45);
            if aged {
                let _ = std::fs::remove_file(&file);
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        Err("refresh lock held too long by another process".to_owned())
    }
}

#[cfg(unix)]
fn write_fresh_0600(path: &std::path::Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
}

#[cfg(not(unix))]
fn write_fresh_0600(path: &std::path::Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, bytes))
}

pub fn now_ms() -> u64 {
    #[expect(
        clippy::disallowed_methods,
        reason = "token expiry timestamps are this module's job"
    )]
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::{Credential, Kind, Store};
    use std::os::unix::fs::PermissionsExt;

    fn credential(access: &str) -> Credential {
        Credential {
            kind: Kind::Oauth,
            access: access.to_owned(),
            refresh: Some("r".to_owned()),
            expires_ms: 1,
            account: None,
            org: None,
        }
    }

    fn mode_of(store: &Store, provider: &str) -> u32 {
        std::fs::metadata(store.path(provider))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn a_saved_token_is_0600_from_the_first_save_and_every_rewrite() {
        let root = std::env::temp_dir().join(format!("yi-oauth-store-{}", std::process::id()));
        let store = Store::open(root.clone());
        store.save("p", &credential("one")).unwrap();
        assert_eq!(mode_of(&store, "p"), 0o600, "first save");
        store.save("p", &credential("two")).unwrap();
        assert_eq!(mode_of(&store, "p"), 0o600, "rewrite");
        assert_eq!(store.load("p").unwrap().access, "two");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_stale_tmp_from_a_crashed_save_does_not_stick_the_next_save() {
        let root = std::env::temp_dir().join(format!("yi-oauth-stale-{}", std::process::id()));
        let store = Store::open(root.clone());
        let tmp = store.path("p").with_extension("json.tmp");
        std::fs::create_dir_all(tmp.parent().unwrap()).unwrap();
        std::fs::write(&tmp, "half-written").unwrap();
        store.save("p", &credential("one")).unwrap();
        assert_eq!(store.load("p").unwrap().access, "one");
        assert_eq!(mode_of(&store, "p"), 0o600);
        let _ = std::fs::remove_dir_all(root);
    }
}
