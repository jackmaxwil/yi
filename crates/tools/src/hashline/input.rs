use std::path::Path;

use super::clipboard::has_clipboard_edit;
use super::format::{HL_FILE_HASH_LENGTH, HL_FILE_PREFIX, HL_FILE_SUFFIX};
use super::messages::CLIPBOARD_INTERLEAVED_SECTIONS;
use super::parser::{Executor, ParsedSection};
use super::tokenizer::{TokenKind, classify_line, split_hashline_lines, tokenize_all};
use super::types::FileOp;

fn unquote_hashline_path(path_text: &str) -> &str {
    if path_text.len() < 2 {
        return path_text;
    }
    let bytes = path_text.as_bytes();
    let first = bytes[0];
    if (first == b'"' || first == b'\'') && bytes[path_text.len() - 1] == first {
        return &path_text[1..path_text.len() - 1];
    }
    path_text
}

/// Strip apply_patch-style noise models prepend to the path: a leading `***` and a
/// `(Update|Add|Delete|Move)<sep>*(File|to)?<sep>*:` keyword block, case-insensitive.
fn strip_apply_patch_path_noise(path_text: &str) -> &str {
    let mut rest = path_text.trim_start();
    let mut stars = 0;
    while stars < 3 && rest.starts_with('*') {
        rest = &rest[1..];
        stars += 1;
    }
    rest = rest.trim_start();
    // Keywords are ASCII: match the original bytes. A lowered copy can change
    // byte length ('İ' -> "i\u{307}"), desyncing offsets into a mid-char slice.
    let bytes = rest.as_bytes();
    for keyword in ["update", "add", "delete", "move"] {
        if !bytes
            .get(..keyword.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(keyword.as_bytes()))
        {
            continue;
        }
        let mut index = keyword.len();
        while index < bytes.len() && !bytes[index].is_ascii_alphanumeric() && bytes[index] != b':' {
            index += 1;
        }
        for tail_keyword in ["file", "to"] {
            let end = index + tail_keyword.len();
            if bytes
                .get(index..end)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(tail_keyword.as_bytes()))
            {
                index = end;
                break;
            }
        }
        while index < bytes.len() && !bytes[index].is_ascii_alphanumeric() && bytes[index] != b':' {
            index += 1;
        }
        if index < bytes.len() && bytes[index] == b':' {
            let mut after = &rest[index + 1..];
            after = after.trim_start();
            let mut trailing_stars = 0;
            while trailing_stars < 3 && after.starts_with('*') {
                after = &after[1..];
                trailing_stars += 1;
            }
            return after.trim_start();
        }
    }
    rest
}

fn normalize_hashline_path(raw_path: &str, cwd: Option<&Path>) -> String {
    let unquoted = strip_apply_patch_path_noise(unquote_hashline_path(raw_path.trim())).to_owned();
    let Some(cwd) = cwd else {
        return unquoted;
    };
    let candidate = Path::new(&unquoted);
    if !candidate.is_absolute() {
        return unquoted;
    }
    match candidate.strip_prefix(cwd) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_owned(),
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => unquoted,
    }
}

#[derive(Debug, Clone)]
struct RawSection {
    path: String,
    file_hash: Option<String>,
    diff: String,
    interleaved: bool,
}

fn try_parse_recovery_header(line: &str, cwd: Option<&Path>) -> Option<RawSection> {
    let body = line
        .strip_prefix(HL_FILE_PREFIX)?
        .strip_suffix(HL_FILE_SUFFIX)?
        .trim();
    let body = strip_apply_patch_path_noise(body);
    if body.is_empty() {
        return None;
    }
    let trimmed = body.trim_end();
    let (path_text, file_hash) = match trimmed.char_indices().rev().nth(HL_FILE_HASH_LENGTH) {
        Some((position, '#'))
            if trimmed[position + 1..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()) =>
        {
            (
                &trimmed[..position],
                Some(trimmed[position + 1..].to_uppercase()),
            )
        }
        _ => (trimmed, None),
    };
    if path_text.contains('#') {
        return None;
    }
    let path = normalize_hashline_path(path_text, cwd);
    if path.is_empty() {
        return None;
    }
    Some(RawSection {
        path,
        file_hash,
        diff: String::new(),
        interleaved: false,
    })
}

fn parse_hashline_header_line(
    line: &str,
    cwd: Option<&Path>,
) -> Result<Option<RawSection>, String> {
    let trimmed = line.trim_end();
    if !trimmed.starts_with(HL_FILE_PREFIX) {
        return Ok(None);
    }
    let token = classify_line(trimmed, 0);
    let TokenKind::Header { path, file_hash } = token.kind else {
        if let Some(recovered) = try_parse_recovery_header(trimmed, cwd) {
            return Ok(Some(recovered));
        }
        return Err(format!(
            "Input header must be {HL_FILE_PREFIX}PATH{HL_FILE_SUFFIX} or {HL_FILE_PREFIX}PATH#TAG{HL_FILE_SUFFIX} with a {HL_FILE_HASH_LENGTH}-hex content-hash tag; got {trimmed:?}. \
TAG is the {HL_FILE_HASH_LENGTH}-hex after # on the {HL_FILE_PREFIX}path#TAG{HL_FILE_SUFFIX} line of the read, write or edit result; omit it to edit the last version shown."
        ));
    };
    let parsed_path = normalize_hashline_path(&path, cwd);
    if parsed_path.is_empty() {
        return Err(format!(
            "Input header \"{HL_FILE_PREFIX}{HL_FILE_SUFFIX}\" is empty; provide a file path."
        ));
    }
    Ok(Some(RawSection {
        path: parsed_path,
        file_hash,
        diff: String::new(),
        interleaved: false,
    }))
}

fn split_raw_sections(input: &str, cwd: Option<&Path>) -> Result<Vec<RawSection>, String> {
    let stripped = input.strip_prefix('\u{FEFF}').unwrap_or(input);
    let mut lines: Vec<&str> = split_hashline_lines(stripped);
    let mut start = 0;
    while start < lines.len() {
        let head = lines[start];
        let token = classify_line(head, 0);
        if head.trim().is_empty() || matches!(token.kind, TokenKind::EnvelopeBegin) {
            start += 1;
            continue;
        }
        break;
    }
    lines.drain(..start);
    let first_line = lines.first().copied().unwrap_or("");

    if parse_hashline_header_line(first_line, cwd)?.is_none() {
        let first_trimmed = first_line.trim_end();
        if first_trimmed.starts_with("@@") {
            return Err(format!(
                "unified-diff hunk header (`@@ -N,M +N,M @@`) is not valid in hashline. \
File sections start with `{HL_FILE_PREFIX}path#HASH{HL_FILE_SUFFIX}`; use `replace`, `delete`, or `insert` ops."
            ));
        }
        let preview: String = first_line.chars().take(120).collect();
        return Err(format!(
            "input must begin with \"{HL_FILE_PREFIX}PATH#HASH{HL_FILE_SUFFIX}\" on the first non-blank line for anchored edits; got: {preview:?}. \
Example: \"{HL_FILE_PREFIX}src/foo.ts#1A2B{HL_FILE_SUFFIX}\" then edit ops."
        ));
    }

    let mut sections: Vec<RawSection> = Vec::new();
    let mut current: Option<RawSection> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in lines {
        let trimmed = line.trim_end();
        let token = classify_line(line, 0);
        match token.kind {
            TokenKind::EnvelopeEnd | TokenKind::Abort => break,
            TokenKind::EnvelopeBegin => continue,
            _ => {}
        }
        if trimmed.starts_with(HL_FILE_PREFIX)
            && let Some(header) = parse_hashline_header_line(line, cwd)?
        {
            let previous = current.take();
            if let Some(section) = previous {
                let has_ops = current_lines.iter().any(|line| !line.trim().is_empty());
                if has_ops {
                    sections.push(RawSection {
                        diff: current_lines.join("\n"),
                        ..section
                    });
                }
            }
            current_lines.clear();
            current = Some(header);
            continue;
        }
        current_lines.push(line);
    }
    if let Some(section) = current.take() {
        let has_ops = current_lines.iter().any(|line| !line.trim().is_empty());
        if has_ops {
            sections.push(RawSection {
                diff: current_lines.join("\n"),
                ..section
            });
        }
    }
    Ok(sections)
}

/// Same-path sections merge into one with concatenated diffs: anchors authored against one
/// snapshot must apply as a batch, or the first shifts lines under the second.
fn merge_same_path_sections(sections: Vec<RawSection>) -> Result<Vec<RawSection>, String> {
    let mut merged: Vec<RawSection> = Vec::new();
    let mut previous_path: Option<String> = None;
    for section in sections {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| existing.path == section.path)
        {
            if let (Some(existing_hash), Some(section_hash)) =
                (existing.file_hash.as_deref(), section.file_hash.as_deref())
                && existing_hash != section_hash
            {
                return Err(format!(
                    "Conflicting hashline snapshot tags for {}: #{existing_hash} and #{section_hash}. Re-read the file and retry with one current header.",
                    section.path
                ));
            }
            if existing.file_hash.is_none() {
                existing.file_hash = section.file_hash;
            }
            // Merging across another file's section moves these ops up to the
            // first occurrence; flag it so clipboard ops can refuse the reorder.
            if previous_path.as_deref() != Some(section.path.as_str()) {
                existing.interleaved = true;
            }
            existing.diff.push('\n');
            existing.diff.push_str(&section.diff);
            previous_path = Some(section.path);
            continue;
        }
        previous_path = Some(section.path.clone());
        merged.push(section);
    }
    Ok(merged)
}

#[derive(Debug, Clone)]
pub struct PatchSection {
    pub path: String,
    pub file_hash: Option<String>,
    pub diff: String,
    interleaved: bool,
}

impl PatchSection {
    pub fn parse(&self) -> Result<ParsedSection, String> {
        let mut executor = Executor::new();
        for token in tokenize_all(&self.diff) {
            executor.feed(&token)?;
        }
        let mut parsed = executor.end()?;
        // Same-path sections merge into their first occurrence; if that merge crossed another
        // file's section the authored register order is gone and clipboard ops are undefined.
        if self.interleaved && has_clipboard_edit(&parsed.edits) {
            return Err(CLIPBOARD_INTERLEAVED_SECTIONS.to_owned());
        }
        if let Some(FileOp::Move { dest }) = &parsed.file_op {
            parsed.file_op = Some(FileOp::Move {
                dest: normalize_hashline_path(dest, None),
            });
        }
        Ok(parsed)
    }

    pub fn with_path(&self, path: String) -> Self {
        Self {
            path,
            ..self.clone()
        }
    }

    pub fn collect_anchor_lines(&self) -> Result<Vec<u64>, String> {
        use super::types::{Cursor, Edit, PasteTarget};
        let parsed = self.parse()?;
        let mut lines: Vec<u64> = Vec::new();
        for edit in &parsed.edits {
            match edit {
                Edit::Delete { anchor, .. } | Edit::Block { anchor, .. } => lines.push(anchor.line),
                Edit::Cut { range, .. } => lines.extend(range.start.line..=range.end.line),
                Edit::Paste { at, .. } => match at {
                    PasteTarget::Span { range } => lines.extend(range.start.line..=range.end.line),
                    PasteTarget::Gap {
                        cursor: Cursor::BeforeAnchor { anchor } | Cursor::AfterAnchor { anchor },
                    } => lines.push(anchor.line),
                    PasteTarget::Gap { .. } => {}
                },
                Edit::Insert {
                    cursor: Cursor::BeforeAnchor { anchor } | Cursor::AfterAnchor { anchor },
                    ..
                } => lines.push(anchor.line),
                Edit::Insert { .. } => {}
            }
        }
        lines.sort_unstable();
        lines.dedup();
        Ok(lines)
    }

    /// True when at least one edit anchors to concrete file content; pure
    /// head/tail literal inserts are safe to apply to files that don't exist.
    pub fn has_anchor_scoped_edit(&self) -> Result<bool, String> {
        Ok(!self.collect_anchor_lines()?.is_empty())
    }
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub sections: Vec<PatchSection>,
}

impl Patch {
    pub fn parse(input: &str, cwd: Option<&Path>) -> Result<Self, String> {
        let raw = merge_same_path_sections(split_raw_sections(input, cwd)?)?;
        Ok(Self {
            sections: raw
                .into_iter()
                .map(|section| PatchSection {
                    path: section.path,
                    file_hash: section.file_hash,
                    diff: section.diff,
                    interleaved: section.interleaved,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_scan_survives_multibyte_lowercase() {
        // 'İ' (U+0130) lowercases to "i\u{307}", 2 bytes to 3.
        assert_eq!(strip_apply_patch_path_noise("update\u{130}file: x"), "x");
    }
}
