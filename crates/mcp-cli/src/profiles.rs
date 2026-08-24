use std::path::PathBuf;

use serde_json::Value;
use yi_types::mcp::{McpOauthProfile, McpProfilesFile};

use crate::oauth::profile_key;

pub struct ProfilesStore {
    root: PathBuf,
    file: McpProfilesFile,
}

impl ProfilesStore {
    pub fn open(root: PathBuf) -> Self {
        let file = std::fs::read_to_string(root.join("profiles.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { root, file }
    }

    pub fn get(&self, profile: &str, server_url: &str) -> Option<McpOauthProfile> {
        self.file
            .profiles
            .get(&profile_key(profile, server_url))
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    pub fn upsert(&mut self, profile: &McpOauthProfile) -> Result<(), String> {
        let key = profile_key(&profile.name, &profile.server_url);
        let value = serde_json::to_value(profile).map_err(|error| error.to_string())?;
        self.file.profiles.insert(key, value);
        self.save()
    }

    pub fn remove(&mut self, profile: &str, server_url: &str) -> Result<(), String> {
        self.file
            .profiles
            .shift_remove(&profile_key(profile, server_url));
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let text = serde_json::to_string_pretty(&self.file).map_err(|error| error.to_string())?;
        std::fs::write(self.root.join("profiles.json"), text).map_err(|error| error.to_string())
    }
}

pub fn value_url(value: &Value) -> Option<&str> {
    value.get("serverUrl").and_then(Value::as_str)
}
