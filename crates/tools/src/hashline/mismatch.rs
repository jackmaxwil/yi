use super::format::{
    FileTag, HL_FILE_HASH_EXAMPLES, HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX,
};
use super::messages::format_anchored_context;
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

#[derive(Debug, Clone, PartialEq)]
pub struct MismatchError {
    pub path: Option<String>,
    pub expected_file_hash: String,
    pub actual_file_hash: FileTag,
    pub file_lines: Vec<String>,
    pub anchor_lines: Vec<u64>,
    pub hash_recognized: bool,
}

impl MismatchError {
    pub fn rejection_header(&self) -> Vec<String> {
        let path_text = self
            .path
            .as_deref()
            .map(|path| format!(" for {path}"))
            .unwrap_or_default();
        if !self.hash_recognized {
            return vec![
                format!(
                    "{EDIT_REJECTED_PREFIX}{path_text}: hash {HL_FILE_HASH_SEP}{} is not from this session.",
                    self.expected_file_hash
                ),
                format!(
                    "The current file hashes to {HL_FILE_HASH_SEP}{}. Copy a current {HL_FILE_PREFIX}path{HL_FILE_HASH_SEP}tag{HL_FILE_SUFFIX} header from the read, write or edit result for this file; never invent the tag and never reuse one from a prior session.",
                    self.actual_file_hash
                ),
            ];
        }
        vec![
            format!("{EDIT_REJECTED_PREFIX}{path_text}: file changed between read and edit."),
            format!(
                "Section is bound to {HL_FILE_HASH_SEP}{}, but the current file hashes to {HL_FILE_HASH_SEP}{}. If a prior edit in this session modified this file, copy the {HL_FILE_PREFIX}path{HL_FILE_HASH_SEP}newhash{HL_FILE_SUFFIX} header from that edit's response; otherwise re-read the file with `read` to refresh the tag before retrying.",
                self.expected_file_hash, self.actual_file_hash
            ),
        ]
    }

    pub fn display_message(&self) -> String {
        let mut lines = self.rejection_header();
        let context = format_anchored_context(&self.anchor_lines, &self.file_lines);
        if !context.is_empty() {
            lines.push(String::new());
            lines.extend(context);
        }
        lines.join("\n")
    }
}

impl std::fmt::Display for MismatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.display_message())
    }
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
