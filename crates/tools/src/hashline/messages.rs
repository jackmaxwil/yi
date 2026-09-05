use super::format::{
    FileTag, HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX, HL_RANGE_SEP, format_numbered_line,
};
use super::types::BlockSpan;

pub const MISMATCH_CONTEXT: u64 = 2;

pub fn format_anchored_context(anchor_lines: &[u64], file_lines: &[String]) -> Vec<String> {
    let total = file_lines.len() as u64;
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
    let mut rows = Vec::new();
    let mut previous: Option<u64> = None;
    for line_num in display {
        if let Some(previous) = previous
            && line_num > previous.saturating_add(1)
        {
            rows.push("...".to_owned());
        }
        previous = Some(line_num);
        let marker = if anchor_lines.contains(&line_num) {
            "*"
        } else {
            " "
        };
        let text = file_lines
            .get((line_num - 1) as usize)
            .map(String::as_str)
            .unwrap_or("");
        rows.push(format!("{marker}{}", format_numbered_line(line_num, text)));
    }
    rows
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

pub const BARE_BODY_AUTO_PIPED_WARNING: &str =
    "Auto-prefixed bare body row(s) with `+`. Body rows must be `+TEXT` literal lines.";

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

pub const BARE_RANGE_AUTO_PUT_WARNING: &str =
    "Recovered a bare `N.=M:` header as `PUT N.=M:`. Prefix replacement ranges with `PUT`.";

pub const READ_METADATA_IGNORED_WARNING: &str =
    "Ignored copied read-output elision row(s). Re-read elided ranges before editing them.";

pub const EMPTY_PUT_AUTO_CUT_WARNING: &str =
    "Interpreted an empty `PUT` body as deletion. Use `CUT N.=M` or `CUT N*` for bodyless deletes.";

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
        let context = format_anchored_context(&[line], file_lines);
        if !context.is_empty() {
            message.push_str(&format!("\n\n{}", context.join("\n")));
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

fn unresolved_lowered_warning(block_form: &str, line: u64, plain_form: &str) -> String {
    format!(
        "`{block_form}` could not resolve a syntactic block on line {line}, so it was applied as plain `{plain_form}`. Verify the landing line; anchor on a line that OPENS a construct."
    )
}

pub fn insert_after_block_closer_lowered_warning(line: u64) -> String {
    closer_lowered_warning(&format!("PUT >{line}*:"), &format!("PUT >{line}:"))
}

pub fn insert_after_block_unresolved_lowered_warning(line: u64) -> String {
    unresolved_lowered_warning(&format!("PUT >{line}*:"), line, &format!("PUT >{line}:"))
}

pub fn paste_after_block_closer_lowered_warning(line: u64) -> String {
    closer_lowered_warning(&format!("PUT >{line}*"), &format!("PUT >{line}"))
}

pub fn paste_after_block_unresolved_lowered_warning(line: u64) -> String {
    unresolved_lowered_warning(&format!("PUT >{line}*"), line, &format!("PUT >{line}"))
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

pub fn empty_register_paste_warning(name: &str, known: &[String]) -> String {
    let base = format!(
        "`@{name}` was empty — no `CUT … @{name}` precedes this op in this call and no persisted register has that name — so nothing was pasted."
    );
    if known.is_empty() {
        base
    } else {
        let listed = known
            .iter()
            .map(|k| format!("`@{k}`"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{base} Available registers: {listed}.")
    }
}

pub fn empty_register_span_paste_message(name: &str, known: &[String]) -> String {
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
            .map(|k| format!("`@{k}`"))
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

pub fn missing_snapshot_tag_message(section_path: &str) -> String {
    format!(
        "No version of {section_path} was shown this session; `read` or `grep` it first (the {HL_FILE_HASH_SEP}tag in the header is then optional). To create a new file, use the write tool."
    )
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealedLine {
    pub line: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct UnseenLinesReveal {
    pub lines: Vec<RevealedLine>,
    pub truncated: bool,
}

pub fn unseen_lines_message(
    section_path: &str,
    unseen_lines: &[u64],
    tag: FileTag,
    reveal: &UnseenLinesReveal,
) -> String {
    let ranges = format_line_ranges(unseen_lines);
    let selector = ranges.replace(", ", ",");
    let header = format!(
        "This edit anchors to lines {ranges} of {section_path} that \
{HL_FILE_PREFIX}{section_path}{HL_FILE_HASH_SEP}{tag}{HL_FILE_SUFFIX} never displayed (it showed a \
partial range, a search hit, or a folded summary)."
    );
    if reveal.lines.is_empty() {
        return format!(
            "{header} Re-read them in full first with a ranged read like \
`{section_path}:{selector}` — it skips summarization and mints a fresh tag (a plain re-read just re-folds \
them) — then re-issue the edit."
        );
    }
    let preview = reveal
        .lines
        .iter()
        .map(|revealed| format!("  {}", format_numbered_line(revealed.line, &revealed.text)))
        .collect::<Vec<_>>()
        .join("\n");
    if reveal.truncated {
        return format!(
            "{header} Preview of the actual file content at the first {} unseen line(s):\n{preview}\n\
The range exceeds the inline preview cap — re-read the remainder with `{section_path}:{selector}` before \
re-issuing the edit.",
            reveal.lines.len()
        );
    }
    format!(
        "{header} Actual file content at those lines:\n{preview}\n\
Verify the content matches what you intend to touch, then re-issue the edit with the same \
{HL_FILE_PREFIX}path{HL_FILE_HASH_SEP}tag{HL_FILE_SUFFIX} header — a straight retry now succeeds without a re-read. \
If the content does NOT match, fix your line numbers."
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
