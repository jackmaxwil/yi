use std::collections::BTreeSet;

use serde_json::{Map, Value};
use yi_tools::hashline::input::Patch;
use yi_types::compaction::CompactionDetails;
use yi_types::kernel::{NESTED_CALLS, NESTED_READS_DROPPED, NESTED_READS_KEPT, NestedCall};
use yi_types::message::{AgentMessage, Content};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileOps {
    pub read: BTreeSet<String>,
    pub written: BTreeSet<String>,
    pub edited: BTreeSet<String>,
    pub reads_dropped: u64,
}

/// An edit carries no `path`: its files are the `[path#TAG]` section headers of its `patch`.
pub fn call_paths(name: &str, arguments: &Map<String, Value>) -> Vec<String> {
    let argument = |key| arguments.get(key).and_then(Value::as_str);
    match name {
        "read" | "write" => argument("path").map(str::to_owned).into_iter().collect(),
        "edit" => (Patch::parse(argument("patch").unwrap_or(""), None).into_iter())
            .flat_map(|patch| patch.sections)
            .map(|section| section.path)
            .collect(),
        _ => Vec::new(),
    }
}

fn note_paths(name: &str, paths: impl IntoIterator<Item = String>, file_ops: &mut FileOps) {
    let set = match name {
        "read" => &mut file_ops.read,
        "write" => &mut file_ops.written,
        "edit" => &mut file_ops.edited,
        _ => return,
    };
    set.extend(paths);
}

/// A cell's own calls ride its `ipython` result: a program's edit is still an edit.
pub fn extract_file_ops_from_message(message: &AgentMessage, file_ops: &mut FileOps) {
    match message {
        AgentMessage::Assistant { content, .. } => {
            for block in content {
                if let Content::ToolCall {
                    name, arguments, ..
                } = block
                {
                    note_paths(name, call_paths(name, arguments), file_ops);
                }
            }
        }
        AgentMessage::ToolResult { details, .. } => {
            for call in nested_calls(details.as_ref()) {
                note_paths(&call.tool, call.paths, file_ops);
            }
            let dropped =
                (details.as_ref()).and_then(|details| details.get(NESTED_READS_DROPPED)?.as_u64());
            file_ops.reads_dropped = file_ops.reads_dropped.saturating_add(dropped.unwrap_or(0));
        }
        _ => {}
    }
}

/// The calls a cell made through the host, as its `ipython` result lists them.
pub fn nested_calls(details: Option<&Value>) -> Vec<NestedCall> {
    (details
        .and_then(|details| details.get(NESTED_CALLS))
        .cloned())
    .and_then(|calls| serde_json::from_value(calls).ok())
    .unwrap_or_default()
}

/// Design §4.4: file lists are cumulative across compactions — the previous
/// entry's details seed the next extraction.
pub fn extract_file_ops(
    messages: &[AgentMessage],
    previous: Option<&CompactionDetails>,
) -> FileOps {
    let mut file_ops = FileOps::default();
    if let Some(details) = previous {
        file_ops.read.extend(details.read_files.iter().cloned());
        file_ops
            .edited
            .extend(details.modified_files.iter().cloned());
        let dropped = details
            .extra
            .get(NESTED_READS_DROPPED)
            .and_then(Value::as_u64);
        file_ops.reads_dropped = dropped.unwrap_or(0);
    }
    for message in messages {
        extract_file_ops_from_message(message, &mut file_ops);
    }
    file_ops
}

pub fn compute_file_lists(file_ops: &FileOps) -> (Vec<String>, Vec<String>) {
    let modified: BTreeSet<&String> = file_ops.edited.union(&file_ops.written).collect();
    let read_files = file_ops
        .read
        .iter()
        .filter(|path| !modified.contains(path))
        .cloned()
        .collect();
    let modified_files = modified.into_iter().cloned().collect();
    (read_files, modified_files)
}

pub fn format_file_operations(
    read_files: &[String],
    modified_files: &[String],
    reads_dropped: u64,
) -> String {
    let mut sections = Vec::new();
    let mut read_rows = read_files.to_vec();
    if reads_dropped > 0 {
        read_rows.push(format!(
            "[… {} read files listed; {reads_dropped} more read calls from cells are not, and their paths were not kept: a cell keeps {NESTED_READS_KEPT} distinct read rows (NESTED_READS_KEPT); the cells that made them: grep(pattern=\"yi.tools\", path=\"history://self\")]",
            read_files.len()
        ));
    }
    if !read_rows.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_rows.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}
