use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Session states (design §5.2): sessions are never auto-removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpSessionState {
    Live,
    Connecting,
    Disconnected,
    Unauthorized,
    Expired,
}

/// How to reach an MCP server: a local stdio command or a remote HTTP URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerSpec {
    #[serde(rename_all = "camelCase")]
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        env: Map<String, Value>,
    },
    #[serde(rename_all = "camelCase")]
    Http { url: String },
}

/// One named `@session` record in `~/.yi/mcp/sessions.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSessionRecord {
    pub name: String,
    pub spec: McpServerSpec,
    pub state: McpSessionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The sessions file: `{ "sessions": { name: record } }`; unknown fields
/// survive round-trips.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpSessionsFile {
    #[serde(default)]
    pub sessions: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// OAuth profile metadata for one MCP server (design D37). Stored in
/// `~/.yi/mcp/profiles.json` — metadata only, never tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOauthProfile {
    pub name: String,
    pub server_url: String,
    pub issuer: String,
    pub client_id: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    /// Whether the callback must carry a matching `iss` parameter (RFC 9207).
    #[serde(default)]
    pub iss_required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The profiles file: `{ "profiles": { key: profile } }`, keyed
/// `<profile-name>@<server-host>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpProfilesFile {
    #[serde(default)]
    pub profiles: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One credential set, serialized only into the token store (keychain entry
/// or a 0600 file — design D37); never into profiles or sessions files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTokenSet {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Absolute expiry in epoch ms; 0 = unknown (treat as non-expiring).
    #[serde(default)]
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
