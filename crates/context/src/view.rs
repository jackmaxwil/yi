use std::collections::BTreeSet;

use yi_types::message::{AgentMessage, Content, UserContent};

use crate::audit::DROPPED_CAP;
use crate::details::{FileOps, compute_file_lists, extract_file_ops_from_message};
use crate::prompts::KERNEL_PERSIST_SUMMARY_NOTE;
use crate::wrapper::internal_source;

pub const BRIEF_LINE_CAP: usize = 120;
pub const BRIEF_LINE_CHARS: usize = 160;
pub const OUTSTANDING_CAP: usize = 12;
pub const FILE_CAP: usize = 32;
pub const EARLIER_CAP: usize = 24;

const OUTSTANDING_MARKERS: &[&str] = &["error", "Error", "FAIL", "failed", "panic", "exit 1"];
const DECISION_MARKERS: &[&str] = &["decided", "because", "must", "never", "instead"];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompiledView {
    pub files: Vec<String>,
    pub outstanding: Vec<String>,
    pub brief: Vec<String>,
    pub earlier: Vec<String>,
    pub dropped: Vec<(String, String)>,
}

impl CompiledView {
    pub fn render(&self) -> String {
        let mut sections = Vec::new();
        if !self.files.is_empty() {
            let mut block = String::from("[Files]");
            for path in &self.files {
                block.push_str("\n- ");
                block.push_str(path);
            }
            sections.push(block);
        }
        if !self.outstanding.is_empty() {
            let mut block = String::from("[Outstanding]");
            for line in &self.outstanding {
                block.push('\n');
                block.push_str(line);
            }
            sections.push(block);
        }
        if !self.brief.is_empty() {
            let mut block = String::from("[Brief]");
            for line in &self.brief {
                block.push('\n');
                block.push_str(line);
            }
            sections.push(block);
        }
        if !self.earlier.is_empty() {
            let mut block = String::from("[Earlier]");
            for line in &self.earlier {
                block.push('\n');
                block.push_str(line);
            }
            sections.push(block);
        }
        if !self.dropped.is_empty() {
            let mut block = String::from("[Dropped]");
            for (ident, id) in self.dropped.iter().take(DROPPED_CAP) {
                block.push_str("\n- ");
                block.push_str(ident);
                block.push_str(" (#");
                block.push_str(id);
                block.push(')');
            }
            sections.push(block);
        }
        {
            let mut block = String::from("[Kernel]");
            block.push('\n');
            block.push_str(KERNEL_PERSIST_SUMMARY_NOTE);
            sections.push(block);
        }
        format!(
            "<yi_compact_view>\n{}\n</yi_compact_view>",
            sections.join("\n")
        )
    }
}

pub fn compile_view(
    attributed: &[(String, AgentMessage)],
    previous_view: Option<&str>,
    file_ops: &FileOps,
) -> CompiledView {
    let mut files = files_from_ops(file_ops);
    if let Some(previous) = previous_view {
        files = union_capped(section_paths(previous), files, FILE_CAP);
    }

    let mut outstanding = Vec::new();
    let mut brief = Vec::new();
    let later_modified = later_modified_paths(attributed);
    for (index, (id, message)) in attributed.iter().enumerate() {
        if skip_message(message) {
            continue;
        }
        if let Some(line) = outstanding_line(message) {
            outstanding.push(line);
        }
        if let Some(line) = brief_line(id, message, &later_modified[index]) {
            brief.push(line);
        }
    }
    if outstanding.len() > OUTSTANDING_CAP {
        outstanding = outstanding.split_off(outstanding.len().saturating_sub(OUTSTANDING_CAP));
    }
    if let Some(previous) = previous_view {
        let mut rolled = previous_brief(previous);
        rolled.append(&mut brief);
        brief = rolled;
    }
    let mut earlier = previous_view.map(previous_earlier).unwrap_or_default();
    if brief.len() > BRIEF_LINE_CAP {
        let overflow = brief.len() - BRIEF_LINE_CAP;
        let demoted: Vec<String> = brief.drain(..overflow).collect();
        earlier.push(earlier_line(&demoted));
    }
    if earlier.len() > EARLIER_CAP {
        earlier = earlier.split_off(earlier.len().saturating_sub(EARLIER_CAP));
    }

    CompiledView {
        files,
        outstanding,
        brief,
        earlier,
        dropped: Vec::new(),
    }
}

fn skip_message(message: &AgentMessage) -> bool {
    internal_source(message).is_some()
        || matches!(
            message,
            AgentMessage::Custom { .. } | AgentMessage::CompactionSummary { .. }
        )
}

fn files_from_ops(file_ops: &FileOps) -> Vec<String> {
    let (read_files, modified_files) = compute_file_lists(file_ops);
    union_capped(modified_files, read_files, FILE_CAP)
}

fn union_capped(older: Vec<String>, newer: Vec<String>, cap: usize) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for path in older.into_iter().chain(newer) {
        if path.is_empty() || !seen.insert(path.clone()) {
            continue;
        }
        out.push(path);
    }
    if out.len() > cap {
        out.split_off(out.len().saturating_sub(cap))
    } else {
        out
    }
}

fn section_paths(text: &str) -> Vec<String> {
    section_lines(text, "[Files]")
        .into_iter()
        .filter_map(|line| line.strip_prefix("- ").map(str::to_owned))
        .collect()
}

fn previous_brief(text: &str) -> Vec<String> {
    section_lines(text, "[Brief]")
        .into_iter()
        .filter(|line| line.starts_with("(#"))
        .collect()
}

fn previous_earlier(text: &str) -> Vec<String> {
    section_lines(text, "[Earlier]")
}

fn earlier_line(lines: &[String]) -> String {
    let first = lines.first().and_then(|line| entry_id(line)).unwrap_or("?");
    let last = lines
        .last()
        .and_then(|line| entry_id(line))
        .unwrap_or(first);
    let user = lines
        .iter()
        .find_map(|line| user_brief_text(line))
        .unwrap_or("…");
    format!("{user} … (#{first}..#{last})")
}

fn entry_id(line: &str) -> Option<&str> {
    line.strip_prefix("(#")?.split(')').next()
}

fn user_brief_text(line: &str) -> Option<&str> {
    let rest = line.split_once(") user: ")?.1;
    Some(rest)
}

fn later_modified_paths(attributed: &[(String, AgentMessage)]) -> Vec<BTreeSet<String>> {
    let mut later = BTreeSet::new();
    let mut out = vec![BTreeSet::new(); attributed.len()];
    for (index, (_, message)) in attributed.iter().enumerate().rev() {
        out[index] = later.clone();
        let mut ops = FileOps::default();
        extract_file_ops_from_message(message, &mut ops);
        later.extend(ops.edited);
        later.extend(ops.written);
    }
    out
}

fn section_lines(text: &str, header: &str) -> Vec<String> {
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line == "</yi_compact_view>" {
            break;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line == header;
            continue;
        }
        if in_section && !line.is_empty() {
            lines.push(line.to_owned());
        }
    }
    lines
}

fn outstanding_line(message: &AgentMessage) -> Option<String> {
    let text = match message {
        AgentMessage::ToolResult {
            content,
            is_error,
            tool_name,
            ..
        } => {
            let body = content_text(content);
            if *is_error || is_outstanding(&body) {
                Some(format!("{tool_name}: {body}"))
            } else {
                None
            }
        }
        AgentMessage::BashExecution {
            command,
            output,
            exit_code,
            ..
        } => {
            if exit_code.is_some_and(|code| code != 0) || is_outstanding(output) {
                Some(format!("{command}: {output}"))
            } else {
                None
            }
        }
        _ => None,
    }?;
    Some(truncate(&one_line(&text), BRIEF_LINE_CHARS))
}

fn is_outstanding(text: &str) -> bool {
    OUTSTANDING_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
}

fn brief_line(
    id: &str,
    message: &AgentMessage,
    later_modified: &BTreeSet<String>,
) -> Option<String> {
    let body = match message {
        AgentMessage::User { content, .. } => {
            let text = user_text(content);
            if text.is_empty() {
                return None;
            }
            format!("user: {text}")
        }
        AgentMessage::Assistant { content, .. } => assistant_brief(content, later_modified)?,
        AgentMessage::ToolResult {
            tool_name,
            content,
            is_error,
            ..
        } => {
            let body = content_text(content);
            let n = body.chars().count();
            if *is_error || is_outstanding(&body) {
                format!("tool: {tool_name} {}", one_line(&body))
            } else {
                format!("tool: {tool_name} → {n} chars")
            }
        }
        AgentMessage::BashExecution {
            command,
            output,
            exit_code,
            ..
        } => {
            if exit_code.is_some_and(|code| code != 0) || is_outstanding(output) {
                format!("bash: {command} {}", one_line(output))
            } else {
                format!("bash: {command} → {} chars", output.chars().count())
            }
        }
        AgentMessage::BranchSummary { summary, .. } => format!("branch: {summary}"),
        _ => return None,
    };
    let line = format!("(#{id}) {}", one_line(&body));
    Some(truncate(&line, BRIEF_LINE_CHARS))
}

fn assistant_brief(content: &[Content], later_modified: &BTreeSet<String>) -> Option<String> {
    let mut reads = Vec::new();
    let mut edits = Vec::new();
    let mut tools = Vec::new();
    for block in content {
        let Content::ToolCall {
            name, arguments, ..
        } = block
        else {
            continue;
        };
        let path = arguments
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match name.as_str() {
            "read" if !path.is_empty() => {
                let stale = if later_modified.contains(path) {
                    " stale"
                } else {
                    ""
                };
                reads.push(format!("read {path}{stale}"));
            }
            "edit" | "write" if !path.is_empty() => edits.push(format!("edit {path}")),
            _ => {
                if let Some(line) = tool_call_brief(block) {
                    tools.push(line);
                }
            }
        }
    }
    if !reads.is_empty() || !edits.is_empty() {
        reads.extend(edits);
        return Some(reads.join("; "));
    }
    if !tools.is_empty() {
        return Some(format!("assistant: {}", tools.join("; ")));
    }
    let prose = content_text(content);
    if prose.is_empty() {
        return None;
    }
    Some(format!("assistant: {}", assistant_prose(&prose)))
}

fn assistant_prose(text: &str) -> String {
    let mut kept = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if DECISION_MARKERS.iter().any(|marker| lower.contains(marker)) {
            kept.push(one_line(line));
        }
    }
    if !kept.is_empty() {
        return kept.join(" ");
    }
    first_sentence(text)
}

fn first_sentence(text: &str) -> String {
    let flat = one_line(text);
    let mut out = String::new();
    let mut prev = '\0';
    for ch in flat.chars() {
        out.push(ch);
        if prev == '.' && ch == ' ' {
            break;
        }
        prev = ch;
    }
    truncate(&out, BRIEF_LINE_CHARS)
}

fn tool_call_brief(block: &Content) -> Option<String> {
    let Content::ToolCall {
        name, arguments, ..
    } = block
    else {
        return None;
    };
    let first = arguments
        .get("path")
        .or_else(|| arguments.get("command"))
        .or_else(|| arguments.values().find(|value| value.as_str().is_some()))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if first.is_empty() {
        Some(name.clone())
    } else {
        Some(format!("{name} {first}"))
    }
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => content_text(blocks),
    }
}

fn content_text(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if out.chars().count() >= max {
            break;
        }
        out.push(ch);
    }
    out
}
