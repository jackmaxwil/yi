use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Content {
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redacted: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    Image { data: String, mime_type: String },
    #[serde(rename_all = "camelCase")]
    ToolCall {
        id: String,
        name: String,
        arguments: Map<String, Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
    },
}

/// D25: who a user-role message actually came from. Absent means [`Attribution::Unproven`],
/// so older messages and host-minted ones are unresolvable by `user://`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Attribution {
    #[default]
    Unproven,
    User,
}

impl Attribution {
    pub fn is_unproven(&self) -> bool {
        matches!(self, Self::Unproven)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<Content>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Pending,
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    pub input: Number,
    pub output: Number,
    pub cache_read: Number,
    pub cache_write: Number,
    pub total: Number,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write1h: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<i64>,
    // Pi writes negative token deltas in usage adjustment records; unsigned fields
    // would fail to load those session files.
    pub total_tokens: i64,
    pub cost: Cost,
    // A provider that reported no usage object at all, as opposed to one reporting zeros.
    // Absent means known, so files written before the field re-serialize byte-identically.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unknown: bool,
}

impl Usage {
    pub fn zero() -> Self {
        let zero = || serde_json::Number::from(0u64);
        Self {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write1h: None,
            reasoning: None,
            total_tokens: 0,
            cost: Cost {
                input: zero(),
                output: zero(),
                cache_read: zero(),
                cache_write: zero(),
                total: zero(),
            },
            unknown: false,
        }
    }

    pub fn unknown() -> Self {
        Self {
            unknown: true,
            ..Self::zero()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticErrorInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessageDiagnostic {
    #[serde(rename = "type")]
    pub diagnostic_type: String,
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DiagnosticErrorInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredHandle {
    pub provider: String,
    pub model_id: String,
    pub api: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "wire DTOs mirror the JSON; variants are parse-then-drop, boxing buys nothing"
)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum AgentMessage {
    #[serde(rename_all = "camelCase")]
    User {
        content: UserContent,
        timestamp: u64,
        #[serde(default, skip_serializing_if = "Attribution::is_unproven")]
        attribution: Attribution,
    },
    #[serde(rename_all = "camelCase")]
    Assistant {
        content: Vec<Content>,
        api: String,
        provider: String,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        response_model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
        usage: Usage,
        stop_reason: StopReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        deferred: Option<DeferredHandle>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_stop_reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        end_turn: Option<bool>,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<Content>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        added_tool_names: Option<Vec<String>>,
        is_error: bool,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    BashExecution {
        command: String,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i64>,
        cancelled: bool,
        truncated: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        full_output_path: Option<String>,
        timestamp: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        exclude_from_context: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    Custom {
        custom_type: String,
        content: UserContent,
        display: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    BranchSummary {
        summary: String,
        from_id: String,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    CompactionSummary {
        summary: String,
        tokens_before: u64,
        timestamp: u64,
    },
}

impl AgentMessage {
    /// A user-role message the host minted for itself; it never carries the
    /// user's authority.
    pub fn host_user(content: UserContent, timestamp: u64) -> Self {
        Self::User {
            content,
            timestamp,
            attribution: Attribution::Unproven,
        }
    }

    /// Input that crossed the process boundary from the user, the only kind
    /// `user://` serves.
    pub fn user_input(content: UserContent, timestamp: u64) -> Self {
        Self::User {
            content,
            timestamp,
            attribution: Attribution::User,
        }
    }

    pub fn attribution(&self) -> Attribution {
        match self {
            Self::User { attribution, .. } => *attribution,
            Self::Assistant { .. }
            | Self::ToolResult { .. }
            | Self::BashExecution { .. }
            | Self::Custom { .. }
            | Self::BranchSummary { .. }
            | Self::CompactionSummary { .. } => Attribution::Unproven,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentMessage, Attribution, UserContent};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn an_absent_field_reads_as_unproven_and_writes_nothing_back() -> TestResult {
        let wire = r#"{"role":"user","content":"hi","timestamp":7}"#;
        let message: AgentMessage = serde_json::from_str(wire)?;
        assert_eq!(message.attribution(), Attribution::Unproven);
        assert_eq!(serde_json::to_string(&message)?, wire);
        Ok(())
    }

    #[test]
    fn only_the_user_input_mint_carries_the_users_authority() -> TestResult {
        let host = AgentMessage::host_user(UserContent::Text("hi".to_owned()), 7);
        assert_eq!(host.attribution(), Attribution::Unproven);
        let typed = AgentMessage::user_input(UserContent::Text("hi".to_owned()), 7);
        assert_eq!(typed.attribution(), Attribution::User);
        let wire = serde_json::to_string(&typed)?;
        assert!(wire.contains(r#""attribution":"user""#), "{wire}");
        assert_eq!(
            serde_json::from_str::<AgentMessage>(&wire)?.attribution(),
            Attribution::User
        );
        Ok(())
    }
}
