use serde_json::Value;

use super::patcher::{PatchSectionResult, SectionOp};

/// The lines a program asked for, or the whole file, whole and unclipped, until the fuse.
pub(super) fn program_lines(
    lines: &[&str],
    whole: bool,
    windows: &[(usize, usize)],
) -> (Vec<yi_types::tool_value::Line>, Option<String>) {
    let all = [(1, lines.len())];
    let (mut kept, mut bytes) = (Vec::new(), 0_usize);
    for &(start, end) in if whole { &all[..] } else { windows } {
        for index in start.saturating_sub(1)..end {
            let Some(text) = lines.get(index) else {
                continue;
            };
            bytes = bytes.saturating_add(text.len());
            let n = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            if bytes > yi_types::tool_value::PROGRAM_FUSE_BYTES {
                let fuse = yi_types::tool_value::PROGRAM_FUSE_BYTES / (1024 * 1024);
                let cut = format!(
                    "[kept lines up to {} of {}: the program fuse is {fuse} MiB; read with offset={n} for the rest]",
                    n.saturating_sub(1),
                    lines.len()
                );
                return (kept, Some(cut));
            }
            kept.push(yi_types::tool_value::Line {
                n,
                text: (*text).to_owned(),
            });
        }
    }
    (kept, None)
}

pub(super) fn edit_value(results: &[PatchSectionResult], diff: &str) -> Value {
    let file = |result: &PatchSectionResult| yi_types::tool_value::EditedFile {
        path: result.path.clone(),
        op: match result.op {
            SectionOp::Update => "updated",
            SectionOp::Delete => "deleted",
            SectionOp::Noop => "noop",
        }
        .to_owned(),
        tag: result.file_hash.to_string(),
        first_changed_line: result.first_changed_line,
        moved_to: result.move_dest.clone(),
    };
    let value = yi_types::tool_value::EditValue {
        files: results.iter().map(file).collect(),
        patch: diff.to_owned(),
    };
    serde_json::to_value(value).unwrap_or(Value::Null)
}
