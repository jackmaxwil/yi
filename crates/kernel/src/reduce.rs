use serde_json::Value;
use yi_types::image::{has_magic_number, strict_base64_head};
use yi_types::kernel::{
    ExecuteResult, ExecuteStatus, JupyterMessage, KernelAttachment, KernelError,
};

use crate::{ATTACHMENT_DISPLAY_MIME, MAX_ATTACHMENT_DATA_CHARS};

/// The display bundles a value can arrive as, richest first: a notebook renders the one it kept.
const RESULT_MIMES: [&str; 3] = ["text/markdown", "application/json", "text/x-python"];
const PLAIN_MIME: &str = "text/plain";

#[derive(Debug)]
pub struct CellState {
    pub request_msg_id: String,
    pub max_chars: usize,
    pub internal: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_chars: usize,
    pub stderr_chars: usize,
    pub result: Option<String>,
    pub result_mime: Option<String>,
    pub attachments: Vec<KernelAttachment>,
    pub status: ExecuteStatus,
    pub error: Option<KernelError>,
}

impl CellState {
    pub fn new(request_msg_id: String, max_chars: usize, internal: bool) -> Self {
        Self {
            request_msg_id,
            max_chars,
            internal,
            stdout: String::new(),
            stderr: String::new(),
            stdout_chars: 0,
            stderr_chars: 0,
            result: None,
            result_mime: None,
            attachments: Vec::new(),
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

fn append_capped(target: &mut String, seen: &mut usize, text: &str, max_chars: usize) {
    *seen = seen.saturating_add(text.chars().count());
    target.push_str(text);
    // Invariant: a char is at most 4 bytes, so past 8x the cap a trim always cuts and runs at
    // most once per 4x the cap of new bytes; it holds at most 8x the cap in bytes plus one chunk.
    if target.len() > max_chars.saturating_mul(8) {
        let (head, tail) = ends(target, max_chars);
        *target = head + &tail;
    }
}

fn ends(text: &str, max_chars: usize) -> (String, String) {
    let half = max_chars / 2;
    let count = text.chars().count();
    let skip = count
        .saturating_sub(max_chars.saturating_sub(half))
        .max(half);
    (
        text.chars().take(half).collect(),
        text.chars().skip(skip).collect(),
    )
}

fn keep_ends(text: String, seen: usize, max_chars: usize) -> String {
    if seen <= max_chars {
        return text;
    }
    let (head, tail) = ends(&text, max_chars);
    let kept = head.chars().count().saturating_add(tail.chars().count());
    format!(
        "{head}\n[\u{2026} {kept} of {seen} chars kept, the middle cut (DEFAULT_MAX_OUTPUT_CHARS {max_chars}); the rest: read the full output file named below, or with none rerun the cell writing it to a file and read that]\n{tail}"
    )
}

pub enum AttachmentParse {
    Attachment(KernelAttachment),
    /// Well-formed but refused; the reason is what the cell's stderr tells the model.
    Rejected(String),
    Malformed,
}

pub fn parse_attachment_display(payload: &Value) -> AttachmentParse {
    let Ok(attachment) = serde_json::from_value::<KernelAttachment>(payload.clone()) else {
        return AttachmentParse::Malformed;
    };
    if attachment.data.len() > MAX_ATTACHMENT_DATA_CHARS {
        return AttachmentParse::Rejected(format!(
            "exceeds {MAX_ATTACHMENT_DATA_CHARS} base64 chars"
        ));
    }
    // Invariant (#817): `data` goes as is into a kitty escape and to the provider; a byte outside
    // base64 could end the escape. The body is checked where each request is built (#860).
    let image = strict_base64_head(&attachment.data)
        .is_some_and(|head| has_magic_number(&attachment.mime_type, &head));
    if !image {
        return AttachmentParse::Rejected(
            "its data is not base64 of the image its mime_type names".to_owned(),
        );
    }
    AttachmentParse::Attachment(attachment)
}

pub fn parent_msg_id(message: &JupyterMessage) -> Option<&str> {
    message.parent_header.get("msg_id").and_then(Value::as_str)
}

/// Fold one iopub message into the active cell (design §9). `on_stream` sees every stream chunk
/// and the displayed value (named `execute_result`) uncapped; only the model's capture is capped.
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
                    &mut cell.stdout_chars,
                    text,
                    cell.max_chars,
                ),
                "stderr" => append_capped(
                    &mut cell.stderr,
                    &mut cell.stderr_chars,
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
            let data = message.content.get("data");
            let picked = data.and_then(|data| {
                RESULT_MIMES
                    .iter()
                    .find_map(|mime| data.get(*mime).map(|value| (*mime, value)))
                    .or_else(|| data.get(PLAIN_MIME).map(|value| (PLAIN_MIME, value)))
            });
            if let Some((mime, value)) = picked {
                let text = match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                cell.result = Some(text.clone());
                cell.result_mime = Some(mime.to_owned());
                if let Some(on_stream) = on_stream.as_mut() {
                    on_stream(StreamChunk {
                        name: "execute_result",
                        text: &text,
                    });
                }
            }
            Reduction::Continue
        }
        "display_data" | "update_display_data" => {
            let data = message.content.get("data");
            if let Some(payload) = data.and_then(|data| data.get(ATTACHMENT_DISPLAY_MIME)) {
                match parse_attachment_display(payload) {
                    AttachmentParse::Attachment(attachment) => cell.attachments.push(attachment),
                    AttachmentParse::Rejected(reason) => {
                        // A well-formed refused attachment fails the cell loudly,
                        // never a silent image drop.
                        if !cell.stderr.is_empty() {
                            cell.stderr.push('\n');
                        }
                        cell.stderr
                            .push_str(&format!("attachment dropped: {reason}"));
                        cell.status = ExecuteStatus::Error;
                    }
                    AttachmentParse::Malformed => {}
                }
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
    let stdout = keep_ends(cell.stdout, cell.stdout_chars, max_chars);
    let stderr = keep_ends(cell.stderr, cell.stderr_chars, max_chars);
    let result = (cell.result).map(|result| {
        let seen = result.chars().count();
        keep_ends(result, seen, max_chars)
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
        result_mime: cell.result_mime,
        attachments: cell.attachments,
        status,
        error: cell.error,
        duration_ms,
    }
}
