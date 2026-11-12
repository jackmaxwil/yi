use std::borrow::{Borrow, Cow};
use std::collections::{HashMap, HashSet};

use yi_types::message::{
    AgentMessage, Attribution, Content, HostSource, StopReason, UNREAD_CONTENT, UserContent,
};
use yi_types::model::{Model, SYSTEM_BLOCK_SEPARATOR};

/// The assembled system prompt for a wire that takes one string: each block separator
/// becomes a paragraph break instead of reaching the model as a raw `\u{1d}` (D291).
pub fn system_text(prompt: &str) -> String {
    prompt
        .split(SYSTEM_BLOCK_SEPARATOR)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn is_vision(model: &Model) -> bool {
    model.input.iter().any(|kind| kind == "image")
}

fn downgrade_images(content: &[Content], placeholder: &str) -> Vec<Content> {
    let mut result = Vec::new();
    let mut previous_was_placeholder = false;
    for block in content {
        if matches!(block, Content::Image { .. }) {
            if !previous_was_placeholder {
                result.push(Content::Text {
                    text: placeholder.to_owned(),
                    text_signature: None,
                });
            }
            previous_was_placeholder = true;
            continue;
        }
        previous_was_placeholder =
            matches!(block, Content::Text { text, .. } if text == placeholder);
        result.push(block.clone());
    }
    result
}

fn has_unread(content: &[Content]) -> bool {
    content
        .iter()
        .any(|block| matches!(block, Content::Other(_)))
}

fn unread_text() -> Content {
    Content::Text {
        text: UNREAD_CONTENT.to_owned(),
        text_signature: None,
    }
}

fn mark_unread(content: Vec<Content>) -> Vec<Content> {
    let unread = |block| match block {
        Content::Other(_) => unread_text(),
        block => block,
    };
    content.into_iter().map(unread).collect()
}

pub fn normalize_anthropic_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// Cross-model replay: content downgrades, tool-call id normalization, dropped errored/aborted
/// assistants, a notice for each result left without its call, synthetic results for orphaned calls.
pub fn transform_messages<'a>(
    messages: &'a [AgentMessage],
    model: &Model,
    normalize_id: Option<fn(&str) -> String>,
) -> Vec<Cow<'a, AgentMessage>> {
    let _span = yi_types::trace::span("ai.transform_messages").arg("messages", messages.len());
    let vision = is_vision(model);
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut transformed: Vec<Cow<'a, AgentMessage>> = Vec::new();

    for message in messages {
        match message {
            AgentMessage::User {
                content: UserContent::Blocks(blocks),
                timestamp,
                attribution,
            } if !vision || has_unread(blocks) => {
                let blocks = if vision {
                    blocks.clone()
                } else {
                    downgrade_images(blocks, "(image omitted: model does not support images)")
                };
                transformed.push(Cow::Owned(AgentMessage::User {
                    content: UserContent::Blocks(mark_unread(blocks)),
                    timestamp: *timestamp,
                    attribution: *attribution,
                }));
            }
            AgentMessage::User { .. } => transformed.push(Cow::Borrowed(message)),
            AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                content,
                details,
                usage,
                added_tool_names,
                is_error,
                timestamp,
            } => {
                let mapped = id_map.get(tool_call_id);
                if vision && mapped.is_none() && !has_unread(content) {
                    transformed.push(Cow::Borrowed(message));
                    continue;
                }
                let content = mark_unread(if vision {
                    content.clone()
                } else {
                    downgrade_images(
                        content,
                        "(tool image omitted: model does not support images)",
                    )
                });
                transformed.push(Cow::Owned(AgentMessage::ToolResult {
                    tool_call_id: mapped.cloned().unwrap_or_else(|| tool_call_id.clone()),
                    tool_name: tool_name.clone(),
                    content,
                    details: details.clone(),
                    usage: usage.clone(),
                    added_tool_names: added_tool_names.clone(),
                    is_error: *is_error,
                    timestamp: *timestamp,
                }));
            }
            AgentMessage::Assistant {
                content,
                api,
                provider,
                model: message_model,
                stop_reason,
                ..
            } => {
                if *stop_reason == StopReason::Error || *stop_reason == StopReason::Aborted {
                    continue;
                }
                let same_model =
                    provider == &model.provider && api == &model.api && message_model == &model.id;
                let mut new_content: Vec<Content> = Vec::new();
                for block in content {
                    match block {
                        Content::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                            ..
                        } => {
                            if redacted == &Some(true) {
                                if same_model {
                                    new_content.push(block.clone());
                                }
                                continue;
                            }
                            let has_signature = thinking_signature
                                .as_deref()
                                .is_some_and(|signature| !signature.trim().is_empty());
                            if same_model && has_signature {
                                new_content.push(block.clone());
                                continue;
                            }
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if same_model {
                                new_content.push(block.clone());
                            } else {
                                new_content.push(Content::Text {
                                    text: thinking.clone(),
                                    text_signature: None,
                                });
                            }
                        }
                        Content::ToolCall { .. } if api == yi_types::message::CURSOR_API => {
                            continue;
                        }
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                            thought_signature,
                            namespace,
                        } => {
                            let thought_signature = if same_model {
                                thought_signature.clone()
                            } else {
                                None
                            };
                            let id = if !same_model {
                                if let Some(normalize) = normalize_id {
                                    let normalized = normalize(id);
                                    if normalized != *id {
                                        id_map.insert(id.clone(), normalized.clone());
                                    }
                                    normalized
                                } else {
                                    id.clone()
                                }
                            } else {
                                id.clone()
                            };
                            new_content.push(Content::ToolCall {
                                id,
                                name: name.clone(),
                                arguments: arguments.clone(),
                                thought_signature,
                                namespace: namespace.clone(),
                            });
                        }
                        Content::Other(_) => new_content.push(unread_text()),
                        other => new_content.push(other.clone()),
                    }
                }
                let mut kept = message.clone();
                if let AgentMessage::Assistant { content, .. } = &mut kept {
                    *content = new_content;
                }
                transformed.push(Cow::Owned(kept));
            }
            _ => {}
        }
    }

    let mut result: Vec<Cow<'a, AgentMessage>> = Vec::new();
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut seen_results: HashSet<String> = HashSet::new();
    let mut unmatched: Vec<Content> = Vec::new();
    let synthesize = |pending: &mut Vec<(String, String)>,
                      seen: &mut HashSet<String>,
                      unmatched: &mut Vec<Content>,
                      result: &mut Vec<Cow<'a, AgentMessage>>| {
        for (id, name) in pending.drain(..) {
            if seen.contains(&id) {
                continue;
            }
            result.push(Cow::Owned(AgentMessage::ToolResult {
                tool_call_id: id,
                tool_name: name,
                content: vec![Content::Text {
                    text: "No result provided".to_owned(),
                    text_signature: None,
                }],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: true,
                timestamp: 0,
            }));
        }
        seen.clear();
        if !unmatched.is_empty() {
            result.push(Cow::Owned(AgentMessage::User {
                content: UserContent::Blocks(std::mem::take(unmatched)),
                timestamp: 0,
                attribution: Attribution::Host(HostSource::Notice),
            }));
        }
    };

    for message in transformed {
        match message.as_ref() {
            AgentMessage::Assistant { content, .. } => {
                synthesize(&mut pending, &mut seen_results, &mut unmatched, &mut result);
                pending = content
                    .iter()
                    .filter_map(|block| match block {
                        Content::ToolCall { id, name, .. } => Some((id.clone(), name.clone())),
                        _ => None,
                    })
                    .collect();
                result.push(message);
            }
            AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                ..
            } if !pending.iter().any(|(id, _)| id == tool_call_id) => {
                unmatched.push(unmatched_notice(tool_name));
            }
            AgentMessage::ToolResult { tool_call_id, .. } => {
                seen_results.insert(tool_call_id.clone());
                result.push(message);
            }
            AgentMessage::User { .. } => {
                synthesize(&mut pending, &mut seen_results, &mut unmatched, &mut result);
                result.push(message);
            }
            _ => result.push(message),
        }
    }
    synthesize(&mut pending, &mut seen_results, &mut unmatched, &mut result);
    omit_refused_images(&mut result);
    if model.api == "anthropic-messages" || model.id.contains("claude") {
        keep_newest_images(&mut result);
    }
    result
}

/// Incident: every wire refuses a result whose call it was not sent, and dropping a failed reply
/// drops its calls; the output stays out, so tool text never reads as the user's or the model's.
fn unmatched_notice(name: &str) -> Content {
    Content::Text {
        text: format!(
            "[{name} result omitted: its call was in a reply that failed or was stopped, which this request leaves out; run the tool again for its output]"
        ),
        text_signature: None,
    }
}

// Invariant: Claude's vision limits (API docs, 2026-09). Past 20 images every image in the
// request must be at most 2000 px, which the host cannot measure, and a request is capped at
// 32 MB; resent history counts toward both, so the oldest images give way, newest first kept.
const CLAUDE_MAX_IMAGES: usize = 20;
const CLAUDE_MAX_IMAGE_CHARS: usize = 24_000_000;

fn blocks_of(message: &AgentMessage) -> &[Content] {
    match message {
        AgentMessage::User {
            content: UserContent::Blocks(blocks),
            ..
        }
        | AgentMessage::ToolResult {
            content: blocks, ..
        } => blocks,
        _ => &[],
    }
}

fn images_newest_first<M: Borrow<AgentMessage>>(messages: &[M]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    for (at, message) in messages.iter().enumerate().rev() {
        for (index, block) in blocks_of(message.borrow()).iter().enumerate().rev() {
            if matches!(block, Content::Image { .. }) {
                found.push((at, index));
            }
        }
    }
    found
}

fn image_at<'m>(
    messages: &'m [Cow<'_, AgentMessage>],
    at: usize,
    index: usize,
) -> Option<(&'m str, &'m str)> {
    match blocks_of(messages.get(at)?).get(index)? {
        Content::Image { data, mime_type } => Some((data, mime_type)),
        _ => None,
    }
}

/// Only a message that loses an image is copied; the rest stay borrowed from the history.
fn replace_with_text(
    messages: &mut [Cow<'_, AgentMessage>],
    at: usize,
    index: usize,
    text: String,
) {
    let blocks = match messages.get_mut(at).map(Cow::to_mut) {
        Some(AgentMessage::User {
            content: UserContent::Blocks(blocks),
            ..
        })
        | Some(AgentMessage::ToolResult {
            content: blocks, ..
        }) => blocks,
        _ => return,
    };
    if let Some(block) = blocks.get_mut(index) {
        *block = Content::Text {
            text,
            text_signature: None,
        };
    }
}

/// Incident: a provider refuses a malformed image on every later request (#860), and a kernel
/// header check cannot see a truncated body or a session saved before it existed.
fn omit_refused_images(messages: &mut [Cow<'_, AgentMessage>]) {
    for (at, index) in images_newest_first(messages) {
        let Some((data, mime_type)) = image_at(messages, at, index) else {
            continue;
        };
        let Some(defect) = yi_types::image::image_defect(mime_type, data) else {
            continue;
        };
        let text = format!(
            "[image omitted: {mime_type}, {} KB of base64; the image {defect}, which the provider refuses. {}]",
            data.len() / 1000,
            defect.remedy()
        );
        replace_with_text(messages, at, index, text);
    }
}

fn keep_newest_images(messages: &mut [Cow<'_, AgentMessage>]) {
    let (mut kept, mut chars) = (0usize, 0usize);
    for (at, index) in images_newest_first(messages) {
        let Some((data, mime_type)) = image_at(messages, at, index) else {
            continue;
        };
        let total = chars.saturating_add(data.len());
        if kept < CLAUDE_MAX_IMAGES && total <= CLAUDE_MAX_IMAGE_CHARS {
            (kept, chars) = (kept.saturating_add(1), total);
            continue;
        }
        let text = format!(
            "(earlier image omitted: {mime_type}, {} KB of base64; the request keeps its newest {CLAUDE_MAX_IMAGES})",
            data.len() / 1000
        );
        replace_with_text(messages, at, index, text);
    }
}
