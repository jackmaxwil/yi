use super::format::{
    FileTag, HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX, HL_RANGE_SEP, format_numbered_line,
};
use super::patcher::{SEEN_LINE_REVEAL_CAP, SEEN_LINE_REVEAL_MAX_COLUMNS};
use super::types::BlockSpan;

pub const MISMATCH_CONTEXT: u64 = 2;

/// The lines a refusal shows around each anchor: the same set it marks as seen.
pub fn anchored_lines(anchor_lines: &[u64], total: u64) -> Vec<u64> {
    let mut display: Vec<u64> = Vec::new();
    for &line in anchor_lines {
        if line < 1 || line > total {
            continue;
        }
        let lo = line.saturating_sub(MISMATCH_CONTEXT).max(1);
        let hi = line.saturating_add(MISMATCH_CONTEXT).min(total);
        for line_num in lo..=hi {
            if !display.contains(&line_num) {
                display.push(line_num);
            }
        }
    }
    display.sort_unstable();
    display
}

/// One refusal's rows; `seen` is every whole line printed, or empty past the row cap.
pub struct RefusalRows {
    pub rows: Vec<String>,
    pub seen: Vec<u64>,
    pub cut_rows: bool,
    pub clipped: Vec<u64>,
}

pub fn refusal_rows(display: &[u64], anchors: &[u64], lines: &[impl AsRef<str>]) -> RefusalRows {
    let mut rows: Vec<String> = Vec::new();
    let mut clipped: Vec<u64> = Vec::new();
    let cut_rows = display.len() > SEEN_LINE_REVEAL_CAP;
    let mut previous: Option<u64> = None;
    for &line in display.iter().take(SEEN_LINE_REVEAL_CAP) {
        if previous.is_some_and(|previous| line > previous.saturating_add(1)) {
            rows.push("...".to_owned());
        }
        previous = Some(line);
        let text = usize::try_from(line)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| lines.get(index))
            .map_or("", |text| text.as_ref());
        let (text, cut) = crate::tool::clip(text, SEEN_LINE_REVEAL_MAX_COLUMNS);
        if cut {
            clipped.push(line);
        }
        let marker = if anchors.contains(&line) { "*" } else { " " };
        rows.push(format!("{marker}{}", format_numbered_line(line, &text)));
    }
    let seen = display
        .iter()
        .copied()
        .filter(|line| !cut_rows && !clipped.contains(line))
        .collect();
    RefusalRows {
        rows,
        seen,
        cut_rows,
        clipped,
    }
}

/// What the rows unlock: whole, the retry; cut at the cap, nothing; a clipped anchor, never.
pub fn refusal_footer(
    section_path: &str,
    tag: FileTag,
    shown: &RefusalRows,
    anchors: &[u64],
    total: u64,
) -> String {
    if !anchors.is_empty() && shown.rows.is_empty() {
        return format!(
            "Lines {} are past the end (the file has {total} lines).",
            format_line_ranges(anchors)
        );
    }
    if shown.cut_rows {
        return format!(
            "More than {SEEN_LINE_REVEAL_CAP} rows, so none of these lines count as displayed. \
Read them whole with `read` on {section_path} with {}, then re-issue the edit with the header that read returns.",
            read_ranges(anchors)
        );
    }
    let wide: Vec<u64> = anchors
        .iter()
        .copied()
        .filter(|line| shown.clipped.contains(line))
        .collect();
    if !wide.is_empty() {
        return format!(
            "Line(s) {} exceed {SEEN_LINE_REVEAL_MAX_COLUMNS} columns, and a read or a refusal never anchors one: \
change them with `grep` on this file (path={section_path}, `replace` shows the rewrite, `apply` then writes it), \
or rewrite the file with `write` (`sed -n 'Np' {section_path}` shows a whole line).",
            format_line_ranges(&wide)
        );
    }
    format!(
        "Check these are the lines you meant, then re-issue with this header: \
{HL_FILE_PREFIX}{section_path}{HL_FILE_HASH_SEP}{tag}{HL_FILE_SUFFIX}. If they are not, fix the line numbers first."
    )
}

/// `ranges=[[a,b],...]` as `read` takes it, one pair per run of consecutive lines.
pub fn read_ranges(lines: &[u64]) -> String {
    let pairs: Vec<String> = format_line_ranges(lines)
        .split(", ")
        .filter(|range| !range.is_empty())
        .map(|range| match range.split_once('-') {
            Some((start, end)) => format!("[{start},{end}]"),
            None => format!("[{range},{range}]"),
        })
        .collect();
    format!("ranges=[{}]", pairs.join(","))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsoluteRangeOp {
    Replace,
    Cut,
}

fn reg_suffix(register: Option<&str>) -> String {
    register.map(|name| format!(" @{name}")).unwrap_or_default()
}

fn range_op_single(op: AbsoluteRangeOp, line: u64, register: Option<&str>) -> String {
    match (op, register) {
        (AbsoluteRangeOp::Replace, Some(_)) => format!("PUT {line}{}", reg_suffix(register)),
        (AbsoluteRangeOp::Replace, None) => format!("PUT {line}:"),
        (AbsoluteRangeOp::Cut, _) => format!("CUT {line}{}", reg_suffix(register)),
    }
}

fn range_op_range(op: AbsoluteRangeOp, start: u64, end: u64, register: Option<&str>) -> String {
    match (op, register) {
        (AbsoluteRangeOp::Replace, Some(_)) => {
            format!("PUT {start}{HL_RANGE_SEP}{end}{}", reg_suffix(register))
        }
        (AbsoluteRangeOp::Replace, None) => format!("PUT {start}{HL_RANGE_SEP}{end}:"),
        (AbsoluteRangeOp::Cut, _) => {
            format!("CUT {start}{HL_RANGE_SEP}{end}{}", reg_suffix(register))
        }
    }
}

fn block_form_at(op: AbsoluteRangeOp, line: u64, register: Option<&str>) -> String {
    match (op, register) {
        (AbsoluteRangeOp::Replace, Some(_)) => format!("PUT {line}*{}", reg_suffix(register)),
        (AbsoluteRangeOp::Replace, None) => format!("PUT {line}*:"),
        (AbsoluteRangeOp::Cut, _) => format!("CUT {line}*{}", reg_suffix(register)),
    }
}

pub fn invalid_absolute_range_message(
    patch_line: u64,
    start: u64,
    end: u64,
    op: AbsoluteRangeOp,
    block: Option<BlockSpan>,
    register: Option<&str>,
) -> String {
    let single = range_op_single(op, start, register);
    let counted_end = start.checked_add(end).map(|sum| sum.saturating_sub(1));
    let block_form = block_form_at(op, start, register);
    let mut message = format!(
        "line {patch_line}: Invalid absolute range: start {start}, end {end}. \
The value after `{HL_RANGE_SEP}` is an absolute source line, not a line count or replacement length. \
For one line use `{single}`."
    );
    if let Some(counted_end) = counted_end.filter(|counted_end| *counted_end >= start) {
        let counted = range_op_range(op, start, counted_end, register);
        message.push_str(&format!(
            " For {end} lines starting at {start}, use `{counted}`."
        ));
    }
    if let Some(block) = block.filter(|block| block.start == start && block.end > start) {
        message.push_str(&format!(
            " The syntactic block beginning at {start} ends at {}, so `{block_form}` is also valid.",
            block.end
        ));
    }
    message
}

pub const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub const END_PATCH_MARKER: &str = "*** End Patch";
pub const ABORT_MARKER: &str = "*** Abort";

pub const REPLACE_PAIR_COALESCED_WARNING: &str = "Multiple hunks targeted the same exact range; kept only the last. Issue one `PUT` or `CUT` hunk per range.";

pub const REPLACEMENT_INDENT_AUTO_SHIFT_WARNING: &str =
    "Auto-indented a replacement body to match unchanged structural rows in its source range.";

pub const BARE_BODY_ROW_REFUSED: &str = "body row has no `+`. Every body row is `+TEXT` (a literal `-`/`+` start is `+- item` / `++ item`); the range removes the old lines, so context rows are never written.";

pub const BLANK_BODY_ROW_REFUSED: &str =
    "blank row inside a body. A blank line is `+` alone; every body row starts with `+`.";

pub const SNAPSHOT_ROWS_AUTO_PUT_WARNING: &str = "Recovered top-level `N:TEXT` snapshot row(s) as single-line `PUT N.=N:` replacements. Use explicit `PUT` headers for reliable edits.";

pub fn repeated_snapshot_row_message(line: u64) -> String {
    format!(
        "two or more pasted `{line}:TEXT` read-output rows name line {line}. \
Such rows are recovered as single-line `PUT {line}{HL_RANGE_SEP}{line}:` replacements, so repeating a \
number would keep only the last row and drop the rest. Write the hunk explicitly: one \
`PUT {line}{HL_RANGE_SEP}M:` header covering exactly the lines that change, followed by `+TEXT` body \
rows holding their complete final content."
    )
}

pub fn literal_op_row_warning(line: u64, text: &str) -> String {
    format!(
        "line {line}: body row `+{text}` is itself a valid hunk header, so it was inserted \
into the file as literal text rather than executed. Ops are never `+`-prefixed — drop \
the `+` to run it, and re-issue if this line landed in the file by mistake."
    )
}

pub fn near_miss_header_warning(line: u64, written: &str, read_as: &str) -> String {
    format!(
        "line {line}: read the hunk header `{written}` as `{read_as}`, one stray mark removed. \
Write headers in that form."
    )
}

pub const BARE_RANGE_AUTO_PUT_WARNING: &str =
    "Recovered a bare `N.=M:` header as `PUT N.=M:`. Prefix replacement ranges with `PUT`.";

pub const READ_METADATA_IGNORED_WARNING: &str =
    "Ignored copied read-output elision row(s). Re-read elided ranges before editing them.";

pub fn empty_put_message(put_form: &str, cut_form: &str) -> String {
    format!(
        "`{put_form}` has no body rows and an empty `PUT` never deletes. To delete, use `{cut_form}`; to replace, add `+TEXT` rows."
    )
}

pub const CUT_COLON_IGNORED_WARNING: &str =
    "Ignored a trailing `:` on bodyless `CUT`. Prefer `CUT N.=M` / `CUT N*` without a colon.";

pub const MINUS_BULLET_AUTO_PIPED_WARNING: &str = "Auto-prefixed bare `- ` bullet row(s) as literal content. `-` rows never remove lines — the range does that; always prefix literal body rows with `+`: `+- item`.";

pub const DIFF_OLD_ROWS_IGNORED_WARNING: &str = "Ignored unified-diff `-old` row(s); the range already removes old content, so only `+new` rows were kept.";

pub const MINUS_ROW_REJECTED: &str = "`-` rows are not valid; the range already names the lines being changed. For Markdown bullets or other literal `-` lines, prefix the literal row with `+`: `+- item`.";

#[derive(Debug, Clone, Copy, Default)]
pub struct BlockDiagnosticSuggestions {
    pub next_block: Option<BlockSpan>,
    pub enclosing_block: Option<BlockSpan>,
}

pub fn block_unresolved_message(
    line: u64,
    op: AbsoluteRangeOp,
    file_lines: Option<&[String]>,
    suggestions: BlockDiagnosticSuggestions,
    register: Option<&str>,
) -> String {
    let phrase = block_form_at(op, line, register);
    let fallback = match (op, register) {
        (AbsoluteRangeOp::Replace, Some(name)) => format!("PUT {line}{HL_RANGE_SEP}M @{name}"),
        (AbsoluteRangeOp::Replace, None) => format!("PUT {line}{HL_RANGE_SEP}M:"),
        (AbsoluteRangeOp::Cut, Some(name)) => format!("CUT {line}{HL_RANGE_SEP}M @{name}"),
        (AbsoluteRangeOp::Cut, None) => format!("CUT {line}{HL_RANGE_SEP}M"),
    };
    let anchor_text = file_lines.and_then(|lines| lines.get(line.saturating_sub(1) as usize));
    let mut message = match (anchor_text, suggestions.next_block) {
        (Some(anchor), Some(next)) if anchor.trim().is_empty() => {
            let retry = block_form_at(op, next.start, register);
            format!(
                "Line {line} is blank; no syntactic block can begin there. \
The next multi-line block begins at line {} and ends at line {}. Retry `{retry}`.",
                next.start, next.end
            )
        }
        _ => format!(
            "`{phrase}` could not resolve a syntactic block beginning on line {line} \
(unsupported language, blank/closer line, or parse error). Use `{fallback}` with explicit lines."
        ),
    };
    if let Some(enclosing) = suggestions.enclosing_block {
        let retry = block_form_at(op, enclosing.start, register);
        message.push_str(&format!(
            " The nearest enclosing multi-line block begins at line {} and ends at line {}; use `{retry}` to target it.",
            enclosing.start, enclosing.end
        ));
    }
    if let Some(file_lines) = file_lines {
        let total = u64::try_from(file_lines.len()).unwrap_or(u64::MAX);
        let rows = refusal_rows(&anchored_lines(&[line], total), &[line], file_lines).rows;
        if !rows.is_empty() {
            message.push_str(&format!("\n\n{}", rows.join("\n")));
        }
    }
    message
}

pub const BLOCK_RESOLVER_UNAVAILABLE: &str = "Block locators (`N*` in `PUT N*:`, `PUT >N*`, `CUT N*`) are not available here (no block resolver configured). Use a concrete line range.";

fn closer_lowered_warning(block_form: &str, plain_form: &str) -> String {
    format!(
        "`{block_form}` anchors on a closing delimiter, so it was applied as plain `{plain_form}`. Anchor on the line that OPENS the construct."
    )
}

fn unresolved_after_block_message(block_form: &str, line: u64, plain_form: &str) -> String {
    format!(
        "`{block_form}` could not resolve a syntactic block beginning on line {line} (blank, bare inner line, unsupported language, or parse error). Anchor on the line that OPENS the construct, or use plain `{plain_form}` to land right after line {line}."
    )
}

pub fn insert_after_block_closer_lowered_warning(line: u64) -> String {
    closer_lowered_warning(&format!("PUT >{line}*:"), &format!("PUT >{line}:"))
}

pub fn insert_after_block_unresolved_message(line: u64) -> String {
    unresolved_after_block_message(&format!("PUT >{line}*:"), line, &format!("PUT >{line}:"))
}

pub fn paste_after_block_closer_lowered_warning(line: u64) -> String {
    closer_lowered_warning(&format!("PUT >{line}*"), &format!("PUT >{line}"))
}

pub fn paste_after_block_unresolved_message(line: u64) -> String {
    unresolved_after_block_message(&format!("PUT >{line}*"), line, &format!("PUT >{line}"))
}

pub const UNRESOLVED_BLOCK_INTERNAL: &str =
    "internal error: unresolved block edit reached the applier (resolveBlockEdits was not run).";

pub const UNRESOLVED_CLIPBOARD_INTERNAL: &str = "internal error: unresolved clipboard edit reached the applier (resolveClipboardEdits was not run).";

pub const REM_TAKES_NO_BODY: &str = "`REM` deletes the whole file and takes no body rows or line ops. Issue it alone under the header.";

pub const MOVE_TAKES_NO_BODY: &str = "`MV DEST` does not take body rows. Put line edits above the `MV` row; the destination path follows `MV` on the same line.";

pub const CUT_TAKES_NO_BODY: &str = "`CUT` deletes (and captures) the named lines and takes no body rows. To write new content, use `PUT N.=M:` with `+TEXT` rows.";

pub const COLON_ON_REGISTER_PUT: &str = "`PUT … @name` pastes the register and never takes `:` — the colon promises body rows. Drop the colon (`PUT >40 @name`), or drop `@name` and write `+TEXT` body rows.";

pub const REGISTER_PUT_TAKES_NO_BODY: &str = "A register `PUT` pastes captured lines and takes no `+` body rows. To write literal text, drop the `@name` and use `PUT …:` with body rows.";

pub const COLONLESS_PUT_TAKES_NO_BODY: &str = "`PUT` without `:` is clipboard-backed and takes no body rows. Add `:` after the locator to write literal content (`PUT >40:` then `+TEXT` rows).";

pub const COLONLESS_SPAN_PUT: &str = "Colonless `PUT` is clipboard-backed, and span targets need a named register (`PUT 5.=9 @name`); the anonymous register pastes only at gaps (`PUT >40`). To write literal content, add `:` and `+TEXT` body rows.";

pub const EMPTY_PASTE: &str = "Nothing to paste: no unlabeled `CUT` precedes this `PUT` in this call, and the anonymous register never carries across calls. Put `CUT N.=M` / `CUT N*` above it, or use named registers (`CUT … @name` → `PUT … @name`) for cross-call moves.";

pub fn empty_register_paste_warning(name: &str, known: &[(String, usize)]) -> String {
    let base = format!(
        "`@{name}` was empty — no `CUT … @{name}` precedes this op in this call and no persisted register has that name — so nothing was pasted."
    );
    if known.is_empty() {
        base
    } else {
        let listed = known
            .iter()
            .map(|(k, lines)| format!("`@{k}` ({lines} lines)"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{base} Available registers: {listed}.")
    }
}

pub fn empty_register_span_paste_message(name: &str, known: &[(String, usize)]) -> String {
    let base = format!(
        "`@{name}` is empty — no `CUT … @{name}` precedes this op in this call and no persisted register \
has that name — so pasting it over a range would delete those lines and write nothing back. \
Capture the register first (`CUT … @{name}`), or use `CUT` if deleting the range is what you meant."
    );
    if known.is_empty() {
        base
    } else {
        let listed = known
            .iter()
            .map(|(k, lines)| format!("`@{k}` ({lines} lines)"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{base} Available registers: {listed}.")
    }
}

pub fn ambiguous_anonymous_paste_message(pending: &[String]) -> String {
    format!(
        "{} unlabeled `CUT`s are pending ({}) — an unlabeled paste cannot tell which one you meant. \
Label the moves (`CUT … @name` → `PUT … @name`), or keep at most one unlabeled `CUT` before each unlabeled paste.",
        pending.len(),
        pending.join(", ")
    )
}

pub const CLIPBOARD_INTERLEAVED_SECTIONS: &str = "`CUT`/register-`PUT` ops cannot be used in a file whose sections are interleaved with another file's: same-path sections merge into the first occurrence, which would reorder the register sequence. Keep each file's ops under ONE `[path#TAG]` header.";

pub const EMPTY_INSERT: &str = "`PUT <N:` / `PUT >N:` promises body rows and got none. Write `+TEXT` rows, or drop the `:` to paste a register (`PUT >N` = anonymous, `PUT >N @name` = named).";

pub const RECOVERY_EXTERNAL_WARNING: &str = "Recovered from a stale file hash using a previous read snapshot (file changed externally between read and edit).";

pub const RECOVERY_SESSION_CHAIN_WARNING: &str = "Recovered from a stale file hash using an earlier in-session snapshot (a prior edit in this session advanced the hash).";

pub const RECOVERY_LINE_REMAP_WARNING: &str = "Recovered by remapping stale line anchors to unchanged current lines (file changed since the tagged read). Verify the diff matches your intent.";

pub const HEADTAIL_DRIFT_WARNING: &str = "Applied the `PUT <1:`/`PUT >$:` edit despite a stale snapshot tag (file changed since your read) — head/tail position is content-independent. Re-read if the drift was unexpected.";

pub fn write_drift_warning(path: &str) -> String {
    format!(
        "{path}: the file on disk after this write differs from what was sent — the client \
(editor/IDE) likely reformatted it on save (e.g. format-on-save, tab/space settings). \
The returned snapshot reflects the actual file; re-read before further edits if the \
extra changes were unexpected."
    )
}

pub fn missing_snapshot_tag_message(
    section_path: &str,
    minted: Option<(FileTag, &[String], &str)>,
) -> String {
    match minted {
        Some((tag, rows, footer)) => format!(
            "No version of {section_path} is on record for this session (never shown, or no longer held). \
It reads now as {HL_FILE_PREFIX}{section_path}{HL_FILE_HASH_SEP}{tag}{HL_FILE_SUFFIX} at the lines this edit names:\n{}\n{footer}",
            rows.join("\n")
        ),
        None => format!(
            "No version of {section_path} is on record for this session (never shown, or no longer held); `read` or `grep` it first (the {HL_FILE_HASH_SEP}tag in the header is then optional). To create a new file, use the write tool."
        ),
    }
}

pub fn rebased_warning(from: FileTag, to: FileTag) -> String {
    format!(
        "rebased {HL_FILE_HASH_SEP}{from} -> {HL_FILE_HASH_SEP}{to}: the cited lines were found unchanged in the current file and the edit landed on them"
    )
}

pub fn path_recovered_from_tag_message(
    authored_path: &str,
    resolved_path: &str,
    tag: FileTag,
) -> String {
    format!(
        "Path \"{authored_path}\" does not exist; matched its filename and snapshot tag \
{HL_FILE_HASH_SEP}{tag} to {resolved_path} (read earlier this session). Anchor future edits on \
{HL_FILE_PREFIX}{resolved_path}{HL_FILE_HASH_SEP}TAG{HL_FILE_SUFFIX}."
    )
}

/// A minified file can clip thousands of lines; the hint names the first ranges, not all.
const SED_TERMS: usize = 20;

/// One sed call for every clipped line, runnable as printed.
pub fn clipped_lines_hint(clipped: &[u64], clip: usize, path: &str) -> Option<String> {
    let ranges = format_line_ranges(clipped);
    if ranges.is_empty() {
        return None;
    }
    let terms: Vec<String> = ranges
        .split(", ")
        .map(|range| format!("{}p", range.replace('-', ",")))
        .collect();
    let script = terms
        .get(..terms.len().min(SED_TERMS))
        .unwrap_or(&[])
        .join(";");
    let more = if terms.len() > SED_TERMS {
        format!(" (the first {SED_TERMS} of {} ranges)", terms.len())
    } else {
        String::new()
    };
    Some(format!(
        "[{} line(s) exceeded {clip} chars and were clipped (never edit-anchors) — full lines: bash: sed -n '{script}' {}{more}]",
        clipped.len(),
        shell_path(path)
    ))
}

/// Bare when shell-inert, so the bash bridge still tags the output; else single-quoted with each
/// `'` closed, escaped and reopened. A leading `-` would read as a sed flag, so it gets `./`.
fn shell_path(path: &str) -> String {
    if path.starts_with('-') {
        return shell_path(&format!("./{path}"));
    }
    let inert = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-');
    if !path.is_empty() && path.chars().all(inert) {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', r"'\''"))
    }
}

pub fn format_line_ranges(lines: &[u64]) -> String {
    let mut sorted: Vec<u64> = lines.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let Some(&first) = sorted.first() else {
        return String::new();
    };
    let mut parts = Vec::new();
    let mut start = first;
    let mut prev = first;
    for &current in sorted.iter().skip(1) {
        if current == prev.saturating_add(1) {
            prev = current;
            continue;
        }
        parts.push(if start == prev {
            format!("{start}")
        } else {
            format!("{start}-{prev}")
        });
        start = current;
        prev = current;
    }
    parts.push(if start == prev {
        format!("{start}")
    } else {
        format!("{start}-{prev}")
    });
    parts.join(", ")
}

pub fn unseen_lines_message(
    section_path: &str,
    unseen_lines: &[u64],
    tag: FileTag,
    rows: &[String],
    footer: &str,
) -> String {
    format!(
        "This edit anchors to lines {} of {section_path} that \
{HL_FILE_PREFIX}{section_path}{HL_FILE_HASH_SEP}{tag}{HL_FILE_SUFFIX} never displayed (it showed a \
partial range, a search hit, or a folded summary). Those lines read now:\n{}\n{footer}",
        format_line_ranges(unseen_lines),
        rows.join("\n")
    )
}

pub fn block_single_line_message(
    line: u64,
    op: super::types::BlockMode,
    enclosing_block: Option<BlockSpan>,
) -> String {
    fn form_at(op: super::types::BlockMode, line: u64) -> String {
        match op {
            super::types::BlockMode::Replace => format!("PUT {line}*:"),
            super::types::BlockMode::InsertAfter => format!("PUT >{line}*:"),
            super::types::BlockMode::Cut => format!("CUT {line}*"),
            super::types::BlockMode::PasteAfter => format!("PUT >{line}*"),
        }
    }
    let plain = match op {
        super::types::BlockMode::Replace => format!("PUT {line}:"),
        super::types::BlockMode::InsertAfter => format!("PUT >{line}:"),
        super::types::BlockMode::Cut => format!("CUT {line}"),
        super::types::BlockMode::PasteAfter => format!("PUT >{line}"),
    };
    let mut message = format!(
        "`{}` resolved a single-line block — line {line} is a bare statement, not the opening line \
of a multi-line construct. For only this statement use `{plain}`.",
        form_at(op, line)
    );
    if let Some(enclosing) = enclosing_block {
        message.push_str(&format!(
            " The nearest enclosing multi-line block begins at line {} and ends at line {}; use `{}` to target it.",
            enclosing.start,
            enclosing.end,
            form_at(op, enclosing.start)
        ));
    }
    message
}
