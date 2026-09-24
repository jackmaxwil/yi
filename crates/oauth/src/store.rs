use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};
use yi_types::oauth::{CREDENTIAL_SCHEMA, CredentialFile};

use crate::{Error, Result};

const EXPIRY_SLACK: Duration = Duration::from_secs(30);
/// A refresh can take 40 s on the wire — a waiter must outwait it or it streams
/// on the expired token it came in with.
const LOCK_POLL: Duration = Duration::from_millis(80);
const LOCK_WAIT: Duration = Duration::from_secs(50);

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
    /// `None` never expires.
    pub expires: Option<SystemTime>,
    pub account: Option<String>,
    pub org: Option<String>,
    /// Fields a newer Yi wrote that this one does not know; rewritten untouched.
    pub extra: Map<String, Value>,
}

pub struct Store {
    root: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn safe(provider: &str) -> String {
    provider
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[expect(
    clippy::disallowed_methods,
    reason = "token expiry timestamps are this module's job"
)]
pub fn now() -> SystemTime {
    SystemTime::now()
}

fn millis(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn at(millis: u64) -> Option<SystemTime> {
    (millis != 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_millis(millis))
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
        self.root
            .join("tokens")
            .join(format!("{}.json", safe(provider)))
    }

    /// Every stored provider id, read from the token directory itself so a provider
    /// without a login profile still logs out (registry::ids never saw one).
    pub fn list(&self) -> Vec<String> {
        let mut found: Vec<String> = std::fs::read_dir(self.root.join("tokens"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".json"))
                    .map(str::to_owned)
            })
            .collect();
        found.sort();
        found
    }

    pub fn save(&self, provider: &str, credential: &Credential) -> Result<()> {
        let path = self.path(provider);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let file = CredentialFile {
            version: CREDENTIAL_SCHEMA,
            kind: match credential.kind {
                Kind::Oauth => "oauth".to_owned(),
                Kind::Key => "key".to_owned(),
            },
            access: credential.access.clone(),
            refresh: credential.refresh.clone(),
            expires_ms: credential.expires.map(millis).unwrap_or(0),
            account: credential.account.clone(),
            org: credential.org.clone(),
            extra: credential.extra.clone(),
        };
        // Atomic + 0600 like the profile generator: the token never sits on disk
        // with wider permissions, and a crash leaves the old file, not a half one.
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_string(&file)?;
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
        let file: CredentialFile =
            serde_json::from_str(&std::fs::read_to_string(self.path(provider)).ok()?).ok()?;
        let kind = match file.kind.as_str() {
            "oauth" => Kind::Oauth,
            "key" => Kind::Key,
            _ => return None,
        };
        Some(Credential {
            kind,
            access: file.access,
            refresh: file.refresh,
            expires: at(file.expires_ms),
            account: file.account,
            org: file.org,
            extra: file.extra,
        })
    }

    pub fn delete(&self, provider: &str) -> Result<()> {
        let path = self.path(provider);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn expired(credential: &Credential) -> bool {
        Self::expired_at(credential, now())
    }

    pub fn expired_at(credential: &Credential, now: SystemTime) -> bool {
        credential
            .expires
            .is_some_and(|at| now + EXPIRY_SLACK >= at)
    }

    pub fn with_refresh_lock<T>(
        &self,
        provider: &str,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let locks = self.root.join("locks");
        std::fs::create_dir_all(&locks).map_err(|error| error.to_string())?;
        let path = locks.join(format!("{}.lock", safe(provider)));
        // Incident: a pid-file lock broken by age let two waiters refresh at once and a
        // rotated refresh token lost; the OS drops a flock with its holder instead.
        let file = std::fs::File::create(&path)?;
        let started = std::time::Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return action(),
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
            if started.elapsed() > LOCK_WAIT {
                return Err(Error::from("refresh lock held too long by another process"));
            }
            std::thread::sleep(LOCK_POLL);
        }
    }
}

#[cfg(unix)]
fn write_fresh_0600(
    path: &std::path::Path,
    bytes: &[u8],
) -> std::result::Result<(), std::io::Error> {
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
fn write_fresh_0600(
    path: &std::path::Path,
    bytes: &[u8],
) -> std::result::Result<(), std::io::Error> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .truncate(true)
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, bytes))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::{Credential, Kind, Store};
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, SystemTime};

    fn credential(access: &str) -> Credential {
        Credential {
            kind: Kind::Oauth,
            access: access.to_owned(),
            refresh: Some("r".to_owned()),
            expires: Some(SystemTime::UNIX_EPOCH + Duration::from_millis(1)),
            account: None,
            org: None,
            extra: Default::default(),
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

    #[test]
    fn a_rewrite_keeps_the_fields_this_build_does_not_know() {
        let root = std::env::temp_dir().join(format!("yi-oauth-extra-{}", std::process::id()));
        let store = Store::open(root.clone());
        let mut one = credential("one");
        one.extra.insert("device".to_owned(), "laptop".into());
        store.save("p", &one).unwrap();
        let mut loaded = store.load("p").unwrap();
        loaded.access = "two".to_owned();
        store.save("p", &loaded).unwrap();
        let reread = store.load("p").unwrap();
        assert_eq!(reread.access, "two");
        assert_eq!(reread.extra["device"].as_str(), Some("laptop"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn list_reads_the_token_directory_and_the_lock_name_is_sanitized() {
        let root = std::env::temp_dir().join(format!("yi-oauth-list-{}", std::process::id()));
        let store = Store::open(root.clone());
        store.save("openai", &credential("a")).unwrap();
        store.save("weird/name", &credential("b")).unwrap();
        assert_eq!(
            store.list(),
            vec!["openai".to_owned(), "weird_name".to_owned()]
        );
        store
            .with_refresh_lock("weird/name", || {
                assert!(root.join("locks").join("weird_name.lock").exists());
                Ok(())
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
