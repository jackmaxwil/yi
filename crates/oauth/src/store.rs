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
        std::fs::write(&path, json.to_string()).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
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
