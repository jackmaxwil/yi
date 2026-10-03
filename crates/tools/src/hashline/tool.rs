use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value, json};

use super::format::{format_hashline_header, format_numbered_line};
use super::input::Patch;
use super::normalize::{normalize_to_lf, strip_bom};
use super::patcher::{PatchSectionResult, Patcher, SectionOp};
use super::snapshots::SnapshotStore;
use super::types::Clipboard;
use crate::diff::GitPatch;
use crate::ripwire::CheckLayer;
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
    pub(crate) grep_sweeps: std::collections::HashMap<String, u64>,
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

const READ_DESCRIPTION: &str = "Read a file (tagged [path#TAG] header, LINE:TEXT rows that anchor edits), a directory (listing plus skeletons), or a glob (every match). find=\"text\" shows the block around the first match; when that line defines a name, the references to it in files of the same type follow. A capped read ends with the file's skeleton. offset/limit or ranges ([[10,40],[90,120]]) pick windows. Do not re-read a file you just edited: the edit result carries the new anchors, and a failed edit says so. Prefer one read of a large range to many small ones.";

/// How one text is shown. `on_disk`: the text is the file itself, so clipping guards its anchors
/// and code refs apply; a document's copy is neither, and gets a shorter `first_look`.
pub(super) struct View<'a> {
    pub resolve_as: &'a str,
    pub note: Option<String>,
    pub on_disk: bool,
    pub first_look: usize,
}

impl<'a> View<'a> {
    pub(super) fn source(path: &'a str) -> Self {
        Self {
            resolve_as: path,
            note: None,
            on_disk: true,
            first_look: READ_BYTE_FLOOR,
        }
    }
}

pub struct HashlineReadTool {
    pub state: SharedHashline,
    /// Invariant: decided the first time the session sends its tools, then held, so a venv
    /// landing mid-session never rewrites the tool table and the cached prefix behind it.
    description: OnceLock<String>,
}

impl HashlineReadTool {
    pub fn new(state: SharedHashline) -> Self {
        Self {
            state,
            description: OnceLock::new(),
        }
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
        self.description.get_or_init(|| {
            let formats = documents(&self.state)
                .map_or_else(Vec::new, |documents| (documents.converter)().formats);
            crate::document::describe(READ_DESCRIPTION, &formats)
        })
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File, directory, or glob such as src/**/*.rs (relative to the working directory, or absolute)"},
                "find": {"type": "string", "description": "Show the block around the first line containing this text, then, for a definition, its references; exclusive with offset/ranges"},
                "offset": {"type": "integer", "description": "1-based first line to read"},
                "limit": {"type": "integer", "description": "Max lines (default 2000; explicit values may exceed it, byte-budgeted)"},
                "ranges": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}, "description": "1-based inclusive [start, end] windows; exclusive with offset/limit"},
                "pages": {"type": "string", "description": "PDF only: the 1-based pages to convert, such as \"3-5,9\"; the rest are left out"}
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
            return read_dir(&display_path, &path, context);
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
pub(super) const SKELETON_ROWS: usize = 40;
const DIR_ROWS: usize = 200;
const GLOB_FILES_CAP: usize = 200;
const DIR_HEADS_PER_FILE: usize = 8;
const GLOB_HEADS_PER_FILE: usize = 12;

/// The first line holding `needle`, widened to its block when the file's language has one,
/// else to a fixed window; no hit lists the nearest lines by the needle's longest word.
fn locate(
    display_path: &str,
    resolve_as: &str,
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
    let folded = fold_typography(needle);
    let Some(index) = lines
        .iter()
        .position(|line| line.contains(needle) || fold_typography(line).contains(&folded))
    else {
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
        path: resolve_as,
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
            .and_then(|line| crate::orient::defined_name(line)),
    })
}

/// Documents set ’ “ – where a model types ' " -, so `find` compares the two spellings as one.
fn fold_typography(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{2032}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' | '\u{2033}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{a0}' | '\u{202f}' => ' ',
            other => other,
        })
        .collect()
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

fn read_dir(display_path: &str, path: &Path, context: &ToolContext) -> ToolOutput {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => return error_output(format!("failed to read {}: {error}", path.display())),
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, u64)> = Vec::new();
    let mut walled = 0_usize;
    for entry in entries.flatten() {
        if crate::builtins::walled(&context.deny_read, &entry.path()) {
            walled = walled.saturating_add(1);
            continue;
        }
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
    let entries = dirs.len().saturating_add(files.len());
    let listed = dirs.iter().map(|name| format!("{name}/"));
    let listed = listed.chain(
        files
            .iter()
            .map(|(name, bytes)| format!("{name}  {bytes} B")),
    );
    rows.extend(listed.take(DIR_ROWS));
    if entries > DIR_ROWS {
        rows.push(format!(
            "[showing {DIR_ROWS} of {entries} entries — read a narrower path]"
        ));
    }
    let mut skeleton: Vec<String> = Vec::new();
    let mut source_files = 0_usize;
    let gate = context.read_gate();
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
        let opened = gate.open(&path.join(name), &context.deny_read);
        let Ok(text) = opened.and_then(std::io::read_to_string) else {
            continue;
        };
        let (heads, _) = crate::orient::skeleton(&text, DIR_HEADS_PER_FILE);
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
    rows.extend(
        crate::builtins::Walked {
            walled,
            ..Default::default()
        }
        .notices(),
    );
    let mut output = text_output(rows.join("\n"));
    output.result.details = json!({ "dirs": dirs.len(), "files": files.len() });
    output
}

impl HashlineReadTool {
    /// Every file the glob names: whole and tagged while the byte budget lasts, a skeleton
    /// after it, so one call shows a module's shape without a second.
    fn read_glob(&self, pattern: &str, context: &ToolContext) -> ToolOutput {
        let glob = match crate::tool::rooted_glob(pattern, &context.cwd) {
            Ok(glob) => glob,
            Err(error) => return error_output(format!("invalid glob {pattern:?}: {error}")),
        };
        let root = context.cwd.clone();
        let mut matches: Vec<std::path::PathBuf> = Vec::new();
        let walled = crate::builtins::walk_files(&glob.base, &context.deny_read, &mut |path| {
            if glob.matches(path) {
                matches.push(path.to_path_buf());
            }
            matches.len() < GLOB_FILES_CAP && !(context.cancelled)()
        });
        matches.sort();
        if matches.is_empty() {
            let notices = walled.notices().into_iter();
            let rows: Vec<String> = [format!("no file matches {pattern:?}")]
                .into_iter()
                .chain(notices)
                .collect();
            return error_output(rows.join("\n"));
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
        let gate = context.read_gate();
        for path in &matches {
            let display = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            let opened = gate.open(path, &context.deny_read);
            let bytes = opened.and_then(crate::tool::read_all).unwrap_or_default();
            let document = self.listed_document(path, &bytes, spent < READ_BYTE_FLOOR, context);
            let size = match &document {
                Some(super::documents::Listed::Copy(copy)) => std::fs::metadata(&copy.path)
                    .map_or(0, |meta| usize::try_from(meta.len()).unwrap_or(usize::MAX)),
                Some(_) => usize::MAX,
                None => bytes.len(),
            };
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
                    "[byte budget {READ_BYTE_FLOOR} reached after {whole} whole files; files that do not fit are listed, not shown — read one for its text]"
                ));
            }
            if let Some(document) = document {
                sections.push(format!("{display}  ({})", document.describe(bytes.len())));
                continue;
            }
            let text = String::from_utf8(bytes).unwrap_or_default();
            let (heads, total) = crate::orient::skeleton(&text, GLOB_HEADS_PER_FILE);
            let mut rows = vec![format!(
                "{display}  ({} lines, {total} declarations, skeleton)",
                text.lines().count()
            )];
            rows.extend(heads.iter().map(|head| format!("  {head}")));
            sections.push(rows.join("\n"));
        }
        sections.extend(walled.notices());
        let mut output = text_output(sections.join("\n\n"));
        output.result.details = json!({ "files": matches.len(), "whole": whole });
        output
    }

    pub(super) fn render_file(
        &self,
        display_path: &str,
        path: &Path,
        raw: &str,
        view: View<'_>,
        input: &Map<String, Value>,
        context: &ToolContext,
    ) -> ToolOutput {
        let View {
            resolve_as,
            note,
            on_disk,
            first_look,
        } = view;
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
                match locate(
                    display_path,
                    resolve_as,
                    &normalized,
                    &all_lines[..line_count],
                    needle,
                ) {
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
        let floor = if explicit_window || found.is_some() {
            READ_BYTE_FLOOR
        } else {
            first_look
        };
        let budget = explicit_limit.map_or(floor, |limit| {
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
                let (text, was_clipped) = if on_disk {
                    crate::tool::clip(line, READ_LINE_CLIP)
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
                    "[find: no enclosing block; lines {lo}-{hi} of {line_count} around the hit at line {} — ranges= for more]",
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
        rendered.extend(super::messages::clipped_lines_hint(
            &clipped,
            READ_LINE_CLIP,
            display_path,
        ));
        let capped = byte_capped_at.is_some()
            || (!explicit_window && found.is_none() && shown_end < line_count);
        if capped && !on_disk {
            let at = rendered.len().min(2);
            let outline = super::documents::outline_rows(&normalized);
            rendered.splice(at..at, outline);
        } else if capped {
            rendered.extend(if resolve_as.ends_with(".md") {
                super::documents::outline_rows(&normalized)
            } else {
                skeleton_rows(&normalized)
            });
        }
        let mut refs_total = 0_usize;
        if let Some(Found {
            identifier: Some(identifier),
            index,
            ..
        }) = &found
            && on_disk
        {
            let grep = crate::grep::GrepTool {
                hashline: Some(Arc::clone(&self.state)),
            };
            let (refs, total) = grep.references(
                &context.cwd,
                &context.deny_read,
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
        // Invariant: an unchanged line the model saw is still a line it saw, so the prior
        // seen set rebases onto the new tag rather than shrinking to the hunk windows (#473).
        let carried: Vec<u64> = match snapshots.by_content(&result.canonical_path, &result.before) {
            Some(prior) => {
                let map = super::rebase::LineMap::between(&result.before, &result.after);
                prior
                    .seen_lines
                    .iter()
                    .flatten()
                    .filter_map(|line| map.line(*line))
                    .collect()
            }
            None => Vec::new(),
        };
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
        seen.extend(carried);
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
    /// It runs on a clone of the store, so a refusal's reveal is recorded only by an execute.
    fn preview(&self, input: &Map<String, Value>, cwd: &Path) -> Option<String> {
        let patch_text = input.get("patch").and_then(Value::as_str)?;
        let patch = Patch::parse(patch_text, Some(cwd)).ok()?;
        let state = lock_state(&self.state);
        let mut snapshots = state.snapshots.clone();
        let mut scratch = super::clipboard::fork_clipboard(&state.clipboard);
        let mut patcher = Patcher::new(&mut snapshots, cwd.to_owned());
        let mut out = String::new();
        for section in &patch.sections {
            let Ok(prepared) = patcher.prepare(section, &mut scratch) else {
                continue;
            };
            if prepared.is_noop() {
                continue;
            }
            let (before, after) = prepared.diff_inputs();
            let resolved = yi_permission::resolve_target(prepared.path(), cwd);
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
        let parsing = yi_types::trace::span("edit.parse");
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
        let home = documents(&self.state).map(|documents| documents.home);
        if let Some(refusal) = patch.sections.iter().find_map(|section| {
            crate::document::write_refusal(home.as_deref(), &resolve_path(context, &section.path))
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
        drop(parsing);
        let applying = yi_types::trace::span("edit.apply");
        let mut patcher = Patcher::new(snapshots, context.cwd.clone());
        patcher.walls = context.write_walls();
        let results = patcher.apply(&patch, &mut host_clipboard);
        drop(applying);
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
        let mut checked: Vec<(usize, &str)> = Vec::new();
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
                // An empty paste is a no-op whose warning is the only reason it changed nothing.
                rendered.extend(result.warnings.iter().map(|w| format!("warning: {w}")));
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
            if result.op != SectionOp::Delete {
                checked.push((rendered.len(), result.canonical_path.as_str()));
            }
            rendered.push(render_section_result(result, snapshots));
        }
        let (verdicts, layer) = post_edit_checks(&checked, &results, context);
        let mut syntax: Option<String> = None;
        for ((index, _), line) in checked.iter().zip(verdicts) {
            let Some(line) = line else { continue };
            if let Some(section) = rendered.get_mut(*index) {
                section.push('\n');
                section.push_str(&line);
            }
            syntax = Some(crate::syntax::worst(syntax, line));
        }
        let ripwire = layer.map_or("skipped", |layer| {
            rendered.push(layer.render());
            layer.name()
        });
        let mut output = text_output(rendered.join("\n\n"));
        if !diff.is_empty() {
            output.result.details = crate::diff::patch_details(&GitPatch::from_text(diff));
        }
        if let Value::Object(details) = &mut output.result.details {
            details.insert("ripwire".to_owned(), Value::String(ripwire.to_owned()));
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
                    "deleted": count(SectionOp::Delete),
                    "noop": count(SectionOp::Noop),
                    "moved": results.iter().filter(|result| result.move_dest.is_some()).count(),
                }),
            );
        }
        output
    }
}

/// Both read the settled tree, so the ripwire check runs beside the syntax verdicts.
fn post_edit_checks(
    paths: &[(usize, &str)],
    results: &[PatchSectionResult],
    context: &ToolContext,
) -> (Vec<Option<String>>, Option<CheckLayer>) {
    let regions: Vec<(String, u64)> = (results.iter())
        .filter(|result| result.op == SectionOp::Update && crate::ripwire::installed())
        .flat_map(|result| {
            let (path, before) = (&result.canonical_path, &result.before);
            crate::ripwire::regions(context, path, before, &result.after)
        })
        .collect();
    std::thread::scope(|scope| {
        let ripwire = (!regions.is_empty()).then(|| {
            scope.spawn(|| {
                let _span = yi_types::trace::span("edit.ripwire");
                crate::ripwire::check(context, regions)
            })
        });
        let verdicts = paths
            .iter()
            .map(|(_, path)| crate::syntax::verdict(Path::new(path)))
            .collect();
        let layer = ripwire.map(|handle| {
            (handle.join()).unwrap_or_else(|_| CheckLayer::failed("the check's thread panicked"))
        });
        (verdicts, layer)
    })
}

pub fn record_write_snapshot(
    state: &SharedHashline,
    path: &Path,
    content: &str,
) -> super::format::FileTag {
    let line_count = u64::try_from(content.split('\n').count()).unwrap_or(u64::MAX);
    let seen: Vec<u64> = (1..=line_count).collect();
    record_view_snapshot(state, path, content, &seen)
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
