use std::path::PathBuf;

use serde_json::{Map, Value};
use yi_types::oauth::ProfileFile;

use crate::Result;

/// Profiles live beside the token store so a catalog refresh can never rewrite
/// them: `~/.yi/oauth/<provider>.json`, the binary ships none.
pub const PROFILE_DIR: &str = "oauth";

#[derive(Debug, Clone)]
pub enum Kind {
    OauthCode(Box<OauthCode>),
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

/// Header and query maps are JSON objects so a generator can write them by name;
/// `preserve_order` keeps the author's order on the wire.
fn pairs(map: &Map<String, Value>) -> Vec<(String, String)> {
    map.iter()
        .filter_map(|(name, item)| Some((name.clone(), item.as_str()?.to_owned())))
        .collect()
}

fn oauth_code(id: &str, file: ProfileFile) -> Result<Kind> {
    let need = |field: Option<String>, key: &str| -> Result<String> {
        field.ok_or_else(|| format!("{id}: profile needs `{key}`").into())
    };
    Ok(Kind::OauthCode(Box::new(OauthCode {
        id: id.to_owned(),
        client_id: need(file.client_id, "client_id")?,
        client_secret: file.client_secret,
        authorize: need(file.authorize, "authorize")?,
        token: need(file.token, "token")?,
        scopes: file.scopes.unwrap_or_default(),
        callback_port: file
            .callback_port
            .ok_or_else(|| format!("{id}: profile needs `callback_port`"))?,
        callback_path: file.callback_path.unwrap_or_else(|| "/callback".to_owned()),
        redirect_host: file.redirect_host.unwrap_or_else(|| "localhost".to_owned()),
        port_fallback: file.port_fallback,
        json_token: file.json_token,
        extra_authorize: pairs(&file.extra_authorize),
        stream_headers: pairs(&file.stream_headers),
        refresh_headers: pairs(&file.refresh_headers),
    })))
}

pub fn parse(id: &str, value: &Value) -> Result<Kind> {
    let file: ProfileFile = serde_json::from_value(value.clone()).map_err(|error| {
        crate::Error::from(format!("{id}: profile is not the profile shape: {error}"))
    })?;
    if !file.extra.is_empty() {
        let mut keys: Vec<&str> = file.extra.keys().map(String::as_str).collect();
        keys.sort_unstable();
        return Err(format!(
            "{id}: unknown profile key(s) {}; every key is one of {:?}",
            keys.join(", "),
            ProfileFile::KNOWN_KEYS
        )
        .into());
    }
    match file.kind.as_str() {
        "oauth-code" => oauth_code(id, file),
        "api-key" => Ok(Kind::ApiKey),
        other => Err(format!("{id}: unknown profile kind {other}").into()),
    }
}

/// A profile file wins over the built-in list. A file that exists but does not
/// parse is an `Err` naming why — never a silent fall through to "no profile".
pub fn lookup(provider: &str) -> Result<Option<Kind>> {
    match load(provider)? {
        Some(kind) => Ok(Some(kind)),
        None => Ok(API_KEY_IDS.contains(&provider).then_some(Kind::ApiKey)),
    }
}

pub fn load(provider: &str) -> Result<Option<Kind>> {
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
