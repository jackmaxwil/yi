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
/// The patcher's reveal clip, shared: a row read wider than a rejection can
/// re-show would anchor blind. A clipped row never joins the seen set.
const READ_LINE_CLIP: usize = super::patcher::SEEN_LINE_REVEAL_MAX_COLUMNS;
/// Incident: 182 byte-identical no-op edits on one path in 205 calls before an
/// abort; this many consecutive no-ops now escalate the soft hint to an error.
const NOOP_HARD_LIMIT: u32 = 3;
const EDIT_CONTEXT_LINES: u64 = 3;

#[derive(Default)]
pub struct HashlineState {
    pub snapshots: SnapshotStore,
    noop: std::collections::HashMap<String, (u64, u32)>,
    clipboard: Clipboard,
    pub documents: Option<crate::Documents>,
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

const READ_DESCRIPTION: &str = "Read a file (tagged [path#TAG] header, LINE:TEXT rows that anchor edits), a directory (listing plus skeletons), or a glob (every match). find=\"text\" shows the block around the first match plus its references. A capped read ends with the file's skeleton. offset/limit or ranges ([[10,40],[90,120]]) pick windows. Do not re-read a file you just edited: the edit result carries the new anchors, and a failed edit says so. Prefer one read of a large range to many small ones.";

pub struct HashlineReadTool {
    pub state: SharedHashline,
    description: String,
}

impl HashlineReadTool {
    pub fn new(state: SharedHashline) -> Self {
        let description =
            crate::document::describe(READ_DESCRIPTION, lock_state(&state).documents.as_ref());
        Self { state, description }
    }
}

pub(crate) fn documents(state: &SharedHashline) -> Option<crate::Documents> {
    lock_state(state).documents.clone()
}

impl Tool for HashlineReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File, directory, or glob such as src/**/*.rs (relative to the working directory, or absolute)"},
                "find": {"type": "string", "description": "Show the block around the first line containing this text, then its references; exclusive with offset/ranges"},
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
        if display_path.contains(['*', '?', '[']) {
            return self.read_glob(&display_path, context);
        }
        let path = resolve_path(context, &display_path);
        if path.is_dir() {
            return read_dir(&display_path, &path);
        }
        self.read_file(&display_path, &path, &input, context)
    }
}

struct Found {
    window: (usize, usize),
    index: usize,
    block: bool,
    identifier: Option<String>,
}

const FIND_CONTEXT: usize = 20;
const NEAR_MISSES: usize = 5;
const REFS_CAP: usize = 20;
const SKELETON_ROWS: usize = 40;
const GLOB_FILES_CAP: usize = 200;
const DIR_HEADS_PER_FILE: usize = 8;
const GLOB_HEADS_PER_FILE: usize = 12;

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn identifier_in(needle: &str, line: &str) -> Option<String> {
    needle
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|word| word.len() >= 3 && !word.as_bytes().first().is_some_and(u8::is_ascii_digit))
        .filter(|word| {
            line.match_indices(word).any(|(at, _)| {
                let before = at
                    .checked_sub(1)
                    .and_then(|index| line.as_bytes().get(index));
                let after = line.as_bytes().get(at.saturating_add(word.len()));
                !before.is_some_and(|byte| is_word(*byte))
                    && !after.is_some_and(|byte| is_word(*byte))
            })
        })
        .max_by_key(|word| word.len())
        .map(str::to_owned)
}

/// The first line holding `needle`, widened to its block when the file's language has one,
/// else to a fixed window; no hit lists the nearest lines by the needle's longest word.
fn locate(
    display_path: &str,
    normalized: &str,
    lines: &[&str],
    needle: &str,
) -> Result<Found, Box<ToolOutput>> {
    let invalid = |message: String| {
        Box::new(crate::tool::error_output_kind(
            message,
            yi_types::event::ToolErrorKind::InvalidArgs,
        ))
    };
    let Some(index) = lines.iter().position(|line| line.contains(needle)) else {
        // Nearest by the longest word's stem, so a plural or a typo still lands nearby.
        let word = needle
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .max_by_key(|word| word.len())
            .unwrap_or(needle)
            .to_ascii_lowercase();
        let stem: String = word
            .chars()
            .take(word.len().saturating_sub(2).max(3))
            .collect();
        let candidates: Vec<(usize, &&str)> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| !stem.is_empty() && line.to_ascii_lowercase().contains(&stem))
            .collect();
        let near: Vec<String> = candidates
            .iter()
            .take(NEAR_MISSES)
            .map(|(index, line)| format_numbered_line(index.saturating_add(1) as u64, line.trim()))
            .collect();
        let mut message = format!("no line of {display_path} contains {needle:?}");
        if !near.is_empty() {
            message.push_str(&format!(
                "; nearest by {stem:?} ({} of {} lines):\n{}",
                near.len(),
                candidates.len(),
                near.join("\n")
            ));
        }
        return Err(invalid(message));
    };
    let line = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
    let span = super::patcher::block_resolver(&super::types::BlockResolverRequest {
        path: display_path,
        text: normalized,
        line,
    });
    let window = match span {
        Some(span) => (
            usize::try_from(span.start).unwrap_or(index + 1),
            usize::try_from(span.end)
                .unwrap_or(index + 1)
                .min(lines.len()),
        ),
        None => (
            index.saturating_sub(FIND_CONTEXT).saturating_add(1),
            index
                .saturating_add(FIND_CONTEXT)
                .saturating_add(1)
                .min(lines.len()),
        ),
    };
    Ok(Found {
        window,
        index,
        block: span.is_some(),
        identifier: lines
            .get(index)
            .and_then(|line| identifier_in(needle, line)),
    })
}

fn skeleton_rows(text: &str) -> Vec<String> {
    let heads: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| crate::orient::is_decl(line) && !line.starts_with(char::is_whitespace))
        .collect();
    if heads.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![if heads.len() > SKELETON_ROWS {
        format!(
            "[skeleton: first {SKELETON_ROWS} of {} top-level declarations — grep def=true for all]",
            heads.len()
        )
    } else {
        format!("[skeleton: {} top-level declarations]", heads.len())
    }];
    rows.extend(heads.iter().take(SKELETON_ROWS).map(|(index, line)| {
        format!(
            "  {}  {}",
            index.saturating_add(1),
            crate::orient::decl_head(line)
        )
    }));
    rows
}

fn skeleton_heads(text: &str, cap: usize) -> (Vec<String>, usize) {
    let all = crate::orient::skeleton_of(text, usize::MAX);
    let total = all.len();
    let mut heads: Vec<String> = all.into_iter().take(cap).collect();
    if total > cap {
        heads.push(format!("… +{} more", total.saturating_sub(cap)));
    }
    (heads, total)
}

fn read_dir(display_path: &str, path: &Path) -> ToolOutput {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => return error_output(format!("failed to read {}: {error}", path.display())),
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, u64)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        match entry.metadata() {
            Ok(meta) if meta.is_dir() => dirs.push(name),
            Ok(meta) => files.push((name, meta.len())),
            Err(_) => files.push((name, 0)),
        }
    }
    dirs.sort();
    files.sort();
    let mut rows = vec![format!("[{}]", display_path.trim_end_matches('/'))];
    rows.extend(dirs.iter().map(|name| format!("{name}/")));
    rows.extend(
        files
            .iter()
            .map(|(name, bytes)| format!("{name}  {bytes} B")),
    );
    let mut skeleton: Vec<String> = Vec::new();
    let mut source_files = 0_usize;
    for (name, _) in &files {
        let source = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| crate::orient::SKELETON_EXTS.contains(&extension));
        if !source {
            continue;
        }
        source_files = source_files.saturating_add(1);
        if skeleton.len() >= SKELETON_ROWS {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path.join(name)) else {
            continue;
        };
        let (heads, _) = skeleton_heads(&text, DIR_HEADS_PER_FILE);
        if !heads.is_empty() {
            skeleton.push(format!("{name}: {}", heads.join("; ")));
        }
    }
    if !skeleton.is_empty() {
        rows.push(format!(
            "[skeleton: {} of {source_files} source files, up to {DIR_HEADS_PER_FILE} heads each — read a file for the rest]",
            skeleton.len()
        ));
        rows.extend(skeleton);
    }
    let mut output = text_output(rows.join("\n"));
    output.result.details = json!({ "dirs": dirs.len(), "files": files.len() });
    output
}

impl HashlineReadTool {
    /// Every file the glob names: whole and tagged while the byte budget lasts, a skeleton
    /// after it, so one call shows a module's shape without a second.
    fn read_glob(&self, pattern: &str, context: &ToolContext) -> ToolOutput {
        let matcher = match globset::GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
        {
            Ok(glob) => glob.compile_matcher(),
            Err(error) => return error_output(format!("invalid glob {pattern:?}: {error}")),
        };
        let root = context.cwd.clone();
        let mut matches: Vec<std::path::PathBuf> = Vec::new();
        crate::builtins::walk_files(&root, &mut |path| {
            let relative = path.strip_prefix(&root).unwrap_or(path);
            if matcher.is_match(relative) {
                matches.push(path.to_path_buf());
            }
            matches.len() < GLOB_FILES_CAP
        });
        matches.sort();
        if matches.is_empty() {
            return error_output(format!("no file matches {pattern:?}"));
        }
        let mut sections = vec![if matches.len() >= GLOB_FILES_CAP {
            format!(
                "[first {GLOB_FILES_CAP} files matching {pattern}; the walk stopped there — narrow the glob for the rest]"
            )
        } else {
            format!("[{} files match {pattern}]", matches.len())
        }];
        let mut spent = 0_usize;
        let mut whole = 0_usize;
        let mut budget_named = false;
        for path in &matches {
            let display = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            let size = std::fs::metadata(path)
                .map_or(0, |meta| usize::try_from(meta.len()).unwrap_or(usize::MAX));
            if spent.saturating_add(size) <= READ_BYTE_FLOOR {
                let output = self.read_file(&display, path, &Map::new(), context);
                spent = spent.saturating_add(size);
                whole = whole.saturating_add(1);
                sections.push(output_text_of(&output));
                continue;
            }
            if !budget_named {
                budget_named = true;
                sections.push(format!(
                    "[byte budget {READ_BYTE_FLOOR} reached after {whole} whole files; the rest are skeletons — read a file for its text]"
                ));
            }
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let (heads, total) = skeleton_heads(&text, GLOB_HEADS_PER_FILE);
            let mut rows = vec![format!(
                "{display}  ({} lines, {total} declarations, skeleton)",
                text.lines().count()
            )];
            rows.extend(heads.iter().map(|head| format!("  {head}")));
            sections.push(rows.join("\n"));
        }
        let mut output = text_output(sections.join("\n\n"));
        output.result.details = json!({ "files": matches.len(), "whole": whole });
        output
    }

    fn read_file(
        &self,
        display_path: &str,
        path: &Path,
        input: &Map<String, Value>,
        context: &ToolContext,
    ) -> ToolOutput {
        let failed = |error: std::io::Error| format!("failed to read {}: {error}", path.display());
        match std::fs::read_to_string(path) {
            Ok(raw) if !raw.starts_with("{\\rtf") => {
                self.render_file(display_path, path, &raw, None, input, context)
            }
            Ok(raw) => self.read_document(display_path, path, Ok(raw), input, context),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                self.read_document(display_path, path, Err(failed(error)), input, context)
            }
            Err(error) => error_output(failed(error)),
        }
    }

    /// The converter, never an extension list, decides what is a document. RTF is the one kind
    /// written as 7-bit text, so its marker offers it; `plain` is what shows if nothing converts.
    fn read_document(
        &self,
        display_path: &str,
        path: &Path,
        plain: Result<String, String>,
        input: &Map<String, Value>,
        context: &ToolContext,
    ) -> ToolOutput {
        let unconverted = |line: Option<String>| match &plain {
            Ok(raw) => self.render_file(display_path, path, raw, None, input, context),
            Err(failed) => error_output(
                line.map_or_else(|| failed.clone(), |line| format!("{failed}\n{line}")),
            ),
        };
        let Some(documents) = documents(&self.state) else {
            return unconverted(None);
        };
        let copy = match crate::document::convert(&documents, path, &context.cancelled) {
            crate::document::Converted::Markdown(copy) => copy,
            crate::document::Converted::NotADocument => return unconverted(None),
            crate::document::Converted::Unavailable(line) => return unconverted(Some(line)),
            crate::document::Converted::Refused(_) if plain.is_ok() => return unconverted(None),
            crate::document::Converted::Refused(reason) => {
                return error_output(format!("failed to read {}: {reason}", path.display()));
            }
        };
        let raw = match std::fs::read_to_string(&copy) {
            Ok(raw) => raw,
            Err(error) => {
                return error_output(format!("failed to read {}: {error}", copy.display()));
            }
        };
        let note = format!(
            "[{display_path} converted to Markdown; editing this copy does not change {display_path}]"
        );
        let mut output = self.render_file(
            &copy.to_string_lossy(),
            &copy,
            &raw,
            Some(note),
            input,
            context,
        );
        if let Some(details) = output.result.details.as_object_mut() {
            let extension = path
                .extension()
                .map(|ext| ext.to_string_lossy().into_owned());
            details.insert("convertedFrom".to_owned(), json!(extension));
        }
        output
    }

    fn render_file(
        &self,
        display_path: &str,
        path: &Path,
        raw: &str,
        note: Option<String>,
        input: &Map<String, Value>,
        context: &ToolContext,
    ) -> ToolOutput {
        let file_bytes = raw.len();
        let normalized = normalize_to_lf(strip_bom(raw).text);
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
        let explicit_window = explicit_limit.is_some()
            || input.get("offset").is_some()
            || input.get("ranges").is_some();
        let needle = input.get("find").and_then(Value::as_str);
        let found = match needle {
            Some(_) if explicit_window => {
                return crate::tool::error_output_kind(
                    "pass find or offset/limit/ranges, not both".to_owned(),
                    yi_types::event::ToolErrorKind::InvalidArgs,
                );
            }
            Some(needle) => {
                match locate(display_path, &normalized, &all_lines[..line_count], needle) {
                    Ok(found) => Some(found),
                    Err(output) => return *output,
                }
            }
            None => None,
        };
        let windows = match &found {
            Some(found) => vec![found.window],
            None => match read_windows(input, explicit_limit, line_count) {
                Ok(windows) => windows,
                Err(output) => return *output,
            },
        };
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
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .into_owned();
        let tag = lock_state(&self.state)
            .snapshots
            .record(&canonical, &normalized, Some(&seen));

        let empty = rows.is_empty();
        let mut rendered = vec![format_hashline_header(display_path, tag)];
        rendered.extend(note);
        if let Some(found) = &found {
            let (lo, hi) = found.window;
            rendered.push(if found.block {
                format!("[find: block at lines {lo}-{hi} of {line_count}]")
            } else {
                format!(
                    "[find: no block resolver for this file; lines {lo}-{hi} of {line_count} around the hit at line {} — ranges= for more]",
                    found.index.saturating_add(1)
                )
            });
        }
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
        } else if shown_end < line_count && found.is_none() {
            rendered.push(format!(
                "[showing lines {first_shown}-{shown_end} of {line_count} — continue with offset={}]",
                shown_end + 1
            ));
        } else if first_shown > 1 && shown_end >= line_count && found.is_none() {
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
        let capped = byte_capped_at.is_some()
            || (!explicit_window && found.is_none() && shown_end < line_count);
        if capped {
            rendered.extend(skeleton_rows(&normalized));
        }
        let mut refs_total = 0_usize;
        if let Some(Found {
            identifier: Some(identifier),
            index,
            ..
        }) = &found
        {
            let grep = crate::grep::GrepTool {
                hashline: Some(Arc::clone(&self.state)),
            };
            let (refs, total) = grep.references(
                &context.cwd,
                identifier,
                Some((&canonical, *index)),
                REFS_CAP,
            );
            refs_total = total;
            rendered.push(format!(
                "[refs: {} of {total} for {identifier}]",
                total.min(REFS_CAP)
            ));
            rendered.extend(refs);
            if total > REFS_CAP {
                rendered.push(format!("[the rest: grep \"\\b{identifier}\\b\"]"));
            }
        }
        let mut output = text_output(rendered.join("\n"));
        output.result.details = json!({
            "fileBytes": file_bytes,
            "lines": line_count,
            "shownLines": seen.len().saturating_add(clipped.len()),
            "clippedLines": clipped.len(),
            "byteCapped": byte_capped_at.is_some(),
            "windows": windows.len(),
            "skeleton": capped,
            "refs": refs_total,
        });
        output
    }
}

fn output_text_of(output: &ToolOutput) -> String {
    output
        .result
        .content
        .iter()
        .map(|content| match content {
            yi_types::message::Content::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The requested line windows, 1-based inclusive, merged and clamped. `ranges` and
/// `offset`/`limit` are two spellings of the same thing and never combine.
fn read_windows(
    input: &Map<String, Value>,
    explicit_limit: Option<usize>,
    line_count: usize,
) -> Result<Vec<(usize, usize)>, Box<ToolOutput>> {
    let invalid = |message: String| {
        Err(Box::new(crate::tool::error_output_kind(
            message,
            yi_types::event::ToolErrorKind::InvalidArgs,
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
    /// `edit.freeformGrammar`; the JSON argument is the default.
    pub freeform_grammar: bool,
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
        // Every hunk gets a window, not just the first: a window the model cannot see is a
        // line it cannot anchor to, forcing a full re-read after each multi-hunk edit.
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

    /// The patch language as a Lark grammar; on providers with custom tools
    /// the body rows stop paying JSON escaping.
    fn freeform(&self) -> Option<yi_types::model::FreeformFormat> {
        self.freeform_grammar
            .then(|| yi_types::model::FreeformFormat {
                definition: include_str!("grammar.lark").to_owned(),
            })
    }

    /// [`Patcher::prepare`] validates and materializes the new text without touching disk.
    /// The clipboard fork is dropped, so previewing a denied patch leaves no register.
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
            Err(message) => {
                return crate::tool::error_output_kind(
                    message,
                    yi_types::event::ToolErrorKind::InvalidArgs,
                );
            }
        };
        if patch.sections.is_empty() {
            return error_output("Patch input did not produce any sections.");
        }
        let documents = documents(&self.state);
        if let Some(refusal) = patch.sections.iter().find_map(|section| {
            crate::document::source_refusal(
                documents.as_ref(),
                &resolve_path(context, &section.path),
            )
        }) {
            return error_output(refusal);
        }
        let mut state = lock_state(&self.state);
        let HashlineState {
            snapshots,
            noop,
            clipboard,
            ..
        } = &mut *state;
        let mut host_clipboard = std::mem::take(clipboard);
        let mut patcher = Patcher::new(snapshots, context.cwd.clone());
        let results = patcher.apply(&patch, &mut host_clipboard);
        *clipboard = host_clipboard;
        let results = match results {
            Ok(results) => results,
            Err(message) => {
                let kind = if message.starts_with(super::mismatch::EDIT_REJECTED_PREFIX) {
                    yi_types::event::ToolErrorKind::StaleTag
                } else {
                    yi_types::event::ToolErrorKind::ToolError
                };
                return crate::tool::error_output_kind(message, kind);
            }
        };

        let hash = input_hash(&patch_text);
        let mut rendered: Vec<String> = Vec::new();
        let mut diff = String::new();
        let mut syntax: Option<String> = None;
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
                        yi_types::event::ToolErrorKind::NoopLoop,
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
            let mut section = render_section_result(result, snapshots);
            if result.op != SectionOp::Delete
                && let Some(line) = crate::syntax::verdict(Path::new(&result.canonical_path))
            {
                section.push('\n');
                section.push_str(&line);
                if syntax.as_deref().is_none_or(|kept| kept == "syntax: ok") {
                    syntax = Some(line);
                }
            }
            rendered.push(section);
        }
        let charted = results.iter().any(|result| {
            result.op == SectionOp::Update
                && Path::new(&result.canonical_path)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| matches!(extension, "rs" | "py"))
        });
        let grid = if charted {
            let layer = grid_check(context);
            rendered.push(layer.render());
            layer.name()
        } else {
            "skipped"
        };
        let mut output = text_output(rendered.join("\n\n"));
        if !diff.is_empty() {
            output.result.details = crate::diff::patch_details(&GitPatch::from_text(diff));
        }
        if let Value::Object(details) = &mut output.result.details {
            details.insert("grid".to_owned(), Value::String(grid.to_owned()));
            details.insert(
                "syntax".to_owned(),
                syntax.map_or(Value::Null, Value::String),
            );
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

/// What `grid check --quick` said after an edit to a charted file; an absent grid is a
/// named header, never a failed edit.
enum GridLayer {
    Clean,
    Findings(String),
    Unavailable(Unavailable),
}

enum Unavailable {
    NoBinary,
    Timeout { ms: u64 },
    Exit { code: i32 },
}

const GRID_CHECK_TIMEOUT_MS: u64 = 2_000;
const GRID_CHECK_LINES: usize = 40;

impl GridLayer {
    fn name(&self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Findings(_) => "findings",
            Self::Unavailable(Unavailable::NoBinary) => "no-binary",
            Self::Unavailable(Unavailable::Timeout { .. }) => "timeout",
            Self::Unavailable(Unavailable::Exit { .. }) => "exit",
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Clean => "[grid check: clean]".to_owned(),
            Self::Findings(text) => format!("[grid check]\n{text}"),
            Self::Unavailable(Unavailable::NoBinary) => {
                "[grid check: unavailable — no grid binary]".to_owned()
            }
            Self::Unavailable(Unavailable::Timeout { ms }) => {
                format!("[grid check: unavailable — no answer in {ms} ms]")
            }
            Self::Unavailable(Unavailable::Exit { code }) => {
                format!("[grid check: unavailable — exit {code}]")
            }
        }
    }
}

/// Exit 3 is grid's finding; anything else means the layer is absent.
fn grid_check(context: &ToolContext) -> GridLayer {
    let mut spawn = crate::process::command("grid");
    spawn.args(["check", "--quick"]).current_dir(&context.cwd);
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(GRID_CHECK_TIMEOUT_MS);
    let parent = Arc::clone(&context.cancelled);
    let cancelled: crate::tool::CancelFlag =
        Arc::new(move || parent() || std::time::Instant::now() > deadline);
    let capture =
        match crate::process::run_captured(spawn, None, &cancelled, crate::process::OUTPUT_CAP) {
            Ok(capture) => capture,
            Err(_) => return GridLayer::Unavailable(Unavailable::NoBinary),
        };
    if capture.cancelled && !context.cancelled.as_ref()() {
        return GridLayer::Unavailable(Unavailable::Timeout {
            ms: GRID_CHECK_TIMEOUT_MS,
        });
    }
    match capture.exit_code {
        Some(0) => GridLayer::Clean,
        Some(3) => {
            let all: Vec<&str> = capture
                .stdout
                .lines()
                .chain(capture.stderr.lines())
                .filter(|line| !line.trim().is_empty())
                .collect();
            let mut text: Vec<String> = all
                .iter()
                .take(GRID_CHECK_LINES)
                .map(|line| (*line).to_owned())
                .collect();
            if all.len() > GRID_CHECK_LINES {
                text.push(format!(
                    "[grid check: first {GRID_CHECK_LINES} of {} lines — bash: grid check --quick for all]",
                    all.len()
                ));
            }
            GridLayer::Findings(text.join("\n"))
        }
        Some(code) => GridLayer::Unavailable(Unavailable::Exit { code }),
        None => GridLayer::Unavailable(Unavailable::NoBinary),
    }
}

pub fn record_write_snapshot(state: &SharedHashline, path: &Path, content: &str) {
    let line_count = u64::try_from(content.split('\n').count()).unwrap_or(u64::MAX);
    let seen: Vec<u64> = (1..=line_count).collect();
    record_view_snapshot(state, path, content, &seen);
}

/// A view of `content` the model has just seen through some other tool: the rows in `seen`
/// may anchor an edit, the rest stay behind the seen-lines guard.
pub fn record_view_snapshot(
    state: &SharedHashline,
    path: &Path,
    content: &str,
    seen: &[u64],
) -> super::format::FileTag {
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let normalized = normalize_to_lf(strip_bom(content).text);
    lock_state(state)
        .snapshots
        .record(&canonical, &normalized, Some(seen))
}
