//! The on-disk shapes of provider login (D191): the versioned token file and profile,
//! each with an `extra` map so a field a newer Yi wrote survives a rewrite (§20).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CREDENTIAL_SCHEMA: u64 = 1;
pub const PROFILE_SCHEMA: u64 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CredentialFile {
    pub version: u64,
    /// `oauth` or `key`.
    pub kind: String,
    pub access: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    /// Millis since the epoch; 0 means "never expires". The file speaks millis;
    /// the API speaks `SystemTime`.
    #[serde(default)]
    pub expires_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A profile as written on disk: every field optional, validation into a spec is
/// `yi-oauth`'s job — this struct is the shape, not the rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileFile {
    #[serde(default = "profile_schema")]
    pub version: u64,
    /// `oauth-code` or `api-key`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorize: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_host: Option<String>,
    #[serde(default)]
    pub port_fallback: bool,
    #[serde(default)]
    pub json_token: bool,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_authorize: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub stream_headers: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub refresh_headers: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn profile_schema() -> u64 {
    PROFILE_SCHEMA
}

impl ProfileFile {
    /// The keys a profile may carry; anything else is a typo the author meant to
    /// say something with (`stream_header` sending no headers was the incident).
    pub const KNOWN_KEYS: [&'static str; 15] = [
        "version",
        "kind",
        "client_id",
        "client_secret",
        "authorize",
        "token",
        "scopes",
        "callback_port",
        "callback_path",
        "redirect_host",
        "port_fallback",
        "json_token",
        "extra_authorize",
        "stream_headers",
        "refresh_headers",
    ];
}

#[cfg(test)]
mod tests {
    use super::{CredentialFile, ProfileFile};

    #[test]
    fn the_token_file_round_trips_fields_it_does_not_know() {
        let mut file: CredentialFile =
            serde_json::from_str(include_str!("../tests/fixtures/oauth-token-v1.json")).unwrap();
        assert_eq!(file.version, 1);
        assert_eq!(file.kind, "oauth");
        assert_eq!(file.extra["device"].as_str(), Some("laptop"));
        file.access = "rotated".to_owned();
        let written = serde_json::to_string(&file).unwrap();
        let reread: CredentialFile = serde_json::from_str(&written).unwrap();
        assert_eq!(reread, file, "a refresh rewrite keeps every unknown field");
    }

    #[test]
    fn the_profile_fixture_parses_with_defaults() {
        let file: ProfileFile =
            serde_json::from_str(include_str!("../tests/fixtures/oauth-profile-v1.json")).unwrap();
        assert_eq!(file.kind, "oauth-code");
        assert_eq!(file.callback_path.as_deref(), Some("/callback"));
        assert!(!file.port_fallback);
    }
}
