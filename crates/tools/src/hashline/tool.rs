use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use super::format::{format_hashline_header, format_numbered_line};
use super::input::Patch;
use super::normalize::{normalize_to_lf, strip_bom};
use super::patcher::{PatchSectionResult, Patcher, SectionOp};
use super::snapshots::SnapshotStore;
use super::types::Clipboard;
use crate::diff::GitPatch;
use crate::tool::{
    Tool, ToolContext, ToolKind, ToolOutput, error_output, require_str, resolve_path, text_output,
};

const READ_LINE_CAP: usize = 2_000;
/// Model-facing byte budget: floor when no limit is asked, scaled with an
/// explicit one, ceilinged so a deliberate big read stays bounded.
const READ_BYTE_FLOOR: usize = 50 * 1024;
const READ_BYTE_CEIL: usize = 512 * 1024;
/// Matches the patcher's reveal clip rule: a clipped row never joins the
/// seen set, so an edit cannot anchor on text the model saw truncated.
const READ_LINE_CLIP: usize = 2_000;
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

pub(crate) fn lock_state(state: &SharedHashline) -> std::sync::MutexGuard<'_, HashlineState> {
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
        "Read a file. Output starts with a [path#TAG] snapshot header and numbered LINE:TEXT rows; use both to anchor edits. offset/limit select one range; ranges (e.g. [[10,40],[90,120]]) reads several windows in one call."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path (absolute, or relative to the working directory)"},
                "offset": {"type": "integer", "description": "1-based first line to read"},
                "limit": {"type": "integer", "description": "Max lines (default 2000; explicit values may exceed it, byte-budgeted)"},
                "ranges": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}, "description": "1-based inclusive [start, end] windows; exclusive with offset/limit"}
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
        let file_bytes = raw.len();
        let normalized = normalize_to_lf(strip_bom(&raw).text);
        let all_lines: Vec<&str> = normalized.split('\n').collect();
        let line_count = if normalized.ends_with('\n') {
            all_lines.len().saturating_sub(1)
        } else {
            all_lines.len()
        };
        let explicit_limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX));
        let windows = match read_windows(&input, explicit_limit, line_count) {
            Ok(windows) => windows,
            Err(output) => return *output,
        };
        // An explicit limit may exceed the default line cap; the byte budget
        // scales with it so a deliberate big read is allowed, a runaway not.
        let budget = explicit_limit.map_or(READ_BYTE_FLOOR, |limit| {
            READ_BYTE_FLOOR
                .max(limit.saturating_mul(256))
                .min(READ_BYTE_CEIL)
        });

        let mut rows: Vec<String> = Vec::new();
        let mut seen: Vec<u64> = Vec::new();
        let mut spent = 0_usize;
        let mut clipped: Vec<u64> = Vec::new();
        let mut byte_capped_at: Option<u64> = None;
        let mut previous_end: Option<usize> = None;
        let mut shown_end = 0_usize;
        'windows: for &(start, end) in &windows {
            if let Some(previous) = previous_end
                && start > previous + 1
            {
                rows.push(format!("[lines {}-{} not shown]", previous + 1, start - 1));
            }
            previous_end = Some(end);
            for index in (start - 1)..end {
                let Some(line) = all_lines.get(index) else {
                    continue;
                };
                let number = index as u64 + 1;
                if spent >= budget {
                    byte_capped_at = Some(number);
                    break 'windows;
                }
                let (text, was_clipped) = if line.chars().count() > READ_LINE_CLIP {
                    let cut: String = line.chars().take(READ_LINE_CLIP).collect();
                    (format!("{cut}\u{2026}"), true)
                } else {
                    ((*line).to_owned(), false)
                };
                spent = spent.saturating_add(text.len());
                rows.push(format_numbered_line(number, &text));
                if was_clipped {
                    clipped.push(number);
                } else {
                    seen.push(number);
                }
                shown_end = number as usize;
            }
        }

        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .to_string_lossy()
            .into_owned();
        let tag = lock_state(&self.state)
            .snapshots
            .record(&canonical, &normalized, Some(&seen));

        let empty = rows.is_empty();
        let mut rendered = vec![format_hashline_header(&display_path, tag)];
        rendered.extend(rows);
        let first_shown = windows.first().map_or(1, |(start, _)| *start);
        if empty && byte_capped_at.is_none() {
            rendered.push(format!(
                "[offset {first_shown} is beyond end of file ({line_count} lines)]"
            ));
        } else if let Some(line) = byte_capped_at {
            rendered.push(format!(
                "[byte budget {budget} reached at line {line} of {line_count} — continue with offset={line}]"
            ));
        } else if shown_end < line_count && windows.len() == 1 {
            rendered.push(format!(
                "[showing lines {first_shown}-{shown_end} of {line_count} — continue with offset={}]",
                shown_end + 1
            ));
        } else if first_shown > 1 && shown_end >= line_count {
            rendered.push(format!(
                "[showing lines {first_shown}-{shown_end} — end of file ({line_count} lines)]"
            ));
        }
        if let Some(first) = clipped.first() {
            rendered.push(format!(
                "[{} line(s) exceeded {READ_LINE_CLIP} chars and were clipped (never edit-anchors) — full line: bash: sed -n '{first}p' {display_path}]",
                clipped.len()
            ));
        }
        let mut output = text_output(rendered.join("\n"));
        output.result.details = json!({
            "fileBytes": file_bytes,
            "lines": line_count,
            "shownLines": seen.len().saturating_add(clipped.len()),
            "clippedLines": clipped.len(),
            "byteCapped": byte_capped_at.is_some(),
            "windows": windows.len(),
        });
        output
    }
}

/// The requested line windows, 1-based inclusive, merged and clamped.
/// `ranges` and `offset`/`limit` are two spellings of the same thing and
/// never combine.
fn read_windows(
    input: &Map<String, Value>,
    explicit_limit: Option<usize>,
    line_count: usize,
) -> Result<Vec<(usize, usize)>, Box<ToolOutput>> {
    let invalid = |message: String| {
        Err(Box::new(crate::tool::error_output_kind(
            message,
            "invalid_args",
        )))
    };
    if let Some(ranges) = input.get("ranges") {
        if input.get("offset").is_some() || explicit_limit.is_some() {
            return invalid("pass ranges or offset/limit, not both".to_owned());
        }
        let Some(items) = ranges.as_array() else {
            return invalid("ranges must be an array of [start, end] pairs".to_owned());
        };
        let mut windows: Vec<(usize, usize)> = Vec::new();
        for item in items {
            let pair: Option<(u64, u64)> = item.as_array().and_then(|pair| {
                match (
                    pair.first().and_then(Value::as_u64),
                    pair.get(1).and_then(Value::as_u64),
                ) {
                    (Some(start), Some(end)) if pair.len() == 2 => Some((start, end)),
                    _ => None,
                }
            });
            let Some((start, end)) = pair else {
                return invalid(format!(
                    "range {item} is not a [start, end] pair of positive integers"
                ));
            };
            if start < 1 || end < start {
                return invalid(format!(
                    "range [{start}, {end}] is not ascending and 1-based"
                ));
            }
            let start = usize::try_from(start).unwrap_or(usize::MAX);
            let end = usize::try_from(end).unwrap_or(usize::MAX).min(line_count);
            if start > line_count {
                return invalid(format!(
                    "range start {start} is beyond the file ({line_count} lines)"
                ));
            }
            windows.push((start, end));
        }
        if windows.is_empty() {
            return invalid("ranges is empty".to_owned());
        }
        windows.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in windows {
            match merged.last_mut() {
                Some((_, previous_end)) if start <= previous_end.saturating_add(1) => {
                    *previous_end = (*previous_end).max(end);
                }
                _ => merged.push((start, end)),
            }
        }
        return Ok(merged);
    }
    let offset = input
        .get("offset")
        .and_then(Value::as_u64)
        .map_or(0, |line| line.saturating_sub(1) as usize);
    let limit = explicit_limit.unwrap_or(READ_LINE_CAP);
    let end = offset.saturating_add(limit).min(line_count);
    Ok(vec![(offset + 1, end)])
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

    /// The patch language as a Lark grammar (ported from the omp donor); on
    /// providers with custom tools the body rows stop paying JSON escaping.
    fn freeform(&self) -> Option<yi_types::model::FreeformFormat> {
        Some(yi_types::model::FreeformFormat {
            syntax: "lark".to_owned(),
            definition: include_str!("grammar.lark").to_owned(),
        })
    }

    /// [`Patcher::prepare`] validates and materializes the new text without touching disk.
    /// The clipboard is forked and the fork dropped, so previewing a patch the
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
            Err(message) => return crate::tool::error_output_kind(message, "invalid_args"),
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
            Err(message) => {
                let kind = if message.starts_with(super::mismatch::EDIT_REJECTED_PREFIX) {
                    "stale_tag"
                } else {
                    "tool_error"
                };
                return crate::tool::error_output_kind(message, kind);
            }
        };

        let hash = input_hash(&patch_text);
        let mut rendered: Vec<String> = Vec::new();
        let mut diff = String::new();
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
                    return crate::tool::error_output_kind(
                        format!(
                            "Edit to {} was a byte-identical no-op {} times in a row. The file already contains this content — re-read it ({}) and issue a different edit, or stop editing.",
                            result.path, entry.1, result.header
                        ),
                        "noop_loop",
                    );
                }
                rendered.push(format!(
                    "{}\nno changes (the file already contains this content). Re-read the file before issuing another edit.",
                    result.header
                ));
                continue;
            }
            noop.remove(&result.canonical_path);
            diff.push_str(
                crate::diff::patch(
                    &result.before,
                    &result.after,
                    Path::new(&result.canonical_path),
                )
                .as_str(),
            );
            rendered.push(render_section_result(result, snapshots));
        }
        let mut output = text_output(rendered.join("\n\n"));
        if !diff.is_empty() {
            output.result.details = crate::diff::patch_details(&GitPatch::from_text(diff));
        }
        // The op mix is what `yi stats` aggregates for the M3 register/move
        // economics question; counts, never content.
        if let Value::Object(details) = &mut output.result.details {
            let count = |op: SectionOp| results.iter().filter(|result| result.op == op).count();
            details.insert(
                "ops".to_owned(),
                json!({
                    "updated": count(SectionOp::Update),
                    "created": count(SectionOp::Create),
                    "deleted": count(SectionOp::Delete),
                    "noop": count(SectionOp::Noop),
                    "moved": results.iter().filter(|result| result.move_dest.is_some()).count(),
                }),
            );
        }
        output
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
