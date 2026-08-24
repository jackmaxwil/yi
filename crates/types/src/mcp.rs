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
