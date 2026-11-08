//! `read` on a file that is not text: the converter's copy, decoded text, or a refusal.
use serde_json::{Map, Value, json};
use std::path::Path;

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
use crate::tool::{Cap, ToolContext, ToolOutput, error_output, text_output};
use yi_types::image::{ImageDefect, MAX_IMAGE_CHARS, MAX_IMAGE_SIDE};

impl HashlineReadTool {
    pub(super) fn read_file(
        &self,
        display_path: &str,
        path: &Path,
        input: &Map<String, Value>,
        context: &ToolContext,
    ) -> ToolOutput {
        let pages = match input.get("pages").and_then(Value::as_str) {
            Some(spec) => match crate::document::parse_pages(spec) {
                Ok(_) => Some(spec),
                Err(reason) => {
                    return crate::tool::error_output_kind(
                        reason,
                        yi_types::event::ToolErrorKind::InvalidArgs,
                    );
                }
            },
            None => None,
        };
        let (bytes, stat) = match context.read_stat(path) {
            Ok((bytes, stat)) => (bytes, Some(stat)),
            Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
                let Some(document) = self.document_bytes(path, pages, context) else {
                    return error_output(format!("failed to read {}: {error}", path.display()));
                };
                match document {
                    Ok(bytes) => (bytes, None),
                    Err(output) => return output,
                }
            }
            Err(error) => {
                return error_output(format!("failed to read {}: {error}", path.display()));
            }
        };
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
            return image(display_path, path, kind, &bytes, stat.as_ref());
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

    /// The bytes of a file past the text cap that is a document, under the converter's own
    /// ceiling, or an image's size refusal; `None` for anything else, so the text cap's row stands.
    fn document_bytes(
        &self,
        path: &Path,
        pages: Option<&str>,
        context: &ToolContext,
    ) -> Option<Result<Vec<u8>, ToolOutput>> {
        let file = context.read_gate().open(path, &context.deny_read).ok()?;
        let len = usize::try_from(file.metadata().ok()?.len()).unwrap_or(usize::MAX);
        let window = u64::try_from(crate::document::MARKER_WINDOW).unwrap_or(u64::MAX);
        let mut head = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(file, window), &mut head).ok()?;
        if let Some((kind, _)) = crate::document::image_kind(&head) {
            let chars = len.div_ceil(3).saturating_mul(4);
            return Some(Err(refusal(path, kind, ImageDefect::TooLarge { chars })));
        }
        if !crate::document::could_be_document(&head) {
            return None;
        }
        let pdf = crate::document::is_pdf(&head);
        let cap = match pages.is_some() && pdf {
            true => Cap::Paged,
            false => Cap::Document { pages: pdf },
        };
        Some(
            context.read_document(path, cap).map_err(|error| {
                error_output(format!("failed to read {}: {error}", path.display()))
            }),
        )
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

/// An image the provider takes goes to the model as itself, under a line saying what it is;
/// `attachmentMedia` carries the stat the read opened it with, so reclaim can attach it again.
fn image(
    display_path: &str,
    path: &Path,
    (kind, mime): (&str, &str),
    bytes: &[u8],
    stat: Option<&std::fs::Metadata>,
) -> ToolOutput {
    let most = (MAX_IMAGE_CHARS / 4).saturating_mul(3);
    if bytes.len() > most {
        let chars = bytes.len().div_ceil(3).saturating_mul(4);
        return refusal(path, kind, ImageDefect::TooLarge { chars });
    }
    let data = yi_types::encoding::base64(bytes);
    let side = yi_types::image::image_side(mime, bytes);
    let defect = yi_types::image::image_defect(mime, &data).or(match side {
        Some((width, height)) if width.max(height) > MAX_IMAGE_SIDE => {
            Some(ImageDefect::TooWide { width, height })
        }
        Some(_) => None,
        None => matches!(mime, "image/jpeg" | "image/webp").then_some(ImageDefect::NoHeader),
    });
    if let Some(defect) = defect {
        return refusal(path, kind, defect);
    }
    let side = side.map_or_else(String::new, |(width, height)| {
        format!(", {width}x{height} px")
    });
    let mut output = text_output(format!(
        "{display_path}: a {kind} image{side}, {} bytes",
        bytes.len()
    ));
    let row = stat
        .filter(|stat| usize::try_from(stat.len()).ok() == Some(bytes.len()))
        .zip(path.to_str())
        .map(|(stat, path)| {
            let modified_ns = (stat.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|since| u64::try_from(since.as_nanos()).ok());
            json!({ "mime_type": mime, "path": path, "bytes": stat.len(), "modified_ns": modified_ns })
        });
    if let Some(row) = row {
        output.result.details = json!({ "attachmentMedia": [row] });
    }
    output
        .result
        .content
        .push(yi_types::message::Content::Image {
            data,
            mime_type: mime.to_owned(),
        });
    output
}

/// Why the provider would refuse the image, and the ipython cell that sends it when one can.
/// Invariant: the cell's fence is one backtick longer than any run in it, so a path cannot end it.
fn refusal(path: &Path, kind: &str, defect: ImageDefect) -> ToolOutput {
    use ImageDefect::{NoHeader, NotBase64, NotItsType, TooLarge, TooWide, Type};
    let remedy = defect.remedy();
    let next = match (defect, path.to_str()) {
        (Type | TooLarge { .. } | TooWide { .. }, Some(_)) => {
            let statement = remedy.split('`').nth(1).unwrap_or(remedy);
            let cell = format!("path = {}; {statement}", crate::document::python_str(path));
            let run = cell.split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let fence = "`".repeat(run.saturating_add(1));
            format!("; in ipython run {fence}{cell}{fence}")
        }
        (Type | TooLarge { .. } | TooWide { .. }, None) => {
            "; its path is not UTF-8, so no Python string names it for a cell".to_owned()
        }
        (NoHeader | NotBase64 | NotItsType, _) => ", so no decoder opens it".to_owned(),
    };
    let path = path.display();
    error_output(format!(
        "failed to read {path}: the {kind} image {defect}{next}"
    ))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::os::unix::ffi::OsStrExt;

    const CHECKER: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../runtime/tests/fixtures/documents/checker.png"
    );

    /// Incident: a non-UTF-8 name serialized through `json!` as a `Path` panicked the read; a
    /// non-UTF-8 cwd or a Linux glob reaches it, and APFS refuses to create one, so no file is.
    #[test]
    fn a_path_that_is_not_utf8_sends_the_image_and_names_no_cell() -> Result<(), Box<dyn Error>> {
        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xffchecker.png"));
        let bytes = std::fs::read(CHECKER)?;
        let stat = std::fs::metadata(CHECKER)?;
        let sent = super::image("x.png", path, ("PNG", "image/png"), &bytes, Some(&stat));
        assert!(!sent.is_error);
        assert_eq!(sent.result.content.len(), 2);
        assert_eq!(sent.result.details.get("attachmentMedia"), None);
        let mut wide = bytes;
        wide.get_mut(16..20)
            .ok_or("short png")?
            .copy_from_slice(&8001_u32.to_be_bytes());
        let refused = super::image("x.png", path, ("PNG", "image/png"), &wide, None);
        let text = yi_types::message::join_text(&refused.result.content, "");
        assert!(refused.is_error);
        assert!(
            text.ends_with("over MAX_IMAGE_SIDE 8000 px a side; its path is not UTF-8, so no Python string names it for a cell"),
            "{text}"
        );
        Ok(())
    }
}
