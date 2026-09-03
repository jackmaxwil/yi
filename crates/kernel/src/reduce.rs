use serde_json::Value;
use yi_types::kernel::{
    ExecuteResult, ExecuteStatus, JupyterMessage, KernelAttachment, KernelDiffDisplay, KernelError,
    KernelSentAgentMessage,
};

use crate::{
    AGENT_MESSAGE_DISPLAY_MIME, ATTACHMENT_DISPLAY_MIME, DIFF_DISPLAY_MIME,
    MAX_ATTACHMENT_DATA_CHARS,
};

#[derive(Debug)]
pub struct CellState {
    pub request_msg_id: String,
    pub code: String,
    pub max_chars: usize,
    pub internal: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub result: Option<String>,
    pub diffs: Vec<KernelDiffDisplay>,
    pub attachments: Vec<KernelAttachment>,
    pub sent_agent_messages: Vec<KernelSentAgentMessage>,
    pub status: ExecuteStatus,
    pub error: Option<KernelError>,
}

impl CellState {
    pub fn new(request_msg_id: String, code: String, max_chars: usize, internal: bool) -> Self {
        Self {
            request_msg_id,
            code,
            max_chars,
            internal,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            result: None,
            diffs: Vec::new(),
            attachments: Vec::new(),
            sent_agent_messages: Vec::new(),
            status: ExecuteStatus::Ok,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reduction {
    Continue,
    Done,
}

pub struct StreamChunk<'a> {
    pub name: &'a str,
    pub text: &'a str,
}

fn append_capped(target: &mut String, truncated: &mut bool, text: &str, max_chars: usize) {
    if target.chars().count() >= max_chars {
        return;
    }
    target.push_str(text);
    let count = target.chars().count();
    if count > max_chars {
        *target = target.chars().take(max_chars).collect();
        *truncated = true;
    }
}

pub fn parse_diff_display(payload: &Value) -> Option<KernelDiffDisplay> {
    serde_json::from_value(payload.clone()).ok()
}

pub enum AttachmentParse {
    Attachment(KernelAttachment),
    Oversized,
    Malformed,
}

pub fn parse_attachment_display(payload: &Value) -> AttachmentParse {
    let Ok(attachment) = serde_json::from_value::<KernelAttachment>(payload.clone()) else {
        return AttachmentParse::Malformed;
    };
    if attachment.data.len() > MAX_ATTACHMENT_DATA_CHARS {
        return AttachmentParse::Oversized;
    }
    AttachmentParse::Attachment(attachment)
}

pub fn parse_sent_agent_message(payload: &Value) -> Option<KernelSentAgentMessage> {
    let message: KernelSentAgentMessage = serde_json::from_value(payload.clone()).ok()?;
    matches!(message.delivery_status.as_str(), "delivered" | "queued").then_some(message)
}

pub fn parent_msg_id(message: &JupyterMessage) -> Option<&str> {
    message.parent_header.get("msg_id").and_then(Value::as_str)
}

/// Fold one iopub message into the active cell (design K6). `on_stream` sees uncapped chunks:
/// the UI gets everything, only the model-facing capture is capped.
pub fn reduce(
    cell: &mut CellState,
    message: &JupyterMessage,
    mut on_stream: Option<&mut dyn FnMut(StreamChunk<'_>)>,
) -> Reduction {
    match message.header.msg_type.as_str() {
        "stream" => {
            let name = message
                .content
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let text = message
                .content
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match name {
                "stdout" => append_capped(
                    &mut cell.stdout,
                    &mut cell.stdout_truncated,
                    text,
                    cell.max_chars,
                ),
                "stderr" => append_capped(
                    &mut cell.stderr,
                    &mut cell.stderr_truncated,
                    text,
                    cell.max_chars,
                ),
                _ => {}
            }
            if let Some(on_stream) = on_stream.as_mut() {
                on_stream(StreamChunk { name, text });
            }
            Reduction::Continue
        }
        "execute_result" => {
            if let Some(text) = message
                .content
                .get("data")
                .and_then(|data| data.get("text/plain"))
                .and_then(Value::as_str)
            {
                cell.result = Some(text.to_owned());
            }
            Reduction::Continue
        }
        "display_data" | "update_display_data" => {
            let data = message.content.get("data");
            if let Some(diff) = data
                .and_then(|data| data.get(DIFF_DISPLAY_MIME))
                .and_then(parse_diff_display)
            {
                cell.diffs.push(diff);
            }
            if let Some(payload) = data.and_then(|data| data.get(ATTACHMENT_DISPLAY_MIME)) {
                match parse_attachment_display(payload) {
                    AttachmentParse::Attachment(attachment) => cell.attachments.push(attachment),
                    AttachmentParse::Oversized => {
                        // A well-formed oversized attachment fails the cell loudly,
                        // never a silent image drop.
                        if !cell.stderr.is_empty() {
                            cell.stderr.push('\n');
                        }
                        cell.stderr.push_str(&format!(
                            "attachment dropped: exceeds {MAX_ATTACHMENT_DATA_CHARS} base64 chars"
                        ));
                        cell.status = ExecuteStatus::Error;
                    }
                    AttachmentParse::Malformed => {}
                }
            }
            if let Some(sent) = data
                .and_then(|data| data.get(AGENT_MESSAGE_DISPLAY_MIME))
                .and_then(parse_sent_agent_message)
            {
                cell.sent_agent_messages.push(sent);
            }
            Reduction::Continue
        }
        "error" => {
            let content = Value::Object(message.content.clone());
            if let Ok(error) = serde_json::from_value::<KernelError>(content) {
                cell.error = Some(error);
            }
            cell.status = ExecuteStatus::Error;
            Reduction::Continue
        }
        "status" => {
            let idle = message
                .content
                .get("execution_state")
                .and_then(Value::as_str)
                == Some("idle");
            if idle {
                Reduction::Done
            } else {
                Reduction::Continue
            }
        }
        _ => Reduction::Continue,
    }
}

pub fn finish(cell: CellState, duration_ms: u64, aborted: bool) -> ExecuteResult {
    let max_chars = cell.max_chars;
    let truncation = format!("\n[... output truncated at {max_chars} chars ...]");
    let mut stdout = cell.stdout;
    let mut stderr = cell.stderr;
    if cell.stdout_truncated {
        stdout.push_str(&truncation);
    }
    if cell.stderr_truncated {
        stderr.push_str(&truncation);
    }
    let result = cell.result.map(|result| {
        if result.chars().count() > max_chars {
            let capped: String = result.chars().take(max_chars).collect();
            format!("{capped}{truncation}")
        } else {
            result
        }
    });
    let status = if aborted {
        ExecuteStatus::Aborted
    } else {
        cell.status
    };
    ExecuteResult {
        stdout,
        stderr,
        result,
        diffs: cell.diffs,
        attachments: cell.attachments,
        sent_agent_messages: cell.sent_agent_messages,
        status,
        error: cell.error,
        duration_ms,
    }
}
