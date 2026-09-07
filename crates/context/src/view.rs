use std::collections::BTreeMap;

use yi_types::message::{AgentMessage, Content};

use crate::details::extract_file_ops_from_message;
use crate::prompts::KERNEL_PERSIST_SUMMARY_NOTE;
use crate::serialize::text_of;
use crate::wrapper::internal_source;

pub const BRIEF_LINE_CAP: usize = 120;
pub const BRIEF_LINE_CHARS: usize = 160;
pub const OUTSTANDING_CAP: usize = 12;
pub const EARLIER_CAP: usize = 24;

#[derive(Debug, Clone, PartialEq)]
pub struct Attributed {
    pub id: Option<String>,
    pub message: AgentMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BriefLine {
    pub id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompiledView {
    pub outstanding: Vec<String>,
    pub brief: Vec<BriefLine>,
    pub earlier: Vec<String>,
}

impl BriefLine {
    pub fn render(&self) -> String {
        match &self.id {
            Some(id) => format!("(#{id}) {}", self.text),
            None => self.text.clone(),
        }
    }
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
                block.push_str(&line.render());
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

pub fn compile_view(attributed: &[Attributed], previous: Option<&CompiledView>) -> CompiledView {
    let mut outstanding = Vec::new();
    let mut brief = Vec::new();
    let first_edit = first_edit_index(attributed);
    for (index, entry) in attributed.iter().enumerate() {
        if skip_message(&entry.message) {
            continue;
        }
        if let Some(line) = outstanding_line(&entry.message) {
            outstanding.push(line);
        }
        if let Some(text) = brief_text(&entry.message, &first_edit, index) {
            brief.push(BriefLine {
                id: entry.id.clone(),
                text,
            });
        }
    }
    if outstanding.len() > OUTSTANDING_CAP {
        outstanding = outstanding.split_off(outstanding.len().saturating_sub(OUTSTANDING_CAP));
    }
    let mut earlier = Vec::new();
    if let Some(previous) = previous {
        let mut rolled = previous.brief.clone();
        rolled.append(&mut brief);
        brief = rolled;
        earlier = previous.earlier.clone();
    }
    if brief.len() > BRIEF_LINE_CAP {
        let overflow = brief.len().saturating_sub(BRIEF_LINE_CAP);
        let demoted: Vec<BriefLine> = brief.drain(..overflow).collect();
        earlier.push(earlier_line(&demoted));
    }
    if earlier.len() > EARLIER_CAP {
        earlier = earlier.split_off(earlier.len().saturating_sub(EARLIER_CAP));
    }

    CompiledView {
        outstanding,
        brief,
        earlier,
    }
}

fn skip_message(message: &AgentMessage) -> bool {
    internal_source(message).is_some()
        || matches!(
            message,
            AgentMessage::Custom { .. } | AgentMessage::CompactionSummary { .. }
        )
}

fn earlier_line(lines: &[BriefLine]) -> String {
    let ids = || lines.iter().filter_map(|line| line.id.as_deref());
    let first = ids().next().unwrap_or("?");
    let last = ids().next_back().unwrap_or(first);
    format!("(#{first}..#{last})")
}

fn first_edit_index(attributed: &[Attributed]) -> BTreeMap<String, usize> {
    let mut first = BTreeMap::new();
    for (index, entry) in attributed.iter().enumerate() {
        let mut ops = crate::details::FileOps::default();
        extract_file_ops_from_message(&entry.message, &mut ops);
        for path in ops.edited.into_iter().chain(ops.written) {
            first.entry(path).or_insert(index);
        }
    }
    first
}

fn outstanding_line(message: &AgentMessage) -> Option<String> {
    let text = match message {
        AgentMessage::ToolResult {
            content,
            is_error,
            tool_name,
            ..
        } if *is_error => Some(format!("{tool_name}: {}", text_of(content))),
        AgentMessage::BashExecution {
            command,
            output,
            exit_code,
            ..
        } if exit_code.is_some_and(|code| code != 0) => Some(format!("{command}: {output}")),
        _ => None,
    }?;
    Some(truncate(&one_line(&text), BRIEF_LINE_CHARS))
}

fn brief_text(
    message: &AgentMessage,
    first_edit: &BTreeMap<String, usize>,
    index: usize,
) -> Option<String> {
    let body = match message {
        AgentMessage::User { .. } => return None,
        AgentMessage::Assistant { content, .. } => assistant_tools(content, first_edit, index)?,
        AgentMessage::ToolResult {
            tool_name,
            content,
            is_error,
            ..
        } => {
            let body = text_of(content);
            if *is_error {
                format!(
                    "tool: {tool_name} {}",
                    truncate(&one_line(&body), BRIEF_LINE_CHARS)
                )
            } else {
                format!("tool: {tool_name} → {} chars", body.chars().count())
            }
        }
        AgentMessage::BashExecution {
            command,
            output,
            exit_code,
            ..
        } => {
            if exit_code.is_some_and(|code| code != 0) {
                format!(
                    "bash: {command} {}",
                    truncate(&one_line(output), BRIEF_LINE_CHARS)
                )
            } else {
                format!("bash: {command} → {} chars", output.chars().count())
            }
        }
        AgentMessage::BranchSummary { summary, .. } => format!("earlier attempt: {summary}"),
        _ => return None,
    };
    Some(one_line(&body))
}

fn assistant_tools(
    content: &[Content],
    first_edit: &BTreeMap<String, usize>,
    index: usize,
) -> Option<String> {
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
                let stale = if first_edit.get(path).is_some_and(|first| *first > index) {
                    " stale"
                } else {
                    ""
                };
                parts.push(format!("read {path}{stale}"));
            }
            "edit" | "write" if !path.is_empty() => parts.push(format!("edit {path}")),
            _ => parts.push(tool_call_brief(name, arguments)),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

// Only `path` and `command` name a call: the map's "first string argument" was
// order-dependent, so the same call briefed differently between runs.
fn tool_call_brief(name: &str, arguments: &serde_json::Map<String, serde_json::Value>) -> String {
    let first = arguments
        .get("path")
        .or_else(|| arguments.get("command"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if first.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {first}")
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

pub fn view_extra(view: &CompiledView) -> Vec<(String, serde_json::Value)> {
    let brief: Vec<serde_json::Value> = view
        .brief
        .iter()
        .map(|line| serde_json::json!({"id": line.id, "text": line.text}))
        .collect();
    vec![
        ("brief".to_owned(), serde_json::Value::Array(brief)),
        ("earlier".to_owned(), serde_json::json!(view.earlier)),
    ]
}

pub fn view_from_extra(extra: &serde_json::Map<String, serde_json::Value>) -> Option<CompiledView> {
    let brief = extra.get("brief")?.as_array()?;
    let earlier = extra
        .get("earlier")
        .and_then(|value| value.as_array())
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Some(CompiledView {
        outstanding: Vec::new(),
        brief: brief
            .iter()
            .filter_map(|line| {
                Some(BriefLine {
                    id: line.get("id").and_then(|id| id.as_str()).map(str::to_owned),
                    text: line.get("text")?.as_str()?.to_owned(),
                })
            })
            .collect(),
        earlier,
    })
}
