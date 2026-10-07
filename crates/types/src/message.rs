use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

pub const ENVIRONMENT_TAG: &str = "<environment>";

/// The `raw_stop_reason` of a turn that ended on the provider's in-band error chunk, not on
/// the wire: written by the completions mapper, read by the loop's retry rule (D175).
pub const RAW_STOP_IN_BAND_ERROR: &str = "error";

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
    /// The request a child or one-shot session was started with: its requester's words, read
    /// bare like the user's, but never served by `user://`.
    Task,
    /// Text the host wrote, named by its producer; the model reads it as runtime context.
    Host(HostSource),
}

/// Who in the host wrote a user-role message; the label the model reads it under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostSource {
    Notice,
    Restore,
    Job,
    Lifecycle,
    Mail,
    Landing,
    Deadline,
}

impl HostSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Notice => "notice",
            Self::Restore => "restore",
            Self::Job => "job",
            Self::Lifecycle => "lifecycle",
            Self::Mail => "mail",
            Self::Landing => "landing",
            Self::Deadline => "deadline",
        }
    }
}

/// When D25 landed (2026-09-01): no message written before it carries an attribution.
pub const ATTRIBUTED_SINCE_MS: u64 = 1_788_256_519_000;

impl Attribution {
    pub fn is_unproven(&self) -> bool {
        matches!(self, Self::Unproven)
    }

    /// Whether a user-role message stored at `stored_ms` is its requester's words, for the model
    /// and every surface alike; a host message stamps 0, so only a dated one predates attribution.
    pub fn reads_as_typed(self, stored_ms: u64) -> bool {
        match self {
            Self::User | Self::Task => true,
            Self::Unproven => stored_ms != 0 && stored_ms < ATTRIBUTED_SINCE_MS,
            Self::Host(_) => false,
        }
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
    // Pi v4 session files carry negative token deltas in usage adjustment records; unsigned fields
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

/// yi's one spelling of money: 3 decimals under $1, 2 above. `lower_bound`: a reply in the sum
/// came back without usage, so its cost is missing, not zero (`≥$0.450`, or `$?` alone).
pub fn fmt_cost(total: f64, lower_bound: bool) -> String {
    let mark = if lower_bound { "≥" } else { "" };
    if lower_bound && total <= 0.0 {
        "$?".to_owned()
    } else if total > 0.0 && total < 0.0005 {
        format!("{mark}<$0.001")
    } else if total < 0.9995 {
        format!("{mark}${total:.3}")
    } else {
        format!("{mark}${total:.2}")
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

/// The text blocks of `blocks` joined by `sep`; every other kind of block is skipped.
pub fn join_text(blocks: &[Content], sep: &str) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(sep)
}

/// Whether any text block carries a non-empty answer; tool calls and thinking are not text.
pub fn has_text(blocks: &[Content]) -> bool {
    blocks
        .iter()
        .any(|block| matches!(block, Content::Text { text, .. } if !text.trim().is_empty()))
}

impl AgentMessage {
    /// Host text in the user role, named by its producer; the model reads it as runtime context.
    pub fn host_text(source: HostSource, text: &str, timestamp: u64) -> Self {
        Self::User {
            content: UserContent::Text(text.to_owned()),
            timestamp,
            attribution: Attribution::Host(source),
        }
    }

    /// The request a child or one-shot session runs on, read bare and never served by `user://`.
    pub fn task(text: &str, timestamp: u64) -> Self {
        Self::User {
            content: UserContent::Text(text.to_owned()),
            timestamp,
            attribution: Attribution::Task,
        }
    }

    /// Host text shown in the transcript as a `custom_type` note, with no details attached.
    pub fn host_note(custom_type: &str, text: String, timestamp: u64) -> Self {
        Self::Custom {
            custom_type: custom_type.to_owned(),
            content: UserContent::Text(text),
            display: true,
            details: None,
            timestamp,
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

    /// Every text this message carries, for search and for a brief line: assistant tool calls
    /// read `name {args}`, bash reads `command\noutput`, thinking is excluded.
    pub fn plain_text(&self) -> String {
        match self {
            Self::User { content, .. } | Self::Custom { content, .. } => match content {
                UserContent::Text(text) => text.clone(),
                UserContent::Blocks(blocks) => join_text(blocks, "\n"),
            },
            Self::Assistant { content, .. } => content
                .iter()
                .filter_map(|block| match block {
                    Content::Text { text, .. } => Some(text.clone()),
                    Content::ToolCall {
                        name, arguments, ..
                    } => Some(format!(
                        "{name} {}",
                        serde_json::Value::Object(arguments.clone())
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::ToolResult { content, .. } => join_text(content, "\n"),
            Self::BashExecution {
                command, output, ..
            } => format!("{command}\n{output}"),
            Self::BranchSummary { summary, .. } | Self::CompactionSummary { summary, .. } => {
                summary.clone()
            }
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
        let host = AgentMessage::host_text(super::HostSource::Notice, "hi", 7);
        assert_eq!(
            host.attribution(),
            Attribution::Host(super::HostSource::Notice)
        );
        assert!(!host.attribution().reads_as_typed(7));
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
