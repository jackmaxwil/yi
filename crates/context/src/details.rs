use std::collections::BTreeSet;

use yi_types::compaction::CompactionDetails;
use yi_types::message::{AgentMessage, Content};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileOps {
    pub read: BTreeSet<String>,
    pub written: BTreeSet<String>,
    pub edited: BTreeSet<String>,
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
        let Some(path) = arguments.get("path").and_then(|value| value.as_str()) else {
            continue;
        };
        match name.as_str() {
            "read" => {
                file_ops.read.insert(path.to_owned());
            }
            "write" => {
                file_ops.written.insert(path.to_owned());
            }
            "edit" => {
                file_ops.edited.insert(path.to_owned());
            }
            _ => {}
        }
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
