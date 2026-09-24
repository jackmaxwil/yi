use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::event::ToolResult;
use yi_types::message::Content;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Write,
    Exec,
    Ledger,
}

pub type CancelFlag = Arc<dyn Fn() -> bool + Send + Sync>;

pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancelled: CancelFlag,
    /// None means a reducer must hand back the raw text instead.
    pub recovery_dir: Option<PathBuf>,
    /// How long a command may hold the turn before it keeps running as a job.
    /// None, the default, never backgrounds anything.
    pub auto_background: Option<std::time::Duration>,
    /// Set when the permission layer contained this call rather than asking.
    pub sandbox: Option<crate::sandbox::Sandbox>,
    /// Invariant: paths the reviewer wall hides from this agent. A tool reading a tree rather
    /// than a named path shows the wall no target, so it consults this set itself.
    pub deny_read: Vec<PathBuf>,
    /// The id of the call being executed, so a tool that asks the user in its
    /// own right can name the cell that is waiting. Empty when no id exists.
    pub call_id: String,
}

impl ToolContext {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            cancelled: Arc::new(|| false),
            recovery_dir: None,
            auto_background: None,
            sandbox: None,
            deny_read: Vec::new(),
            call_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub result: ToolResult,
    pub is_error: bool,
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    fn kind(&self) -> ToolKind;

    /// The kind this particular call has when the arguments decide it (a read-only shell
    /// command, a grep that writes); the loop overlaps and the wall screens by this one.
    fn kind_for(&self, _input: &Map<String, Value>) -> ToolKind {
        self.kind()
    }

    /// C8: a grammar for adapters that can take the input as raw text.
    fn freeform(&self) -> Option<yi_types::model::FreeformFormat> {
        None
    }

    /// By the call's own kind, so a grep that writes is no read to the permission gate (D180).
    fn irreversible(&self, input: &Map<String, Value>) -> bool {
        !matches!(self.kind_for(input), ToolKind::Read)
    }

    fn validate(&self, _input: &Map<String, Value>) -> Result<(), String> {
        Ok(())
    }

    /// The approval prompt judges a mutation by what it changes, and only the
    /// tool can render that: the patch language and snapshot store are its own.
    fn preview(&self, _input: &Map<String, Value>, _cwd: &std::path::Path) -> Option<String> {
        None
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput;
}

pub fn text_output(text: impl Into<String>) -> ToolOutput {
    ToolOutput {
        result: ToolResult {
            content: vec![Content::Text {
                text: text.into(),
                text_signature: None,
            }],
            details: Value::Object(Map::new()),
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: false,
    }
}

pub fn error_output(message: impl Into<String>) -> ToolOutput {
    let mut output = text_output(message);
    output.is_error = true;
    output
}

/// The machine-readable half of a failure; the prose stays the model's view.
pub fn error_output_kind(
    message: impl Into<String>,
    kind: yi_types::event::ToolErrorKind,
) -> ToolOutput {
    let mut output = error_output(message);
    if let Value::Object(details) = &mut output.result.details {
        details.insert(
            "errorKind".to_owned(),
            Value::String(kind.as_str().to_owned()),
        );
    }
    output
}

/// A `details` string is stored in the session file and replayed, so it is bounded where the
/// model-facing text is not. Past this a renderer has long since hit its own row budget.
pub const DETAIL_CAP: usize = 64 * 1024;

/// `text` for JSON `details`, truncated on a char boundary and marked when it
/// was. Returns `Value::Null` for empty, so the key can be skipped.
pub fn detail_text(text: &str) -> Value {
    if text.is_empty() {
        return Value::Null;
    }
    if text.len() <= DETAIL_CAP {
        return Value::String(text.to_owned());
    }
    let mut end = DETAIL_CAP;
    while end > 0 && !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let head = text.get(..end).unwrap_or_default();
    Value::String(format!("{head}\n… truncated at {DETAIL_CAP} bytes"))
}

pub fn require_str<'a>(input: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    match input.get(key) {
        Some(Value::String(text)) => Ok(text),
        // Incident: nine F0e `write` calls passed a JSON object as `content` for a .json path
        // and read "missing" as absent; the value was there and wrong-typed (#472).
        Some(value) => Err(format!(
            "{key} is {}, not a string; pass the text itself (json.dumps(obj, indent=2) for JSON)",
            match value {
                Value::Object(_) => "a JSON object",
                Value::Array(_) => "an array",
                Value::Null => "null",
                _ => "a number or boolean",
            }
        )),
        None => Err(format!("missing required string argument: {key}")),
    }
}

pub fn resolve_path(context: &ToolContext, path: &str) -> PathBuf {
    yi_permission::resolve_target(path, &context.cwd)
}
