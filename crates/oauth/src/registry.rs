use std::path::PathBuf;

use serde_json::Value;

/// Profiles live beside the token store so a catalog refresh can never rewrite
/// them: `~/.yi/oauth/<provider>.json`, the binary ships none.
pub const PROFILE_DIR: &str = "oauth";

#[derive(Debug, Clone)]
pub enum Kind {
    OauthCode(Box<OauthCode>),
    DeviceCode(Box<DeviceCode>),
    ApiKey,
}

#[derive(Debug, Clone)]
pub struct OauthCode {
    pub id: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub authorize: String,
    pub token: String,
    pub scopes: String,
    pub callback_port: u16,
    pub callback_path: String,
    pub redirect_host: String,
    pub port_fallback: bool,
    pub json_token: bool,
    pub extra_authorize: Vec<(String, String)>,
    /// Sent on every streamed request for this provider. The profile's author owns
    /// these values; nothing here is compiled into the binary.
    pub stream_headers: Vec<(String, String)>,
    pub refresh_headers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub id: String,
    pub client_id: String,
    pub device: String,
    pub token: String,
    pub scopes: String,
    pub stream_headers: Vec<(String, String)>,
}

/// Providers whose login is a pasted key: no endpoints, no identity, nothing to supply.
pub const API_KEY_IDS: [&str; 3] = ["openai", "openrouter", "google"];

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn profile_root() -> PathBuf {
    home().join(".yi").join(PROFILE_DIR)
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

pub fn profile_path(provider: &str) -> PathBuf {
    profile_root().join(format!("{}.json", safe(provider)))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Header and query maps are JSON objects so a generator can write them by name;
/// `preserve_order` keeps the author's order on the wire.
fn pairs(value: &Value, key: &str) -> Vec<(String, String)> {
    value
        .get(key)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, item)| Some((name.clone(), item.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

fn oauth_code(id: &str, value: &Value) -> Result<OauthCode, String> {
    let need = |key: &str| text(value, key).ok_or_else(|| format!("{id}: profile needs `{key}`"));
    Ok(OauthCode {
        id: id.to_owned(),
        client_id: need("client_id")?,
        client_secret: text(value, "client_secret"),
        authorize: need("authorize")?,
        token: need("token")?,
        scopes: text(value, "scopes").unwrap_or_default(),
        callback_port: value
            .get("callback_port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .ok_or_else(|| format!("{id}: profile needs `callback_port`"))?,
        callback_path: text(value, "callback_path").unwrap_or_else(|| "/callback".to_owned()),
        redirect_host: text(value, "redirect_host").unwrap_or_else(|| "localhost".to_owned()),
        port_fallback: flag(value, "port_fallback"),
        json_token: flag(value, "json_token"),
        extra_authorize: pairs(value, "extra_authorize"),
        stream_headers: pairs(value, "stream_headers"),
        refresh_headers: pairs(value, "refresh_headers"),
    })
}

fn device_code(id: &str, value: &Value) -> Result<DeviceCode, String> {
    let need = |key: &str| text(value, key).ok_or_else(|| format!("{id}: profile needs `{key}`"));
    Ok(DeviceCode {
        id: id.to_owned(),
        client_id: need("client_id")?,
        device: need("device")?,
        token: need("token")?,
        scopes: text(value, "scopes").unwrap_or_default(),
        stream_headers: pairs(value, "stream_headers"),
    })
}

pub fn parse(id: &str, value: &Value) -> Result<Kind, String> {
    match value.get("kind").and_then(Value::as_str) {
        Some("oauth-code") => oauth_code(id, value).map(|spec| Kind::OauthCode(Box::new(spec))),
        Some("device-code") => device_code(id, value).map(|spec| Kind::DeviceCode(Box::new(spec))),
        Some("api-key") => Ok(Kind::ApiKey),
        Some(other) => Err(format!("{id}: unknown profile kind {other}")),
        None => Err(format!("{id}: profile needs `kind`")),
    }
}

/// A profile file wins over the built-in list, so a pasted-key provider can be
/// given endpoints without a new binary.
pub fn lookup(provider: &str) -> Option<Kind> {
    match load(provider) {
        Ok(Some(kind)) => Some(kind),
        Ok(None) | Err(_) => API_KEY_IDS.contains(&provider).then_some(Kind::ApiKey),
    }
}

pub fn load(provider: &str) -> Result<Option<Kind>, String> {
    let path = profile_path(provider);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    parse(provider, &value).map(Some)
}

pub fn ids() -> Vec<String> {
    let mut found: Vec<String> = API_KEY_IDS.iter().map(|id| (*id).to_owned()).collect();
    let Ok(entries) = std::fs::read_dir(profile_root()) else {
        found.sort();
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        if !found.iter().any(|known| known == id) {
            found.push(id.to_owned());
        }
    }
    found.sort();
    found
}
