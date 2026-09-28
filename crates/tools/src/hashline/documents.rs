//! `read` on a file that is not text: the converter's copy, decoded text, or a refusal.
use std::path::Path;

use serde_json::{Map, Value, json};

use super::tool::{HashlineReadTool, SKELETON_ROWS, View, documents};

/// A document's first look: enough to see what it is, with the outline ahead of it.
const DOCUMENT_FIRST_LOOK: usize = 12 * 1024;
/// A glob converts a document it has room for up to this size; a bigger one is listed unread.
const GLOB_CONVERT_MAX: usize = 8 * 1024 * 1024;

pub(super) enum Listed {
    Copy(crate::document::Copy),
    Unconverted(&'static str),
    Refused(String),
}

impl Listed {
    pub(super) fn describe(&self, source_bytes: usize) -> String {
        match self {
            Self::Copy(copy) => {
                let lines =
                    std::fs::read_to_string(&copy.path).map_or(0, |text| text.lines().count());
                format!(
                    "{} converted to {lines} lines of Markdown — read it for the text",
                    copy.kind
                )
            }
            Self::Unconverted(what) => format!(
                "{what}, {} KB, not converted here — read it for the text",
                source_bytes / 1024
            ),
            Self::Refused(reason) => format!("not readable: {reason}"),
        }
    }
}
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
        if pages.is_some() && !crate::document::is_pdf(&bytes) {
            return crate::tool::error_output_kind(
                "pages= applies to a PDF".to_owned(),
                yi_types::event::ToolErrorKind::InvalidArgs,
            );
        }
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
        }
        if let Some(kind) = crate::document::image_kind(&bytes) {
            return error_output(format!(
                "failed to read {}: a {kind} image, not text; in ipython run `print(await attach_image({}))` to put it in front of the model",
                path.display(),
                crate::document::python_str(path)
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
                &decoded,
                View {
                    note: Some(note),
                    on_disk: false,
                    ..View::source(display_path)
                },
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
                raw,
                View::source(display_path),
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
                    &decoded,
                    View {
                        note: Some(note),
                        on_disk: false,
                        ..View::source(display_path)
                    },
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

    /// What a glob shows for a document: the copy when one exists or there is room to make it,
    /// else what kept it unread. A big document is not converted just to be listed.
    pub(super) fn listed_document(
        &self,
        path: &Path,
        bytes: &[u8],
        room: bool,
        context: &ToolContext,
    ) -> Option<Listed> {
        if !crate::document::could_be_document(bytes) {
            return None;
        }
        let documents = documents(&self.state)?;
        let source = crate::document::Source {
            path,
            bytes,
            pages: None,
        };
        if let Some(copy) = crate::document::peek(&documents, &source) {
            return Some(Listed::Copy(copy));
        }
        if !room || bytes.len() > GLOB_CONVERT_MAX {
            return Some(Listed::Unconverted(if room {
                "large document"
            } else {
                "document"
            }));
        }
        match crate::document::convert(&documents, &source, &context.cancelled) {
            crate::document::Converted::Markdown(copy) => Some(Listed::Copy(copy)),
            crate::document::Converted::Refused(reason) => Some(Listed::Refused(reason)),
            crate::document::Converted::NotADocument
            | crate::document::Converted::Unavailable(_) => None,
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
            Some(pages) if pages.contains([',', '-']) => {
                format!("pages {pages} of the {}", copy.kind)
            }
            Some(page) => format!("page {page} of the {}", copy.kind),
            None => copy.kind.clone(),
        };
        let note = format!(
            "[{display_path}: {what} converted to Markdown, read-only — edit and write refuse it]"
        );
        let resolve_as = copy.path.to_string_lossy().into_owned();
        let mut output = self.render_file(
            display_path,
            path,
            &raw,
            View {
                resolve_as: &resolve_as,
                note: Some(note),
                on_disk: false,
                first_look: DOCUMENT_FIRST_LOOK,
            },
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

/// The shallowest heading levels that fit, sampled across the whole text and each title once, so
/// a book's front matter or a per-chapter "Chapter Overview" cannot crowd out its chapters.
pub(super) fn outline_rows(text: &str) -> Vec<String> {
    let mut fenced = false;
    let mut named = std::collections::HashSet::new();
    let heads: Vec<(usize, usize, &str)> = text
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if line.starts_with("```") {
                fenced = !fenced;
            }
            let level = super::blocks::heading_level(line)?;
            let title = line.trim_start_matches('#').trim();
            (!fenced && named.insert(title)).then_some((index, level, line))
        })
        .collect();
    if heads.is_empty() {
        return Vec::new();
    }
    let mut deepest = heads.iter().map(|(_, level, _)| *level).min().unwrap_or(1);
    while deepest < 6
        && heads
            .iter()
            .filter(|(_, level, _)| *level <= deepest + 1)
            .count()
            <= SKELETON_ROWS
    {
        deepest += 1;
    }
    let kept: Vec<&(usize, usize, &str)> = heads
        .iter()
        .filter(|(_, level, _)| *level <= deepest)
        .collect();
    let step = kept.len().div_ceil(SKELETON_ROWS).max(1);
    let shown: Vec<&&(usize, usize, &str)> = kept.iter().step_by(step).collect();
    let mut rows = vec![format!(
        "[outline: {} of {} headings (levels 1-{deepest}{}) — find=\"heading text\" jumps to one]",
        shown.len(),
        heads.len(),
        if step > 1 {
            format!(", one in {step}")
        } else {
            String::new()
        }
    )];
    rows.extend(
        shown
            .iter()
            .map(|(index, _, line)| format!("  {}: {}", index.saturating_add(1), line.trim())),
    );
    rows
}
