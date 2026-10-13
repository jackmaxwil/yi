use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::patcher::{PatchSectionResult, SectionOp};

/// The lines a program asked for, whole and unclipped, until the fuse; `to_end` runs the first
/// window's start to the end of the file.
pub(super) fn program_lines(
    lines: &[&str],
    to_end: bool,
    windows: &[(usize, usize)],
) -> (Vec<yi_types::tool_value::Line>, Option<String>) {
    let start = windows.first().map_or(1, |window| window.0);
    let open = [(start, lines.len())];
    let total = u64::try_from(lines.len()).unwrap_or(u64::MAX);
    let (mut kept, mut bytes) = (Vec::new(), 0_usize);
    for &(start, end) in if to_end { &open[..] } else { windows } {
        for index in start.saturating_sub(1)..end {
            let Some(text) = lines.get(index) else {
                continue;
            };
            bytes = bytes.saturating_add(text.len());
            let n = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            if kept.len() >= yi_types::tool_value::PROGRAM_FUSE_ITEMS {
                let fuse = yi_types::tool_value::PROGRAM_FUSE_ITEMS;
                let cut = yi_types::tool_value::fuse_cut(
                    kept.len(),
                    total,
                    &format!("{fuse} items"),
                    &format!("read with offset={n} for the rest"),
                );
                return (kept, Some(cut));
            }
            if bytes > yi_types::tool_value::PROGRAM_FUSE_BYTES {
                let fuse = yi_types::tool_value::PROGRAM_FUSE_BYTES >> 20;
                let cut = match kept.is_empty() {
                    true => format!(
                        "[line {n} alone is {} bytes, past the {fuse} MiB program fuse; bash `sed -n {n}p` with `cut -c` reads it in parts]",
                        text.len()
                    ),
                    false => yi_types::tool_value::fuse_cut(
                        kept.len(),
                        total,
                        &format!("{fuse} MiB"),
                        &format!("read with offset={n} for the rest"),
                    ),
                };
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

/// A program's glob is its paths: no file is opened, so a walk of a large tree stays a walk.
pub(super) fn program_glob(
    pattern: &str,
    matches: &[PathBuf],
    root: &Path,
    notices: &[String],
) -> crate::ToolOutput {
    let shown = |path: &PathBuf| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    };
    let paths: Vec<String> = matches.iter().map(shown).collect();
    let mut rows = vec![format!("[{} files match {pattern}]", paths.len())];
    rows.extend(notices.iter().cloned());
    let mut output = crate::tool::text_output(rows.join("\n"));
    output.result.details = json!({ "files": paths.len(), "whole": 0 });
    let cut = (!notices.is_empty()).then(|| notices.join("\n"));
    let value = yi_types::tool_value::GlobValue { paths, cut };
    output.value = serde_json::to_value(value).ok();
    output
}
