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
}

pub type CancelFlag = Arc<dyn Fn() -> bool + Send + Sync>;

pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancelled: CancelFlag,
    /// Where lossy tool output is tee'd so it stays recoverable (T19). None
    /// means a reducer must hand back the raw text instead.
    pub recovery_dir: Option<PathBuf>,
    /// D13: how long a foreground command may hold the turn before it keeps
    /// running as a job. None (the default) never backgrounds anything.
    pub auto_background: Option<std::time::Duration>,
}

impl ToolContext {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            cancelled: Arc::new(|| false),
            recovery_dir: None,
            auto_background: None,
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

    fn irreversible(&self, _input: &Map<String, Value>) -> bool {
        !matches!(self.kind(), ToolKind::Read)
    }

    fn validate(&self, _input: &Map<String, Value>) -> Result<(), String> {
        Ok(())
    }

    /// T13: the approval prompt judges a mutation by what it changes, not by
    /// its path, so a mutating tool renders its own diff before it runs. Only
    /// the tool can — the patch language and the snapshot store it validates
    /// against are the tool's, not the permission layer's.
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

pub fn require_str<'a>(input: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing required string argument: {key}"))
}

pub fn resolve_path(context: &ToolContext, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        context.cwd.join(candidate)
    }
}
