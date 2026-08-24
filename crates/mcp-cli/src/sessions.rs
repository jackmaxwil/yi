use std::path::{Path, PathBuf};

use serde_json::Value;
use yi_types::mcp::{McpServerSpec, McpSessionRecord, McpSessionState, McpSessionsFile};

pub fn now_ms() -> u64 {
    #[expect(
        clippy::disallowed_methods,
        reason = "session bookkeeping timestamps are this module's job"
    )]
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub struct SessionsStore {
    root: PathBuf,
    file: McpSessionsFile,
}

impl SessionsStore {
    pub fn open(root: PathBuf) -> Self {
        let file = std::fs::read_to_string(root.join("sessions.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { root, file }
    }

    pub fn get(&self, name: &str) -> Option<McpSessionRecord> {
        self.file
            .sessions
            .get(name)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    pub fn names(&self) -> Vec<String> {
        self.file.sessions.keys().cloned().collect()
    }

    pub fn upsert(&mut self, record: &McpSessionRecord) -> Result<(), String> {
        let value = serde_json::to_value(record).map_err(|error| error.to_string())?;
        self.file.sessions.insert(record.name.clone(), value);
        self.save()
    }

    pub fn set_state(&mut self, name: &str, state: McpSessionState) -> Result<(), String> {
        let mut record = self
            .get(name)
            .ok_or_else(|| format!("unknown session @{name}"))?;
        record.state = state;
        record.updated_at = now_ms();
        self.upsert(&record)
    }

    fn save(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let text = serde_json::to_string_pretty(&self.file).map_err(|error| error.to_string())?;
        std::fs::write(self.root.join("sessions.json"), text).map_err(|error| error.to_string())
    }

    pub fn snapshot_path(&self, name: &str) -> PathBuf {
        self.root.join("snapshots").join(format!("{name}.json"))
    }

    /// Caches the connect-time discovery result (tools + instructions) that
    /// `grep` searches — progressive discovery never needs a live server.
    pub fn write_snapshot(&self, name: &str, snapshot: &Value) -> Result<(), String> {
        let path = self.snapshot_path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(snapshot).map_err(|error| error.to_string())?;
        std::fs::write(path, text).map_err(|error| error.to_string())
    }

    pub fn read_snapshot(&self, name: &str) -> Option<Value> {
        std::fs::read_to_string(self.snapshot_path(name))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
    }
}

pub fn new_record(name: &str, spec: McpServerSpec) -> McpSessionRecord {
    let timestamp = now_ms();
    McpSessionRecord {
        name: name.to_owned(),
        spec,
        state: McpSessionState::Connecting,
        protocol_version: None,
        server_name: None,
        instructions: None,
        created_at: timestamp,
        updated_at: timestamp,
        extra: serde_json::Map::new(),
    }
}

/// Default session name from the server reference: config entry name, else
/// the last path-ish segment (mcpc: mcp.apify.com -> @apify).
pub fn default_session_name(server: &str) -> String {
    let base = server
        .rsplit_once(':')
        .map_or(server, |(_, entry)| entry)
        .trim_end_matches('/');
    let host = base
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let candidate = host.split('/').next().unwrap_or(host);
    let candidate = candidate.split('.').next().unwrap_or(candidate);
    let cleaned: String = candidate
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "session".to_owned()
    } else {
        cleaned
    }
}

pub fn mcp_root(home: &Path) -> PathBuf {
    home.join(".yi").join("mcp")
}
