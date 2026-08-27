use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use super::format::{format_hashline_header, format_numbered_line};
use super::input::Patch;
use super::normalize::{normalize_to_lf, strip_bom};
use super::patcher::{PatchSectionResult, Patcher, SectionOp};
use super::snapshots::SnapshotStore;
use super::types::Clipboard;
use crate::tool::{
    Tool, ToolContext, ToolKind, ToolOutput, error_output, require_str, resolve_path, text_output,
};

const READ_LINE_CAP: usize = 2_000;
/// Consecutive byte-identical no-op edits on one path before the soft hint
/// escalates to a tool error (omp issue #2081: 182 identical repeats in 205
/// calls before the user aborted).
const NOOP_HARD_LIMIT: u32 = 3;
const EDIT_CONTEXT_LINES: u64 = 3;

#[derive(Default)]
pub struct HashlineState {
    pub snapshots: SnapshotStore,
    noop: std::collections::HashMap<String, (u64, u32)>,
    clipboard: Clipboard,
}

pub type SharedHashline = Arc<Mutex<HashlineState>>;

pub fn shared_hashline_state() -> SharedHashline {
    Arc::new(Mutex::new(HashlineState::default()))
}

fn lock_state(state: &SharedHashline) -> std::sync::MutexGuard<'_, HashlineState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn input_hash(input: &str) -> u64 {
    u64::from(xxhash_rust::xxh32::xxh32(input.as_bytes(), 0))
}

pub struct HashlineReadTool {
    pub state: SharedHashline,
}

impl Tool for HashlineReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a file. Output starts with a [path#TAG] snapshot header and numbered LINE:TEXT rows; use both to anchor edits. Optional offset (1-based line) and limit select a range."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path (absolute, or relative to the working directory)"},
                "offset": {"type": "integer", "description": "1-based first line to read"},
                "limit": {"type": "integer", "description": "Maximum number of lines to read"}
            },
            "required": ["path"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let display_path = match require_str(&input, "path") {
            Ok(path) => path.to_owned(),
            Err(message) => return error_output(message),
        };
        let path = resolve_path(context, &display_path);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) => {
                return error_output(format!("failed to read {}: {error}", path.display()));
            }
        };
        let normalized = normalize_to_lf(strip_bom(&raw).text);
        let all_lines: Vec<&str> = normalized.split('\n').collect();
        let line_count = if normalized.ends_with('\n') {
            all_lines.len().saturating_sub(1)
        } else {
            all_lines.len()
        };
        let offset = input
            .get("offset")
            .and_then(Value::as_u64)
            .map_or(0, |line| line.saturating_sub(1) as usize);
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(READ_LINE_CAP, |limit| limit as usize)
            .min(READ_LINE_CAP);
        let end = offset.saturating_add(limit).min(line_count);
        let seen: Vec<u64> = ((offset as u64 + 1)..=(end as u64)).collect();

        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .to_string_lossy()
            .into_owned();
        let tag = lock_state(&self.state)
            .snapshots
            .record(&canonical, &normalized, Some(&seen));

        let mut rendered = vec![format_hashline_header(&display_path, tag)];
        for (index, line) in all_lines
            .iter()
            .enumerate()
            .skip(offset)
            .take(end.saturating_sub(offset))
        {
            rendered.push(format_numbered_line(index as u64 + 1, line));
        }
        let remaining = line_count.saturating_sub(end);
        if remaining > 0 || offset > 0 {
            let shown_start = offset + 1;
            rendered.push(format!(
                "[Showing lines {shown_start}-{end} of {line_count} more lines available in file — Use :START-END to read a specific range]"
            ));
        }
        text_output(rendered.join("\n"))
    }
}

pub struct HashlineEditTool {
    pub state: SharedHashline,
}

fn render_section_result(result: &PatchSectionResult, snapshots: &mut SnapshotStore) -> String {
    let mut out = vec![result.header.clone()];
    let op = match result.op {
        SectionOp::Create => "created",
        SectionOp::Update => "updated",
        SectionOp::Delete => "deleted",
        SectionOp::Noop => "unchanged",
    };
    match (&result.move_dest, result.first_changed_line) {
        (Some(dest), _) => out.push(format!("{op}; moved to {dest}")),
        (None, Some(line)) => out.push(format!("{op}; first change at line {line}")),
        (None, None) => out.push(op.to_owned()),
    }
    if result.op != SectionOp::Delete {
        let lines: Vec<&str> = result.after.split('\n').collect();
        let total = u64::try_from(lines.len()).unwrap_or(u64::MAX);
        let mut seen: Vec<u64> = Vec::new();
        // Every hunk gets a window, not just the first. A window the model
        // cannot see is a line it cannot anchor to, which forced a full
        // re-read of the file after each multi-hunk edit.
        for changed in crate::diff::changed_after_lines(&result.before, &result.after) {
            let lo = changed.saturating_sub(EDIT_CONTEXT_LINES).max(1);
            let hi = changed.saturating_add(EDIT_CONTEXT_LINES).min(total);
            let start = seen
                .last()
                .map_or(lo, |last| lo.max(last.saturating_add(1)));
            for line_num in start..=hi {
                let index = usize::try_from(line_num.saturating_sub(1)).unwrap_or(usize::MAX);
                if let Some(text) = lines.get(index) {
                    out.push(format_numbered_line(line_num, text));
                    seen.push(line_num);
                }
            }
        }
        snapshots.record_seen_lines(&result.canonical_path, result.file_hash, &seen);
    }
    for warning in &result.warnings {
        out.push(format!("warning: {warning}"));
    }
    out.join("\n")
}

impl Tool for HashlineEditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        include_str!("prompt.md")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {"type": "string", "description": "Hashline patch: [path#TAG] section headers followed by PUT/CUT/REM/MV ops and +TEXT body rows"}
            },
            "required": ["patch"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Write
    }

    /// `prepare` is the dry-run half of the patcher's prepare/commit split: it
    /// validates and materializes the new text without touching disk. The
    /// clipboard is forked and the fork is dropped, so previewing a patch the
    /// user then denies leaves no register behind.
    fn preview(&self, input: &Map<String, Value>, cwd: &Path) -> Option<String> {
        let patch_text = input.get("patch").and_then(Value::as_str)?;
        let patch = Patch::parse(patch_text, Some(cwd)).ok()?;
        let mut state = lock_state(&self.state);
        let HashlineState {
            snapshots,
            clipboard,
            ..
        } = &mut *state;
        let mut scratch = super::clipboard::fork_clipboard(clipboard);
        let mut patcher = Patcher::new(snapshots, cwd.to_owned());
        let mut out = String::new();
        for section in &patch.sections {
            let Ok(prepared) = patcher.prepare(section, &mut scratch) else {
                continue;
            };
            if prepared.is_noop() {
                continue;
            }
            let (before, after) = prepared.diff_inputs();
            let resolved = resolve_path(&ToolContext::new(cwd.to_owned()), prepared.path());
            let diff = crate::diff::patch(before, after, &resolved);
            if diff.is_empty() {
                continue;
            }
            out.push_str(diff.as_str());
        }
        (!out.is_empty()).then_some(out)
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let patch_text = match require_str(&input, "patch") {
            Ok(patch) => patch.to_owned(),
            Err(message) => return error_output(message),
        };
        let patch = match Patch::parse(&patch_text, Some(&context.cwd)) {
            Ok(patch) => patch,
            Err(message) => return error_output(message),
        };
        if patch.sections.is_empty() {
            return error_output("Patch input did not produce any sections.");
        }
        let mut state = lock_state(&self.state);
        let HashlineState {
            snapshots,
            noop,
            clipboard,
        } = &mut *state;
        let mut host_clipboard = std::mem::take(clipboard);
        let mut patcher = Patcher::new(snapshots, context.cwd.clone());
        let results = patcher.apply(&patch, &mut host_clipboard);
        *clipboard = host_clipboard;
        let results = match results {
            Ok(results) => results,
            Err(message) => return error_output(message),
        };

        let hash = input_hash(&patch_text);
        let mut rendered: Vec<String> = Vec::new();
        for result in &results {
            if result.op == SectionOp::Noop {
                let entry = noop
                    .entry(result.canonical_path.clone())
                    .or_insert((hash, 0));
                if entry.0 == hash {
                    entry.1 += 1;
                } else {
                    *entry = (hash, 1);
                }
                if entry.1 >= NOOP_HARD_LIMIT {
                    return error_output(format!(
                        "Edit to {} was a byte-identical no-op {} times in a row. The file already contains this content — re-read it ({}) and issue a different edit, or stop editing.",
                        result.path, entry.1, result.header
                    ));
                }
                rendered.push(format!(
                    "{}\nno changes (the file already contains this content). Re-read the file before issuing another edit.",
                    result.header
                ));
                continue;
            }
            noop.remove(&result.canonical_path);
            rendered.push(render_section_result(result, snapshots));
        }
        text_output(rendered.join("\n\n"))
    }
}

pub fn record_write_snapshot(state: &SharedHashline, path: &Path, content: &str) {
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let normalized = normalize_to_lf(strip_bom(content).text);
    let line_count = normalized.split('\n').count() as u64;
    let seen: Vec<u64> = (1..=line_count).collect();
    lock_state(state)
        .snapshots
        .record(&canonical, &normalized, Some(&seen));
}
