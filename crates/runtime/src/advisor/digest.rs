use yi_types::message::{AgentMessage, Content, UserContent};

pub const DEFAULT_USER_BUDGET: usize = 2_000;
pub const DEFAULT_PROSE_BUDGET: usize = 1_200;

/// §7.4 verb tables: the sentences a reviewer needs to see survive the prose
/// budget, the rest are dropped.
pub const CLAIM_VERBS: [&str; 10] = [
    "ran",
    "tested",
    "verified",
    "edited",
    "created",
    "fixed",
    "passes",
    "passed",
    "all green",
    "green",
];
pub const COMMITMENT_VERBS: [&str; 4] = ["will", "next", "then", "instead"];
pub const CONCLUSION_VERBS: [&str; 3] = ["because", "so", "root cause"];

pub fn assistant_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn split_sentences(text: &str) -> Vec<&str> {
    text.split_inclusive(['.', '!', '?', '\n'])
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .collect()
}

// §7.6 constraint markers: sentences carrying these survive truncation first.
const CONSTRAINT_MARKERS: [&str; 10] = [
    "never", "don't", "do not", "no ", "not ", "only", "must", "instead", "stop", "wait",
];

pub struct LogItem<'a> {
    pub id: &'a str,
    pub message: &'a AgentMessage,
}

fn is_constraint(sentence: &str) -> bool {
    let lowered = sentence.to_lowercase();
    CONSTRAINT_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
        || lowered.contains("actually")
}

/// User prose is ground truth, so it stays verbatim; over budget the
/// constraint-carrying sentences survive first, then recency, and an elided
/// span keeps the entry id as the pull handle.
pub fn truncate_user_text(text: &str, budget: usize, id: &str) -> String {
    if text.len() <= budget {
        return text.to_owned();
    }
    let sentences = split_sentences(text);
    let mut keep = vec![false; sentences.len()];
    let mut used = 0_usize;
    for (index, sentence) in sentences.iter().enumerate() {
        if is_constraint(sentence) && used.saturating_add(sentence.len()) <= budget {
            keep[index] = true;
            used = used.saturating_add(sentence.len().saturating_add(1));
        }
    }
    for (index, sentence) in sentences.iter().enumerate().rev() {
        if !keep[index] && used.saturating_add(sentence.len()) <= budget {
            keep[index] = true;
            used = used.saturating_add(sentence.len().saturating_add(1));
        }
    }
    let mut out = String::new();
    let mut elided = false;
    for (index, sentence) in sentences.iter().enumerate() {
        if keep[index] {
            if elided {
                out.push_str(&format!("[…elided, pull {id}] "));
                elided = false;
            }
            out.push_str(sentence);
            out.push(' ');
        } else {
            elided = true;
        }
    }
    if elided {
        out.push_str(&format!("[…elided, pull {id}]"));
    }
    out.trim().to_owned()
}

/// The intent trace: claims, commitments, conclusions, plus each block's first
/// and last sentence, capped.
pub fn select_assistant_text(text: &str, budget: usize) -> String {
    let sentences = split_sentences(text);
    if sentences.is_empty() {
        return String::new();
    }
    let table: Vec<&str> = CLAIM_VERBS
        .iter()
        .chain(COMMITMENT_VERBS.iter())
        .chain(CONCLUSION_VERBS.iter())
        .copied()
        .collect();
    let mut out = String::new();
    let last_index = sentences.len().saturating_sub(1);
    for (index, sentence) in sentences.iter().enumerate() {
        let lowered = sentence.to_lowercase();
        let selected =
            index == 0 || index == last_index || table.iter().any(|verb| lowered.contains(verb));
        if !selected {
            continue;
        }
        if out.len().saturating_add(sentence.len()) > budget {
            break;
        }
        out.push_str(sentence);
        out.push(' ');
    }
    out.trim().to_owned()
}

/// The user's words, never a paraphrase; append-only.
pub fn directives(text: &str, id: &str) -> Vec<String> {
    split_sentences(text)
        .into_iter()
        .filter(|sentence| is_constraint(sentence))
        .map(|sentence| format!("{id}: {sentence}"))
        .collect()
}

/// One entry-id'd line per work-log item; never thinking.
pub fn digest_line(item: &LogItem<'_>, user_budget: usize, prose_budget: usize) -> Option<String> {
    match item.message {
        AgentMessage::User { content, .. } => {
            let text = match content {
                UserContent::Text(text) => text.clone(),
                UserContent::Blocks(_) => return None,
            };
            Some(format!(
                "{} user: {}",
                item.id,
                truncate_user_text(&text, user_budget, item.id)
            ))
        }
        AgentMessage::Assistant { content, .. } => {
            let mut lines = Vec::new();
            for block in content {
                match block {
                    Content::ToolCall {
                        name, arguments, ..
                    } => {
                        let summary = tool_call_summary(name, arguments);
                        lines.push(format!("{} {name} {summary}", item.id));
                    }
                    Content::Text { text, .. } if !text.trim().is_empty() => {
                        let selected = select_assistant_text(text, prose_budget);
                        if !selected.is_empty() {
                            lines.push(format!("{} assistant: {selected}", item.id));
                        }
                    }
                    _ => {}
                }
            }
            (!lines.is_empty()).then(|| lines.join("\n"))
        }
        AgentMessage::ToolResult {
            tool_name,
            content,
            is_error,
            ..
        } => {
            let text: String = content
                .iter()
                .filter_map(|block| match block {
                    Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            let tail: String = text
                .chars()
                .rev()
                .take(160)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Some(format!(
                "{} {tool_name} -> {} | tail: {}",
                item.id,
                if *is_error { "error" } else { "ok" },
                tail.trim()
            ))
        }
        _ => None,
    }
}

fn tool_call_summary(name: &str, arguments: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut parts = Vec::new();
    for key in ["path", "command", "code", "pattern", "i"] {
        if let Some(value) = arguments.get(key).and_then(serde_json::Value::as_str) {
            let capped: String = value.chars().take(120).collect();
            parts.push(format!("{key}={capped}"));
        }
    }
    if parts.is_empty() {
        format!("({} args)", arguments.len())
    } else {
        let _ = name;
        parts.join(" ")
    }
}
