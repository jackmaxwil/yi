use std::collections::BTreeSet;

use yi_types::message::{AgentMessage, Content};

use crate::audit::DROPPED_CAP;
use crate::details::extract_file_ops_from_message;
use crate::prompts::KERNEL_PERSIST_SUMMARY_NOTE;
use crate::wrapper::internal_source;

pub const BRIEF_LINE_CAP: usize = 120;
pub const BRIEF_LINE_CHARS: usize = 160;
pub const OUTSTANDING_CAP: usize = 12;
pub const EARLIER_CAP: usize = 24;

const OUTSTANDING_MARKERS: &[&str] = &["error", "Error", "FAIL", "failed", "panic", "exit 1"];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompiledView {
    pub outstanding: Vec<String>,
    pub brief: Vec<String>,
    pub earlier: Vec<String>,
    pub dropped: Vec<(String, String)>,
}

impl CompiledView {
    pub fn render(&self) -> String {
        let mut sections = Vec::new();
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
) -> CompiledView {
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
    format!("(#{first}..#{last})")
}

fn entry_id(line: &str) -> Option<&str> {
    line.strip_prefix("(#")?.split(')').next()
}

fn later_modified_paths(attributed: &[(String, AgentMessage)]) -> Vec<BTreeSet<String>> {
    let mut later = BTreeSet::new();
    let mut out = vec![BTreeSet::new(); attributed.len()];
    for (index, (_, message)) in attributed.iter().enumerate().rev() {
        out[index] = later.clone();
        let mut ops = crate::details::FileOps::default();
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
        AgentMessage::User { .. } => return None,
        AgentMessage::Assistant { content, .. } => assistant_tools(content, later_modified)?,
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
    Some(format!("(#{id}) {}", one_line(&body)))
}

fn assistant_tools(content: &[Content], later_modified: &BTreeSet<String>) -> Option<String> {
    let mut parts = Vec::new();
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
                parts.push(format!("read {path}{stale}"));
            }
            "edit" | "write" if !path.is_empty() => parts.push(format!("edit {path}")),
            _ => {
                if let Some(line) = tool_call_brief(block) {
                    parts.push(line);
                }
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
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
