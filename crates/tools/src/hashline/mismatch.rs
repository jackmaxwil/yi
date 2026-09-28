use super::format::{
    FileTag, HL_FILE_HASH_EXAMPLES, HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX,
};
use super::types::Anchor;

pub fn format_full_anchor_requirement(raw: Option<&str>) -> String {
    let received = raw
        .map(|raw| format!(" Received {raw:?}."))
        .unwrap_or_default();
    format!(
        "a bare line number from read/search output plus the section header content-hash tag \
(for example {HL_FILE_PREFIX}src/foo.ts{HL_FILE_HASH_SEP}{}{HL_FILE_SUFFIX} and line \"160\"){received}",
        HL_FILE_HASH_EXAMPLES[0]
    )
}

pub fn parse_line_ref(reference: &str) -> Result<Anchor, String> {
    let trimmed = reference.trim();
    let stripped = trimmed
        .trim_start_matches(['>', '+', '-', '*'])
        .trim_start();
    let digits: String = stripped.chars().take_while(char::is_ascii_digit).collect();
    let rest = &stripped[digits.len()..];
    let rest_ok = rest.is_empty() || rest.starts_with(':');
    if digits.is_empty() || !rest_ok {
        return Err(format!(
            "Invalid line reference. Expected {}.",
            format_full_anchor_requirement(Some(reference))
        ));
    }
    let line: u64 = digits.parse().map_err(|_| {
        format!(
            "Invalid line reference. Expected {}.",
            format_full_anchor_requirement(Some(reference))
        )
    })?;
    if line < 1 {
        return Err(format!(
            "Line number must be >= 1, got {line} in \"{reference}\"."
        ));
    }
    Ok(Anchor { line })
}

/// Pinned prefix: the edit tool classifies a rejection as `stale_tag` by it.
pub const EDIT_REJECTED_PREFIX: &str = "Edit rejected";

/// The rejection for a tag that is not the live file's: unknown here, or known but moved.
pub fn mismatch_message(
    path: &str,
    expected: &str,
    actual: FileTag,
    hash_recognized: bool,
    rows: &[String],
    footer: &str,
) -> String {
    let header = if hash_recognized {
        format!(
            "{EDIT_REJECTED_PREFIX} for {path}: file changed between read and edit.\n\
Section is bound to {HL_FILE_HASH_SEP}{expected}, but the current file hashes to {HL_FILE_HASH_SEP}{actual}. If a prior edit in this session modified this file, copy the {HL_FILE_PREFIX}path{HL_FILE_HASH_SEP}newhash{HL_FILE_SUFFIX} header from that edit's response; otherwise re-read the file with `read` to refresh the tag before retrying."
        )
    } else {
        format!(
            "{EDIT_REJECTED_PREFIX} for {path}: hash {HL_FILE_HASH_SEP}{expected} is not from this session.\n\
The current file hashes to {HL_FILE_HASH_SEP}{actual}. Copy a current {HL_FILE_PREFIX}path{HL_FILE_HASH_SEP}tag{HL_FILE_SUFFIX} header from the read, write or edit result for this file; never invent the tag and never reuse one from a prior session."
        )
    };
    let mut lines = vec![header];
    if !rows.is_empty() {
        lines.push(String::new());
        lines.extend(rows.iter().cloned());
    }
    lines.push(footer.to_owned());
    lines.join("\n")
}

pub fn validate_line_ref(anchor: Anchor, file_lines: &[String]) -> Result<(), String> {
    if anchor.line < 1 || anchor.line > file_lines.len() as u64 {
        return Err(format!(
            "Line {} does not exist (file has {} lines)",
            anchor.line,
            file_lines.len()
        ));
    }
    Ok(())
}
