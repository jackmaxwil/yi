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

/// What one program call may collect before its value says `cut`; a test lowers it so a cut
/// costs kilobytes and a hundred files rather than 64 MiB and 100,000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fuse {
    pub bytes: usize,
    pub items: usize,
}

impl Fuse {
    pub const PROGRAM: Self = Self {
        bytes: yi_types::tool_value::PROGRAM_FUSE_BYTES,
        items: yi_types::tool_value::PROGRAM_FUSE_ITEMS,
    };
}

pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancelled: CancelFlag,
    /// The family board's spill dir, kept until the session is removed; None means a reducer
    /// must hand back the raw text instead.
    pub recovery_dir: Option<PathBuf>,
    /// The session's own transcript, which its wall spares under the session stores (#971).
    pub transcript: Option<PathBuf>,
    /// How long a command may hold the turn before it keeps running as a job, when the call
    /// passes no `wait`. None, the default, backgrounds only a call that does.
    pub auto_background: Option<std::time::Duration>,
    /// Set when the permission layer contained this call rather than asking.
    pub sandbox: Option<crate::sandbox::Sandbox>,
    /// Asked once per contained spawn, so its network has a holder.
    pub spawn: Option<crate::SpawnFn>,
    /// Invariant: paths the reviewer wall hides from this agent. A tool reading a tree rather
    /// than a named path shows the wall no target, so it consults this set itself.
    pub deny_read: Vec<PathBuf>,
    pub deny_write: Vec<PathBuf>,
    /// Invariant: set, bash runs each command in this docker container, never on the host.
    pub container: Option<String>,
    /// The id of the call being executed, so a tool that asks the user in its
    /// own right can name the cell that is waiting. Empty when no id exists.
    pub call_id: String,
    pub job_owner: Option<crate::jobs::JobOwner>,
    /// Invariant: set for a call a kernel program made; a tool then fills [`ToolOutput::value`]
    /// and collects past the caps of the model's view, since the program reads the value.
    pub program: bool,
    pub fuse: Fuse,
    pub hold_opens: Option<yi_permission::OpenHold>,
}

impl ToolContext {
    /// The gate every file a call names opens through, so the file judged is the file opened
    /// (#890). Built once per call: each guarded directory costs a stat.
    pub(crate) fn read_gate(&self) -> yi_permission::ReadGate {
        let context = yi_permission::CatastrophicContext::detect(&self.cwd);
        let own = self.recovery_dir.iter().chain(&self.transcript).cloned();
        let mut gate = yi_permission::ReadGate::new(&context).except(own.collect());
        gate.hold_opens(self.hold_opens.clone());
        gate
    }

    /// A named file's bytes, read through [`Self::read_gate`] under the wall's `deny_read`, at
    /// most [`READ_FILE_CAP`] of them: the cap every text read and edit takes.
    pub(crate) fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        read_capped(
            self.read_gate().open(path, &self.deny_read)?,
            path,
            Cap::Text,
        )
    }

    /// [`Self::read`] with the stat of the handle it read, taken before the read: a rewrite after
    /// it moves the file's mtime off the recorded one rather than onto it.
    pub(crate) fn read_stat(&self, path: &Path) -> std::io::Result<(Vec<u8>, std::fs::Metadata)> {
        let file = self.read_gate().open(path, &self.deny_read)?;
        let stat = file.metadata()?;
        Ok((read_capped(file, path, Cap::Text)?, stat))
    }

    /// [`Self::read`] for a document, under the converter's own ceiling: the parsed text, not the
    /// source bytes, is what reaches the model, and a page selection parses only those pages.
    pub(crate) fn read_document(&self, path: &Path, cap: Cap) -> std::io::Result<Vec<u8>> {
        read_capped(self.read_gate().open(path, &self.deny_read)?, path, cap)
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
            transcript: None,
            auto_background: None,
            sandbox: None,
            spawn: None,
            deny_read: Vec::new(),
            deny_write: Vec::new(),
            container: None,
            call_id: String::new(),
            job_owner: None,
            program: false,
            fuse: Fuse::PROGRAM,
            hold_opens: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub result: ToolResult,
    pub is_error: bool,
    /// The data the text was rendered from, a `yi_types::tool_value` shape, filled only for a
    /// [`ToolContext::program`] call and never written to the transcript.
    pub value: Option<Value>,
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

    /// Each file this call would change, its text before and after, read without touching disk.
    fn changes(&self, _input: &Map<String, Value>, _cwd: &std::path::Path) -> Vec<FileChange> {
        Vec::new()
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput;
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: std::path::PathBuf,
    pub before: String,
    pub after: String,
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
        value: None,
    }
}

pub fn error_output(message: impl Into<String>) -> ToolOutput {
    let mut output = text_output(message);
    output.is_error = true;
    output
}

/// Invariant: a command that ran and exited non-zero is a result the model reads; only one Yi
/// could not finish, or the sandbox refused, is an error.
pub(crate) fn ran_output(text: String, details: Value, unfinished: bool) -> ToolOutput {
    let mut output = text_output(text);
    output.result.details = details;
    output.is_error = unfinished;
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

/// A loud cut's row: kept of what existed, the cap that cut it, and the call that gets the rest.
pub fn cut_row(
    kept: usize,
    total: usize,
    what: &str,
    cap: &str,
    limit: usize,
    next_call: &str,
) -> String {
    format!("[\u{2026} {kept} of {total} {what} ({cap} {limit}); the rest: {next_call}]")
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

/// The most of one file `read` shows or `edit` takes whole; past it grep searches the file.
pub const READ_FILE_CAP: usize = 16 * 1024 * 1024;

/// [`read_all`] up to its cap; past it a `FileTooLarge` error whose text is the cut row.
pub(crate) fn read_capped(file: std::fs::File, path: &Path, cap: Cap) -> std::io::Result<Vec<u8>> {
    let limit = cap.limit();
    let ceiling = u64::try_from(limit).unwrap_or(u64::MAX);
    let size = file.metadata().map_or(0, |meta| meta.len());
    let mut bytes = Vec::new();
    if size <= ceiling {
        let mut limited = std::io::Read::take(&file, ceiling.saturating_add(1));
        std::io::Read::read_to_end(&mut limited, &mut bytes)?;
    }
    if size <= ceiling && bytes.len() <= limit {
        return Ok(bytes);
    }
    let size = size.max(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    let total = usize::try_from(size).unwrap_or(usize::MAX);
    let row = cut_row(0, total, "bytes read", cap.name(), limit, &cap.next(path));
    Err(std::io::Error::new(std::io::ErrorKind::FileTooLarge, row))
}

/// What a capped read may take of one file, and what its cut row says.
#[derive(Clone, Copy)]
pub(crate) enum Cap {
    /// Text the model reads as bytes, capped at [`READ_FILE_CAP`].
    Text,
    /// A document converted whole, capped at the converter's ceiling; `pages` says a page
    /// selection could parse past that ceiling, which only a PDF can.
    Document { pages: bool },
    /// A document whose page selection parses only those pages.
    Paged,
}

impl Cap {
    fn limit(self) -> usize {
        let bytes = match self {
            Self::Text => u64::try_from(READ_FILE_CAP).unwrap_or(u64::MAX),
            Self::Document { .. } => crate::document::INPUT_CEILING,
            Self::Paged => crate::document::PAGED_CEILING,
        };
        usize::try_from(bytes).unwrap_or(usize::MAX)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Text => "READ_FILE_CAP",
            Self::Document { .. } => "INPUT_CEILING",
            Self::Paged => "PAGED_CEILING",
        }
    }

    fn next(self, path: &Path) -> String {
        let shown = path.display();
        match self {
            Self::Text => {
                format!("grep path={shown} with a pattern, or bash `sed -n '1,2000p' {shown}`")
            }
            Self::Document { pages: false } => {
                "no read converts a document this large, whatever the page selection".to_owned()
            }
            Self::Document { pages: true } => {
                format!("read path={shown} pages=1-50 for the pages it can convert")
            }
            Self::Paged => {
                "no read reaches past this: the file is too large to convert at any page selection"
                    .to_owned()
            }
        }
    }
}

/// Said when [`overwrite`] could not stage a sibling and wrote the file in place instead.
pub(crate) const IN_PLACE: &str = "the file was written in place (no sibling could replace its name), so a reader during the write could see it short";

/// Replaces the file the gate judged: a sibling temp renamed over its name once that name still
/// names `file`, so no reader or crash ever sees it short; a hard-linked file is written in place.
/// Returns [`IN_PLACE`] when the sibling could not be made or renamed and the write fell back.
pub(crate) fn overwrite(
    file: &std::fs::File,
    path: &Path,
    content: &[u8],
) -> std::io::Result<Option<&'static str>> {
    replace(file, path, content, &mut || ())
}

/// [`overwrite`] with `held` run where another process could swap the name: the one window a test
/// can drive, and the reason production passes an empty one.
fn replace(
    file: &std::fs::File,
    path: &Path,
    content: &[u8],
    held: &mut dyn FnMut(),
) -> std::io::Result<Option<&'static str>> {
    use std::io::{Seek, Write};
    let meta = file.metadata()?;
    let in_place = |mut file: &std::fs::File| {
        file.set_len(0)?;
        file.rewind()?;
        file.write_all(content)
    };
    let fell_back = |file: &std::fs::File| in_place(file).map(|()| Some(IN_PLACE));
    if yi_permission::links(&meta) > 1 {
        return in_place(file).map(|()| None);
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return fell_back(file);
    };
    static TEMPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = TEMPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{}.yi-{}-{serial}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    let Ok(mut staged) = options.write(true).create_new(true).open(&temp) else {
        return fell_back(file);
    };
    // Invariant: a sibling takes this process's owner and the directory's group, so a file whose
    // owner or group differs is written in place to keep both.
    if staged.metadata().map(|staged| owners(&staged)).ok() != Some(owners(&meta)) {
        let _already_gone = std::fs::remove_file(&temp);
        return fell_back(file);
    }
    let staged_ok = (|| {
        staged.set_permissions(meta.permissions())?;
        staged.write_all(content)?;
        staged.sync_all()?;
        held();
        let named = std::fs::symlink_metadata(path)?;
        let id = yi_permission::file_id;
        if named.file_type().is_symlink() || id(&named) != id(&meta) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{} no longer names the file that was opened (a link or another file was put there); nothing was written",
                    path.display()
                ),
            ));
        }
        Ok(())
    })();
    let landed = match staged_ok {
        Ok(()) => match std::fs::rename(&temp, path) {
            Ok(()) => Ok(None),
            Err(_) => fell_back(file),
        },
        Err(error) => Err(error),
    };
    if !matches!(landed, Ok(None)) {
        let _already_gone = std::fs::remove_file(&temp);
    }
    landed
}

#[cfg(unix)]
fn owners(meta: &std::fs::Metadata) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    (meta.uid(), meta.gid())
}

#[cfg(not(unix))]
fn owners(_meta: &std::fs::Metadata) -> (u32, u32) {
    (0, 0)
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

#[cfg(test)]
mod tests {
    use crate::scratch::Scratch;
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn a_name_swapped_for_a_symlink_before_the_rename_is_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-tools-write-atomic-swap")?;
        let (path, victim) = (dir.join("a.txt"), dir.join("victim.txt"));
        fs::write(&path, "judged\n")?;
        fs::write(&victim, "kept\n")?;
        let file = fs::OpenOptions::new().read(true).write(true).open(&path)?;
        let (name, target) = (path.clone(), victim.clone());
        let mut held = move || {
            let _ = fs::remove_file(&name);
            let _ = std::os::unix::fs::symlink(&target, &name);
        };
        let landed = super::replace(&file, &path, b"new\n", &mut held);
        assert!(landed.is_err(), "the swapped name was written");
        assert!(fs::symlink_metadata(&path)?.file_type().is_symlink());
        assert_eq!(fs::read_to_string(&victim)?, "kept\n");
        Ok(())
    }

    /// Incident: a read-only parent proved this fallback on macOS and proved nothing in CI, where
    /// the runner is root and 0o555 stops no one, so staging succeeded and the row never appeared.
    #[test]
    fn a_write_into_a_name_with_no_room_for_a_sibling_writes_in_place_and_says_so()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = Scratch::new("yi-tools-write-in-place")?;
        let name = format!("{}.json", "a".repeat(250));
        let path = dir.join(&name);
        fs::write(&path, "{}")?;
        let mut input = serde_json::Map::new();
        input.insert("path".to_owned(), name.into());
        input.insert("content".to_owned(), "[1]".into());
        let context = super::ToolContext::new(dir.to_path_buf());
        let output = super::Tool::execute(&crate::builtins::WriteTool::default(), input, &context);
        let text = yi_types::message::join_text(&output.result.content, "");
        assert!(!output.is_error, "{text}");
        assert_eq!(fs::read_to_string(&path)?, "[1]");
        let rows: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(
            rows,
            ["syntax: ok", &format!("warning: {}", super::IN_PLACE)]
        );
        assert_eq!(output.result.details["syntax"], "syntax: ok");
        Ok(())
    }
}
