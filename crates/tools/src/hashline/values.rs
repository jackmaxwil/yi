use std::path::{Path, PathBuf};

use crate::tool::Fuse;
use serde_json::{Value, json};

use super::patcher::{PatchSectionResult, SectionOp};

/// The lines a program asked for, whole and unclipped, until the fuse; `to_end` runs the first
/// window's start to the end of the file.
pub(super) fn program_lines(
    lines: &[&str],
    to_end: bool,
    windows: &[(usize, usize)],
    fuse: Fuse,
) -> (Vec<yi_types::tool_value::Line>, Option<String>) {
    let first = windows.first().map_or(1, |window| window.0);
    let open = [(first, lines.len())];
    let windows = if to_end { &open[..] } else { windows };
    let (mut kept, mut bytes) = (Vec::new(), 0_usize);
    for (at, &(start, end)) in windows.iter().enumerate() {
        for index in start.saturating_sub(1)..end {
            let Some(text) = lines.get(index) else {
                continue;
            };
            bytes = bytes.saturating_add(text.len());
            let n = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            let limit = if kept.len() >= fuse.items {
                format!("{} lines", fuse.items)
            } else if bytes > fuse.bytes {
                format!("{} MiB", fuse.bytes >> 20)
            } else {
                kept.push(yi_types::tool_value::Line {
                    n,
                    text: (*text).to_owned(),
                });
                continue;
            };
            if kept.is_empty() {
                let cut = format!(
                    "[line {n} alone is {} bytes, past the {limit} program fuse; bash `sed -n {n}p` with `cut -c` reads it in parts]",
                    text.len()
                );
                return (kept, Some(cut));
            }
            let later = windows.iter().skip(at.saturating_add(1));
            let rest: Vec<String> = std::iter::once(format!("[{n}, {end}]"))
                .chain(later.map(|(start, end)| format!("[{start}, {end}]")))
                .collect();
            let resume = match to_end {
                true => format!("offset={n}"),
                false => format!("ranges=[{}]", rest.join(", ")),
            };
            let cut = format!(
                "[kept {} lines, {first}-{} of {}: the program fuse is {limit}; read with {resume} for the rest]",
                kept.len(),
                n.saturating_sub(1),
                lines.len()
            );
            return (kept, Some(cut));
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
    fuse: usize,
) -> crate::ToolOutput {
    let shown = |path: &PathBuf| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    };
    let paths: Vec<String> = matches.iter().take(fuse).map(shown).collect();
    let mut cut = notices.to_vec();
    let mut rows = vec![format!("[{} files match {pattern}]", paths.len())];
    if matches.len() > fuse {
        rows.clear();
        cut.push(format!(
            "[kept the first {fuse} files matching {pattern}: the walk stopped at the program fuse of {fuse} files; narrow the glob for the rest]"
        ));
    }
    rows.extend(cut.iter().cloned());
    let mut output = crate::tool::text_output(rows.join("\n"));
    output.result.details = json!({ "files": paths.len(), "whole": 0 });
    let cut = (!cut.is_empty()).then(|| cut.join("\n"));
    let value = yi_types::tool_value::GlobValue { paths, cut };
    output.value = serde_json::to_value(value).ok();
    output
}
