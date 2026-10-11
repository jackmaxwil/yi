use std::collections::BTreeSet;

use serde_json::{Map, Value};
use yi_tools::hashline::input::Patch;
use yi_types::compaction::CompactionDetails;
use yi_types::message::{AgentMessage, Content};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileOps {
    pub read: BTreeSet<String>,
    pub written: BTreeSet<String>,
    pub edited: BTreeSet<String>,
}

/// An edit carries no `path`: its files are the `[path#TAG]` section headers of its `patch`.
pub(crate) fn call_paths(name: &str, arguments: &Map<String, Value>) -> Vec<String> {
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

pub fn extract_file_ops_from_message(message: &AgentMessage, file_ops: &mut FileOps) {
    let AgentMessage::Assistant { content, .. } = message else {
        return;
    };
    for block in content {
        let Content::ToolCall {
            name, arguments, ..
        } = block
        else {
            continue;
        };
        let set = match name.as_str() {
            "read" => &mut file_ops.read,
            "write" => &mut file_ops.written,
            "edit" => &mut file_ops.edited,
            _ => continue,
        };
        set.extend(call_paths(name, arguments));
    }
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

pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
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
