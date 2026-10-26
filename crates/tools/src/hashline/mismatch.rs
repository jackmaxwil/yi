use super::format::{FileTag, HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX};

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
