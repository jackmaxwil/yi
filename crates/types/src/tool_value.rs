//! The data a tool's text was rendered from, handed to a kernel program through `tool.call` and
//! never written to the transcript (design §9.2).
use serde::{Deserialize, Serialize};

/// What one program call may collect; a value it cuts says so in its `cut` row.
pub const PROGRAM_FUSE_BYTES: usize = 64 * 1024 * 1024;

/// One numbered line of a file, 1-based, as the file holds it with line endings removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub n: u64,
    pub text: String,
}

/// `read` of one file: the lines asked for, or the whole file when no window was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadValue {
    pub path: String,
    pub tag: String,
    pub lines: Vec<Line>,
    pub total_lines: u64,
    #[serde(default)]
    pub cut: Option<String>,
}

/// One `grep` hit: the file as the view names it, the 1-based line, the whole line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: String,
    pub line: u64,
    pub text: String,
}

/// Every hit `grep` collected, in walk order, past the model's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepValue {
    pub matches: Vec<GrepMatch>,
    pub files: u64,
    #[serde(default)]
    pub cut: Option<String>,
}

/// One entry of a directory `read` lists: a `dir` or a `file` with its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub bytes: Option<u64>,
}

/// A directory `read`: every entry the wall shows, dirs first, each group by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListValue {
    pub entries: Vec<Entry>,
}

/// A glob `read`: every file it matched, by the path the view names, sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobValue {
    pub paths: Vec<String>,
}

/// One file an `edit` section touched: its op (`updated`, `deleted`, `noop`), its tag after the
/// edit, which the next edit of it cites, and where a move sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditedFile {
    pub path: String,
    pub op: String,
    pub tag: String,
    #[serde(default)]
    pub first_changed_line: Option<u64>,
    #[serde(default)]
    pub moved_to: Option<String>,
}

/// An `edit`: every section in patch order, and the unified diff of what changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditValue {
    pub files: Vec<EditedFile>,
    pub patch: String,
}

/// A `write`: the path as given, the tag the next edit of it cites, and the bytes written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteValue {
    pub path: String,
    #[serde(default)]
    pub tag: Option<String>,
    pub bytes: u64,
}

/// A `bash` call: the streams as captured, before the reducer shortened them for the model, or
/// the job it became. `cut` names the cap and the file holding every byte when a stream passed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashValue {
    #[serde(default)]
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    #[serde(default)]
    pub cut: Option<String>,
    #[serde(default)]
    pub job: Option<u64>,
}

/// One layer of `get_context`: its body whole, or why it is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextLayer {
    pub name: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub absent: Option<String>,
}

/// `get_context`: every layer of the orientation packet, none clamped to the view's cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextValue {
    pub layers: Vec<ContextLayer>,
}

/// A child's `ask_user`: its parent's answer as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerValue {
    pub answer: String,
}

/// A `read` of an address: what the resolver served, and where the next page starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedValue {
    pub url: String,
    pub text: String,
    pub served_by: String,
    #[serde(default)]
    pub next_offset: Option<u64>,
}
