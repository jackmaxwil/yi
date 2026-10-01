use std::path::{Path, PathBuf};
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
    /// How long a command may hold the turn before it keeps running as a job, when the call
    /// passes no `wait`. None, the default, backgrounds only a call that does.
    pub auto_background: Option<std::time::Duration>,
    /// Set when the permission layer contained this call rather than asking.
    pub sandbox: Option<crate::sandbox::Sandbox>,
    /// Invariant: paths the reviewer wall hides from this agent. A tool reading a tree rather
    /// than a named path shows the wall no target, so it consults this set itself.
    pub deny_read: Vec<PathBuf>,
    pub deny_write: Vec<PathBuf>,
    /// Invariant: set, bash runs each command in this docker container, never on the host.
    pub container: Option<String>,
    /// The id of the call being executed, so a tool that asks the user in its
    /// own right can name the cell that is waiting. Empty when no id exists.
    pub call_id: String,
}

impl ToolContext {
    /// The gate every file a call names opens through, so the file judged is the file opened
    /// (#890). Built once per call: each guarded directory costs a stat.
    pub(crate) fn read_gate(&self) -> yi_permission::ReadGate {
        yi_permission::ReadGate::new(&yi_permission::CatastrophicContext::detect(&self.cwd))
    }

    /// A named file's bytes, read through [`Self::read_gate`] under the wall's `deny_read`.
    pub(crate) fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        read_all(self.read_gate().open(path, &self.deny_read)?)
    }

    /// What a write may not land on: both lists of the wall.
    pub(crate) fn write_walls(&self) -> Vec<PathBuf> {
        [self.deny_write.as_slice(), self.deny_read.as_slice()].concat()
    }

    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            cancelled: Arc::new(|| false),
            recovery_dir: None,
            auto_background: None,
            sandbox: None,
            deny_read: Vec::new(),
            deny_write: Vec::new(),
            container: None,
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

    /// Shell commands this call arms to run later on the host, outside any sandbox (a todo
    /// blocked on `exec://`); the gate judges each as the bash call it amounts to.
    fn arms(&self, _input: &Map<String, Value>) -> Vec<String> {
        Vec::new()
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
    let head = text
        .get(..text.floor_char_boundary(DETAIL_CAP))
        .unwrap_or_default();
    Value::String(format!("{head}\n… truncated at {DETAIL_CAP} bytes"))
}

pub(crate) fn clip(line: &str, cols: usize) -> (String, bool) {
    match line.char_indices().nth(cols) {
        Some((end, _)) => (
            format!("{}\u{2026}", line.get(..end).unwrap_or_default()),
            true,
        ),
        None => (line.to_owned(), false),
    }
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

pub(crate) fn read_all(mut file: std::fs::File) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)?;
    Ok(bytes)
}

/// Replaces a file's content through the handle the gate judged, never the name again.
pub(crate) fn overwrite(file: &mut std::fs::File, content: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek, Write};
    file.set_len(0)?;
    file.rewind()?;
    file.write_all(content)
}

pub fn resolve_path(context: &ToolContext, path: &str) -> PathBuf {
    yi_permission::resolve_target(path, &context.cwd)
}

/// A glob whose literal head resolves like any path argument, `~`, `../` and absolute alike:
/// the head is the directory a walk starts from, and the rest matches below it.
pub(crate) struct RootedGlob {
    pub(crate) base: PathBuf,
    rest: globset::GlobMatcher,
}

impl RootedGlob {
    pub(crate) fn matches(&self, path: &Path) -> bool {
        path.strip_prefix(&self.base)
            .is_ok_and(|rest| self.rest.is_match(rest))
    }
}

/// The literal head of a glob, the directory its walk starts from, as the path gate must judge
/// it; None when no component holds a glob character.
pub fn glob_head(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split('/').collect();
    let at = (parts.iter()).position(|part| part.contains(['*', '?', '[', '{']))?;
    Some(rooted(raw, &parts.get(..at).unwrap_or_default().join("/")))
}

fn rooted(raw: &str, head: &str) -> String {
    match head.is_empty() && raw.starts_with('/') {
        true => "/".to_owned(),
        false => head.to_owned(),
    }
}

/// Split at the glob's head; without a glob character, at the last component.
pub(crate) fn rooted_glob(raw: &str, base: &Path) -> Result<RootedGlob, String> {
    let head = glob_head(raw)
        .unwrap_or_else(|| rooted(raw, raw.rsplit_once('/').map_or("", |(head, _)| head)));
    let rest = raw.get(head.len()..).unwrap_or(raw).trim_start_matches('/');
    let rest = globset::GlobBuilder::new(rest)
        .literal_separator(false)
        .build()
        .map_err(|error| error.to_string())?
        .compile_matcher();
    let head = yi_permission::resolve_target(&head, base);
    Ok(RootedGlob {
        base: yi_permission::lexical_normalize(&head),
        rest,
    })
}
