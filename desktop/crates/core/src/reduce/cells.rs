//! The agent's kernel cells: each `ipython` call of a transcript, read as a notebook cell.

use serde_json::Value;
use yi_types::image::image_side;
use yi_types::message::Content;

use crate::reduce::turns::{Message, ToolCall, Transcript};

/// The tool whose calls run in the session's kernel.
pub const TOOL: &str = "ipython";

/// The display-bundle member a cell's value was drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mime<'a> {
    Plain,
    Markdown,
    Json,
    Python,
    Other(&'a str),
}

/// A picture a cell showed: its type and its base64 body, as Yi sent them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picture<'a> {
    pub mime: &'a str,
    pub data: &'a str,
}

/// One output of a cell, in the order a notebook draws them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output<'a> {
    /// The kernel restarted before this cell ran: what earlier cells defined is gone.
    Restarted,
    Stdout(&'a str),
    Stderr(&'a str),
    Picture(Picture<'a>),
    Value(Mime<'a>, &'a str),
    Error {
        name: &'a str,
        value: &'a str,
        traceback: Vec<&'a str>,
    },
    /// Stopped by a cancel or at the kernel's time limit.
    Aborted,
}

impl Transcript {
    /// The kernel cells, in the order the agent wrote them.
    pub fn cells(&self) -> impl Iterator<Item = &ToolCall> {
        let calls = self.messages.iter().flat_map(Message::calls);
        let found = calls.filter_map(|id| self.tools.get(id));
        found.filter(|call| call.name == TOOL)
    }

    /// Whether a kernel cell has shown a picture: what brings the IPython pane by itself.
    pub fn pictured(&self) -> bool {
        let picture = |block: &Content| matches!(block, Content::Image { .. });
        self.cells().any(|cell| cell.output.iter().any(picture))
    }
}

impl ToolCall {
    /// The Python a kernel cell ran, as the model wrote it.
    pub fn code(&self) -> &str {
        self.args.get("code").and_then(Value::as_str).unwrap_or("")
    }

    /// What a finished kernel cell put out, read from its result's details and images; none
    /// while it runs, since Yi sends a cell's result whole when it ends.
    pub fn outputs(&self) -> Vec<Output<'_>> {
        let text = |name: &str| self.details.get(name).and_then(Value::as_str);
        let mut outputs = Vec::new();
        if self.details.get("kernelRestarted").and_then(Value::as_bool) == Some(true) {
            outputs.push(Output::Restarted);
        }
        outputs.extend(text("stdout").map(Output::Stdout));
        outputs.extend(text("stderr").map(Output::Stderr));
        let pictures = self.output.iter().filter_map(|block| match block {
            Content::Image { data, mime_type } => Some(Picture {
                mime: mime_type,
                data,
            }),
            _ => None,
        });
        outputs.extend(pictures.map(Output::Picture));
        if let Some(value) = text("result") {
            outputs.push(Output::Value(mime(text("resultMime")), value));
        }
        if let Some(error) = self.details.get("error").filter(|e| !e.is_null()) {
            let field = |name: &str| error.get(name).and_then(Value::as_str).unwrap_or("");
            let lines = error.get("traceback").and_then(Value::as_array);
            let traceback = lines.into_iter().flatten().filter_map(Value::as_str);
            outputs.push(Output::Error {
                name: field("ename"),
                value: field("evalue"),
                traceback: traceback.collect(),
            });
        }
        if text("status") == Some("aborted") {
            outputs.push(Output::Aborted);
        }
        outputs
    }
}

fn mime(named: Option<&str>) -> Mime<'_> {
    match named {
        None | Some("text/plain") => Mime::Plain,
        Some("text/markdown") => Mime::Markdown,
        Some("application/json") => Mime::Json,
        Some("text/x-python") => Mime::Python,
        Some(other) => Mime::Other(other),
    }
}

impl Picture<'_> {
    /// The picture's bytes; none when its body is not base64.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        let body = self.data.trim_end_matches('=').as_bytes();
        let mut out = Vec::with_capacity(body.len().saturating_mul(3) / 4);
        for group in body.chunks(4) {
            let mut bits: u32 = 0;
            for &byte in group {
                bits = bits << 6 | sextet(byte)?;
            }
            let filled = bits << 4usize.saturating_sub(group.len()).saturating_mul(6);
            let [_, a, b, c] = filled.to_be_bytes();
            out.extend([a, b, c].into_iter().take(group.len().saturating_sub(1)));
        }
        Some(out)
    }

    /// Its width and height in pixels, from the header in `bytes`.
    pub fn side(&self, bytes: &[u8]) -> Option<(u32, u32)> {
        image_side(self.mime, bytes)
    }
}

fn sextet(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

#[cfg(test)]
mod tests {
    use super::Picture;

    /// A chart decodes to the PNG it was, or the pane draws nothing where it should be. The
    /// body is the head of the chart in `fixtures/wire/kernel-cells.jsonl`.
    #[test]
    fn a_pictures_base64_body_decodes_to_its_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let png = Picture {
            mime: "image/png",
            data: "iVBORw0KGgoAAAANSUhEUgAAAPAAAACMCAIAAADN17N/",
        };
        let bytes = png.bytes().ok_or("not base64")?;
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(png.side(&bytes), Some((240, 140)));
        let bad = Picture {
            mime: "image/png",
            data: "iVBO*w0K",
        };
        assert_eq!(bad.bytes(), None);
        Ok(())
    }
}
