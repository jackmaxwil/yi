//! `read` on a file that is not text: the converter's copy, decoded text, or a refusal.
use std::path::Path;

use serde_json::{Map, Value, json};

use super::tool::{HashlineReadTool, SKELETON_ROWS, documents};
use crate::tool::{ToolContext, ToolOutput, error_output};

impl HashlineReadTool {
    pub(super) fn read_file(
        &self,
        display_path: &str,
        path: &Path,
        input: &Map<String, Value>,
        context: &ToolContext,
    ) -> ToolOutput {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return error_output(format!("failed to read {}: {error}", path.display()));
            }
        };
        let pages = input.get("pages").and_then(Value::as_str);
        let mut hint: Option<String> = None;
        if crate::document::could_be_document(&bytes) {
            match self.convert(path, &bytes, pages, context) {
                Some(crate::document::Converted::Markdown(copy)) => {
                    return self.render_copy(
                        display_path,
                        path,
                        &copy,
                        &bytes,
                        pages,
                        input,
                        context,
                    );
                }
                Some(crate::document::Converted::Unavailable(line)) => hint = Some(line),
                Some(crate::document::Converted::Refused(reason)) => {
                    let mut output =
                        error_output(format!("failed to read {}: {reason}", path.display()));
                    output.result.details = json!({ "refused": reason });
                    return output;
                }
                Some(crate::document::Converted::NotADocument) | None => {}
            }
        } else if pages.is_some() {
            return crate::tool::error_output_kind(
                "pages= applies to a PDF".to_owned(),
                yi_types::event::ToolErrorKind::InvalidArgs,
            );
        }
        if let Some(kind) = crate::document::image_kind(&bytes) {
            return error_output(format!(
                "failed to read {}: a {kind} image, not text; in ipython the bundled attach_image skill puts it in front of the model",
                path.display()
            ));
        }
        if bytes.starts_with(b"\xff\xfe") || bytes.starts_with(b"\xfe\xff") {
            let (decoded, encoding) = crate::document::decode_text(&bytes);
            let note = format!(
                "[decoded from {encoding}; the file is not UTF-8, so edit cannot anchor to it]"
            );
            return self.render_file(
                display_path,
                path,
                display_path,
                &decoded,
                Some(note),
                false,
                input,
                context,
            );
        }
        if crate::document::has_nul(&bytes) {
            let mut lines = vec![format!(
                "failed to read {}: a binary file (NUL bytes), and not a document the converter reads",
                path.display()
            )];
            lines.extend(hint);
            return error_output(lines.join("\n"));
        }
        match std::str::from_utf8(&bytes) {
            Ok(raw) => self.render_file(
                display_path,
                path,
                display_path,
                raw,
                None,
                true,
                input,
                context,
            ),
            Err(_) => {
                let (decoded, encoding) = crate::document::decode_text(&bytes);
                let note = format!(
                    "[decoded from {encoding}; the file is not UTF-8, so edit cannot anchor to it]"
                );
                self.render_file(
                    display_path,
                    path,
                    display_path,
                    &decoded,
                    Some(note),
                    false,
                    input,
                    context,
                )
            }
        }
    }

    fn convert(
        &self,
        path: &Path,
        bytes: &[u8],
        pages: Option<&str>,
        context: &ToolContext,
    ) -> Option<crate::document::Converted> {
        let documents = documents(&self.state)?;
        let source = crate::document::Source { path, bytes, pages };
        Some(crate::document::convert(
            &documents,
            &source,
            &context.cancelled,
        ))
    }

    pub(super) fn copy_of(
        &self,
        path: &Path,
        bytes: &[u8],
        pages: Option<&str>,
        context: &ToolContext,
    ) -> Option<crate::document::Copy> {
        if !crate::document::could_be_document(bytes) {
            return None;
        }
        match self.convert(path, bytes, pages, context)? {
            crate::document::Converted::Markdown(copy) => Some(copy),
            _ => None,
        }
    }

    /// The header keeps the original's path, so a re-read hits the cache and an edit meets the
    /// refusal, and nothing sends the model to a path outside its working tree.
    #[expect(
        clippy::too_many_arguments,
        reason = "one render, one document, its copy and its window"
    )]
    fn render_copy(
        &self,
        display_path: &str,
        path: &Path,
        copy: &crate::document::Copy,
        bytes: &[u8],
        pages: Option<&str>,
        input: &Map<String, Value>,
        context: &ToolContext,
    ) -> ToolOutput {
        let raw = match std::fs::read_to_string(&copy.path) {
            Ok(raw) => raw,
            Err(error) => {
                return error_output(format!("failed to read {}: {error}", copy.path.display()));
            }
        };
        let what = match pages {
            Some(pages) => format!("pages {pages} of the {}", copy.kind),
            None => copy.kind.clone(),
        };
        let note = format!(
            "[{display_path}: {what} converted to Markdown, read-only — edit and write refuse it]"
        );
        let resolve_as = copy.path.to_string_lossy().into_owned();
        let mut output = self.render_file(
            display_path,
            path,
            &resolve_as,
            &raw,
            Some(note),
            false,
            input,
            context,
        );
        if let Some(details) = output.result.details.as_object_mut() {
            details.insert("sourceBytes".to_owned(), json!(bytes.len()));
            details.insert(
                "converted".to_owned(),
                json!({ "from": copy.kind, "cache": if copy.hit { "hit" } else { "miss" }, "ms": copy.millis }),
            );
        }
        output
    }
}

/// Headings with their line numbers, so a capped read of prose still shows its shape.
pub(super) fn outline_rows(text: &str) -> Vec<String> {
    let mut fenced = false;
    let heads: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            if line.starts_with("```") {
                fenced = !fenced;
            }
            !fenced && super::blocks::heading_level(line).is_some()
        })
        .collect();
    if heads.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![format!(
        "[outline: {} of {} headings — find=\"heading text\" jumps to one]",
        heads.len().min(SKELETON_ROWS),
        heads.len()
    )];
    rows.extend(
        heads
            .iter()
            .take(SKELETON_ROWS)
            .map(|(index, line)| format!("  {}: {}", index.saturating_add(1), line.trim())),
    );
    rows
}
