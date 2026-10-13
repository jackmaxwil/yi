use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use yi_types::tool_value::{PROGRAM_FUSE_BYTES, PROGRAM_FUSE_ITEMS};

use super::patcher::{PatchSectionResult, SectionOp};

/// The lines a program asked for, whole and unclipped, until the fuse; `to_end` runs the first
/// window's start to the end of the file.
pub(super) fn program_lines(
    lines: &[&str],
    to_end: bool,
    windows: &[(usize, usize)],
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
            let fuse = if kept.len() >= PROGRAM_FUSE_ITEMS {
                format!("{PROGRAM_FUSE_ITEMS} lines")
            } else if bytes > PROGRAM_FUSE_BYTES {
                format!("{} MiB", PROGRAM_FUSE_BYTES >> 20)
            } else {
                kept.push(yi_types::tool_value::Line {
                    n,
                    text: (*text).to_owned(),
                });
                continue;
            };
            if kept.is_empty() {
                let cut = format!(
                    "[line {n} alone is {} bytes, past the {fuse} program fuse; bash `sed -n {n}p` with `cut -c` reads it in parts]",
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
                "[kept {} lines, {first}-{} of {}: the program fuse is {fuse}; read with {resume} for the rest]",
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
) -> crate::ToolOutput {
    let shown = |path: &PathBuf| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    };
    let paths: Vec<String> = matches.iter().take(PROGRAM_FUSE_ITEMS).map(shown).collect();
    let mut cut = notices.to_vec();
    let mut rows = vec![format!("[{} files match {pattern}]", paths.len())];
    if matches.len() > PROGRAM_FUSE_ITEMS {
        rows.clear();
        cut.push(format!(
            "[kept the first {PROGRAM_FUSE_ITEMS} files matching {pattern}: the walk stopped at the program fuse of {PROGRAM_FUSE_ITEMS} files; narrow the glob for the rest]"
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
