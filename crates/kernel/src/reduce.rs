use serde_json::Value;
use yi_types::image::{has_magic_number, strict_base64_head};
use yi_types::kernel::{
    ExecuteResult, ExecuteStatus, JSON_MIME, JupyterMessage, KernelAttachment, KernelDisplay,
    KernelError, RESULT_MIMES,
};

use crate::{ATTACHMENT_DISPLAY_MIME, MAX_ATTACHMENT_DATA_CHARS};

const PLAIN_MIME: &str = "text/plain";
const IMAGE_MIMES: [&str; 2] = ["image/png", "image/jpeg"];
const DISPLAY_MIMES: [&str; 2] = ["text/html", "image/svg+xml"];
/// Images one cell's displays may add to the model's view, so a plotting loop sends few.
pub const MAX_DISPLAY_IMAGES: usize = 8;
/// Characters of html and svg one cell keeps for clients; the details are stored with the session.
pub const MAX_DISPLAY_CHARS: usize = 262_144;

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
    pub displays: Vec<KernelDisplay>,
    display_chars: usize,
    over_budget: usize,
    shown_images: usize,
    over_images: usize,
    refused: Vec<String>,
    unseen: std::collections::BTreeMap<String, usize>,
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
            displays: Vec::new(),
            display_chars: 0,
            over_budget: 0,
            shown_images: 0,
            over_images: 0,
            refused: Vec::new(),
            unseen: std::collections::BTreeMap::new(),
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
    match serde_json::from_value::<KernelAttachment>(payload.clone()) {
        Ok(attachment) => check_attachment(attachment),
        Err(_) => AttachmentParse::Malformed,
    }
}

fn check_attachment(attachment: KernelAttachment) -> AttachmentParse {
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

/// Incident: a string-valued `application/json` member was stored unquoted, so the pane called the
/// kernel's own well-formed value a parse failure.
fn picked_result(data: &Value, max_chars: usize) -> Option<(&'static str, String)> {
    let (mime, value) = RESULT_MIMES
        .iter()
        .find_map(|named| data.get(*named).map(|value| (*named, value)))
        .or_else(|| data.get(PLAIN_MIME).map(|value| (PLAIN_MIME, value)))?;
    if let Value::String(text) = value
        && mime != JSON_MIME
    {
        return Some((mime, text.clone()));
    }
    let mut sink = Capped {
        text: String::new(),
        limit: max_chars.saturating_mul(4).saturating_add(4),
    };
    let whole = serde_json::to_writer(&mut sink, value).is_ok();
    if whole && sink.text.chars().count() <= max_chars {
        return Some((mime, sink.text));
    }
    // Invariant: past the cap the walk's prefix is over `max_chars`, so `finish` cuts it with its
    // notice; the notice's total is a lower bound, the walk having stopped early.
    match data.get(PLAIN_MIME).and_then(Value::as_str) {
        Some(plain) => Some((PLAIN_MIME, plain.to_owned())),
        None => Some((mime, sink.text)),
    }
}

/// Invariant: serializing past four bytes a char is past the cell's char cap, so the walk stops
/// there instead of materializing a document the cell would cut anyway.
struct Capped {
    text: String,
    limit: usize,
}
impl std::io::Write for Capped {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let room = self.limit.saturating_sub(self.text.len());
        let written = std::str::from_utf8(bytes).unwrap_or_default();
        let keep = written.floor_char_boundary(room.min(written.len()));
        self.text.push_str(written.get(..keep).unwrap_or_default());
        if keep == bytes.len() {
            Ok(bytes.len())
        } else {
            Err(std::io::Error::other("past the cell's character cap"))
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An update replaces a display the model already read, so it adds no text to the model's view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Result,
    Display,
    Update,
}

/// A displayed bundle's images attach, its html and svg stay for clients, and its text reads as
/// an `execute_result`'s does; a member nothing carries is counted for the cell's one notice.
fn absorb_bundle(
    cell: &mut CellState,
    data: &Value,
    from: Source,
) -> Option<(&'static str, String)> {
    let bundle = data.as_object()?;
    let mut pictured = false;
    for mime in IMAGE_MIMES {
        let Some(raw) = bundle.get(mime).and_then(Value::as_str) else {
            continue;
        };
        if cell.shown_images >= MAX_DISPLAY_IMAGES {
            cell.over_images = cell.over_images.saturating_add(1);
            continue;
        }
        let image = KernelAttachment {
            mime_type: mime.to_owned(),
            data: raw.split_whitespace().collect(),
            path: None,
            bytes: None,
            modified_ns: None,
        };
        match check_attachment(image) {
            AttachmentParse::Attachment(image) => {
                cell.attachments.push(image);
                cell.shown_images = cell.shown_images.saturating_add(1);
                pictured = true;
            }
            AttachmentParse::Rejected(reason) => cell
                .refused
                .push(format!("[{mime} display not shown: {reason}]")),
            AttachmentParse::Malformed => {}
        }
    }
    for mime in DISPLAY_MIMES {
        let Some(text) = bundle.get(mime).and_then(Value::as_str) else {
            continue;
        };
        let chars = text.chars().count();
        if cell.display_chars.saturating_add(chars) > MAX_DISPLAY_CHARS {
            cell.over_budget = cell.over_budget.saturating_add(1);
            continue;
        }
        cell.display_chars = cell.display_chars.saturating_add(chars);
        cell.displays.push(KernelDisplay {
            mime_type: mime.to_owned(),
            data: text.to_owned(),
        });
    }
    let appended =
        from == Source::Display && !pictured && !bundle.contains_key(ATTACHMENT_DISPLAY_MIME);
    let picked = if from == Source::Result || appended {
        picked_result(data, cell.max_chars)
    } else {
        None
    };
    if let Some((_, text)) = picked.as_ref().filter(|_| appended) {
        append_capped(
            &mut cell.stdout,
            &mut cell.stdout_chars,
            &format!("{text}\n"),
            cell.max_chars,
        );
    }
    let shown = picked.is_some();
    for mime in bundle.keys() {
        let carried = IMAGE_MIMES.contains(&mime.as_str())
            || mime == PLAIN_MIME
            || mime == ATTACHMENT_DISPLAY_MIME
            || RESULT_MIMES.contains(&mime.as_str());
        let model_sees = shown && DISPLAY_MIMES.contains(&mime.as_str());
        if !carried && !model_sees {
            *cell.unseen.entry(mime.clone()).or_default() += 1;
        }
    }
    picked
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
            if let Some(data) = message.content.get("data")
                && let Some((mime, text)) = absorb_bundle(cell, data, Source::Result)
            {
                cell.result = Some(text);
                cell.result_mime = Some(mime.to_owned());
                if let Some(on_stream) = on_stream.as_mut() {
                    on_stream(StreamChunk {
                        name: "execute_result",
                        text: cell.result.as_deref().unwrap_or_default(),
                    });
                }
            }
            Reduction::Continue
        }
        "display_data" | "update_display_data" => {
            let data = message.content.get("data");
            if let Some(data) = data {
                absorb_bundle(
                    cell,
                    data,
                    if message.header.msg_type == "display_data" {
                        Source::Display
                    } else {
                        Source::Update
                    },
                );
            }
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
    let notices = notices(
        &cell.refused,
        cell.over_images,
        cell.over_budget,
        &cell.unseen,
    );
    ExecuteResult {
        stdout,
        stderr,
        result,
        result_mime: cell.result_mime,
        attachments: cell.attachments,
        status,
        error: cell.error,
        duration_ms,
        displays: cell.displays,
        notices,
    }
}

fn notices(
    refused: &[String],
    over_images: usize,
    over_budget: usize,
    unseen: &std::collections::BTreeMap<String, usize>,
) -> Vec<String> {
    let mut rows = refused.to_vec();
    if over_images > 0 {
        rows.push(format!(
            "[\u{2026} {MAX_DISPLAY_IMAGES} of {} image displays shown (MAX_DISPLAY_IMAGES {MAX_DISPLAY_IMAGES}); display fewer, or save the rest to files]",
            MAX_DISPLAY_IMAGES.saturating_add(over_images)
        ));
    }
    if over_budget > 0 {
        rows.push(format!(
            "[\u{2026} {over_budget} text/html or image/svg+xml displays not kept for clients (MAX_DISPLAY_CHARS {MAX_DISPLAY_CHARS} per cell); the model never reads them, write the data to a file to keep it]"
        ));
    }
    if !unseen.is_empty() {
        let list: Vec<String> = unseen
            .iter()
            .map(|(mime, n)| format!("{mime} x{n}"))
            .collect();
        rows.push(format!(
            "[\u{2026} display output not shown to the model: {}; it reads images (png, jpeg) and text, so print the data or display it as a PNG]",
            list.join(", ")
        ));
    }
    rows
}
