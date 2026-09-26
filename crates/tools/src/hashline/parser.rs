use std::collections::{HashMap, HashSet};

use super::format::{format_cut_header, format_replace_header};
use super::messages::{
    AbsoluteRangeOp, BARE_BODY_ROW_REFUSED, BARE_RANGE_AUTO_PUT_WARNING, BLANK_BODY_ROW_REFUSED,
    COLON_ON_REGISTER_PUT, COLONLESS_PUT_TAKES_NO_BODY, COLONLESS_SPAN_PUT,
    CUT_COLON_IGNORED_WARNING, CUT_TAKES_NO_BODY, DIFF_OLD_ROWS_IGNORED_WARNING, EMPTY_INSERT,
    MINUS_BULLET_AUTO_PIPED_WARNING, MINUS_ROW_REJECTED, MOVE_TAKES_NO_BODY,
    READ_METADATA_IGNORED_WARNING, REGISTER_PUT_TAKES_NO_BODY, REM_TAKES_NO_BODY,
    REPLACE_PAIR_COALESCED_WARNING, SNAPSHOT_ROWS_AUTO_PUT_WARNING, empty_put_message,
    invalid_absolute_range_message, literal_op_row_warning, near_miss_header_warning,
    repeated_snapshot_row_message,
};
use super::prefixes::is_read_metadata_line;
use super::tokenizer::{BlockTarget, Token, TokenKind, is_hunk_header_text, near_miss_hunk_header};
use super::types::{Anchor, BlockMode, Cursor, Edit, FileOp, ParsedRange, PasteTarget};

/// Bounds parser amplification before the target file's line count is available.
const MAX_EXPANDED_RANGE_LINES: u64 = 100_000;

fn validate_range(
    range: &ParsedRange,
    line_num: u64,
    op: AbsoluteRangeOp,
    register: Option<&str>,
) -> Result<(), String> {
    let op_name = match op {
        AbsoluteRangeOp::Replace => "replace",
        AbsoluteRangeOp::Cut => "cut",
    };
    if range.start.line < 1 || range.end.line < 1 {
        return Err(format!(
            "line {line_num}: {op_name} range endpoints must be positive safe integers; got {} and {}.",
            range.start.line, range.end.line
        ));
    }
    if range.end.line < range.start.line {
        return Err(invalid_absolute_range_message(
            line_num,
            range.start.line,
            range.end.line,
            op,
            None,
            register,
        ));
    }
    let span = range.end.line - range.start.line + 1;
    if span > MAX_EXPANDED_RANGE_LINES {
        return Err(format!(
            "line {line_num}: {op_name} range spans {span} lines; the maximum is {MAX_EXPANDED_RANGE_LINES}. Split it into smaller hunks."
        ));
    }
    Ok(())
}

fn is_skippable_comment_line(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

fn bodyless_target_message(target: &BlockTarget, had_colon: bool) -> Option<&'static str> {
    match target {
        BlockTarget::Cut { .. } | BlockTarget::CutBlock { .. } => Some(CUT_TAKES_NO_BODY),
        BlockTarget::Rem | BlockTarget::Move { .. } => None,
        _ if target.register().is_some() => Some(REGISTER_PUT_TAKES_NO_BODY),
        _ if !had_colon => Some(COLONLESS_PUT_TAKES_NO_BODY),
        _ => None,
    }
}

fn parse_top_level_snapshot_row(text: &str) -> Option<(u64, &str)> {
    let trimmed_start = text.trim_start();
    let offset = text.len() - trimmed_start.len();
    let bytes = trimmed_start.as_bytes();
    if bytes
        .first()
        .is_none_or(|byte| !(b'1'..=b'9').contains(byte))
    {
        return None;
    }
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index >= bytes.len() || (bytes[index] != b':' && bytes[index] != b'|') {
        return None;
    }
    let line: u64 = trimmed_start[..index].parse().ok()?;
    Some((line, &text[offset + index + 1..]))
}

fn parse_top_level_bare_range_header(text: &str) -> Option<ParsedRange> {
    let trimmed = text.trim();
    let stripped = trimmed.strip_suffix(':')?;
    let bytes = stripped.as_bytes();
    if bytes
        .first()
        .is_none_or(|byte| !(b'1'..=b'9').contains(byte))
    {
        return None;
    }
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    let start: u64 = stripped[..index].parse().ok()?;
    let mut saw_sep = false;
    let rest = &stripped[index..];
    let mut chars = rest.char_indices();
    let mut end_start = None;
    for (position, ch) in chars.by_ref() {
        if ch.is_whitespace() || matches!(ch, '-' | '.' | '=' | '\u{2026}') {
            saw_sep = true;
            continue;
        }
        if saw_sep && ('1'..='9').contains(&ch) {
            end_start = Some(position);
        }
        break;
    }
    drop(chars);
    let end_start = end_start?;
    let end_text = rest[end_start..].trim_end();
    if end_text.is_empty() || !end_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let end: u64 = end_text.parse().ok()?;
    let _ = chars;
    Some(ParsedRange {
        start: Anchor { line: start },
        end: Anchor { line: end },
    })
}

fn is_md_bullet_row(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.len() >= 3
        && trimmed.starts_with("- ")
        && trimmed[2..]
            .chars()
            .next()
            .is_some_and(|ch| !ch.is_whitespace())
}

fn detect_apply_patch_contamination(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let preview = |text: &str| {
        if text.chars().count() > 48 {
            let clipped: String = text.chars().take(48).collect();
            format!("{clipped}\u{2026}")
        } else {
            text.to_owned()
        }
    };
    if trimmed.starts_with("*** Update File:")
        || trimmed.starts_with("*** Add File:")
        || trimmed.starts_with("*** Delete File:")
        || trimmed.starts_with("*** Move to:")
    {
        return Some(format!(
            "apply_patch sentinel {:?} is not valid in hashline. \
File sections start with `[path#HASH]` (no `Update File:` / `Add File:` keyword). \
Use `PUT N.=M:`, `CUT N.=M`, or `PUT <N:`/`PUT >N:` ops.",
            preview(trimmed)
        ));
    }
    if trimmed.starts_with("@@") {
        if is_unified_diff_hunk_header(trimmed) {
            return Some(
                "unified-diff hunk header (`@@ -N,M +N,M @@`) is not valid in hashline. \
Use `PUT N.=M:`, `CUT N.=M`, or `PUT <N:`/`PUT >N:` ops."
                    .to_owned(),
            );
        }
        return Some(format!(
            "`@@`-bracketed hunk header {:?} is not valid in hashline. \
Drop the `@@ ... @@` brackets and write a header such as `PUT N.=M:`.",
            preview(trimmed)
        ));
    }
    let bare = trimmed.trim_end();
    if !bare.is_empty()
        && bare.as_bytes()[0] != b'0'
        && bare.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Some(format!(
            "hunk headers need a verb and both endpoints. Use `PUT {bare}.={bare}:` to replace, or `CUT {bare}.={bare}` to delete."
        ));
    }
    if let Some((first, second)) = bare.split_once(char::is_whitespace) {
        let second = second.trim().trim_end_matches(':').trim_end();
        let numeric = |value: &str| {
            !value.is_empty()
                && value.as_bytes()[0] != b'0'
                && value.bytes().all(|byte| byte.is_ascii_digit())
        };
        if numeric(first) && numeric(second) {
            return Some(format!(
                "bare range hunk header {bare:?} is not valid. \
Hunk headers need a verb: use `PUT N.=M:` or `CUT N.=M`."
            ));
        }
    }
    None
}

fn is_unified_diff_hunk_header(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("@@") else {
        return false;
    };
    let rest = rest.trim_start();
    let mut parts = rest.split_whitespace();
    let looks_like_pair = |part: &str| {
        let part = part.strip_prefix(['-', '+']).unwrap_or(part);
        let mut halves = part.splitn(2, ',');
        let a = halves.next().unwrap_or("");
        let b = halves.next().unwrap_or("");
        !a.is_empty()
            && !b.is_empty()
            && a.bytes().all(|byte| byte.is_ascii_digit())
            && b.bytes().all(|byte| byte.is_ascii_digit())
    };
    match (parts.next(), parts.next(), parts.next()) {
        (Some(first), Some(second), Some(close)) => {
            looks_like_pair(first) && looks_like_pair(second) && close.starts_with("@@")
        }
        _ => false,
    }
}

#[derive(Debug, Clone)]
struct PayloadRow {
    text: String,
    line_num: u64,
    minus: bool,
}

struct Pending {
    target: BlockTarget,
    line_num: u64,
    payloads: Vec<PayloadRow>,
    had_colon: bool,
    deferred_blanks: Vec<PayloadRow>,
}

struct PendingComment {
    line_num: u64,
    text: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedSection {
    pub edits: Vec<Edit>,
    pub file_op: Option<FileOp>,
    pub warnings: Vec<String>,
}

#[derive(Default)]
pub struct Executor {
    edits: Vec<Edit>,
    warnings: Vec<String>,
    edit_index: usize,
    pending: Option<Pending>,
    file_op: Option<FileOp>,
    terminated: bool,
    skippable_comments: Vec<PendingComment>,
    recovered_snapshot_lines: HashSet<u64>,
}

impl Executor {
    pub fn new() -> Self {
        Self::default()
    }

    fn warn_once(&mut self, warning: &str) {
        if !self.warnings.iter().any(|existing| existing == warning) {
            self.warnings.push(warning.to_owned());
        }
    }

    fn discard_pending_skippable_comments(&mut self) {
        self.skippable_comments.clear();
    }

    fn consume_pending_skippable_comments(&mut self) -> Result<(), String> {
        let comments = std::mem::take(&mut self.skippable_comments);
        for comment in comments {
            self.handle_raw(&comment.text, comment.line_num)?;
        }
        Ok(())
    }

    pub fn feed(&mut self, token: &Token) -> Result<(), String> {
        if self.terminated {
            return Ok(());
        }
        match &token.kind {
            TokenKind::EnvelopeBegin => self.consume_pending_skippable_comments(),
            TokenKind::EnvelopeEnd => {
                self.consume_pending_skippable_comments()?;
                self.terminated = true;
                Ok(())
            }
            TokenKind::Abort => {
                self.terminated = true;
                Ok(())
            }
            TokenKind::Header { .. } => {
                self.consume_pending_skippable_comments()?;
                self.flush_pending()
            }
            TokenKind::Blank => {
                self.consume_pending_skippable_comments()?;
                self.handle_blank("", token.line_num);
                Ok(())
            }
            TokenKind::PayloadLiteral { text } => {
                self.consume_pending_skippable_comments()?;
                self.handle_literal_payload(text, token.line_num)
            }
            TokenKind::Raw { text } => {
                if let Some((repaired, scan)) = near_miss_hunk_header(text) {
                    self.warnings
                        .push(near_miss_header_warning(token.line_num, text, &repaired));
                    return self.feed(&Token {
                        kind: TokenKind::OpBlock {
                            target: scan.target,
                            had_colon: scan.had_colon,
                        },
                        line_num: token.line_num,
                    });
                }
                if self.pending.is_none() && is_skippable_comment_line(text) {
                    self.skippable_comments.push(PendingComment {
                        text: text.clone(),
                        line_num: token.line_num,
                    });
                    return Ok(());
                }
                self.consume_pending_skippable_comments()?;
                self.handle_raw(text, token.line_num)
            }
            TokenKind::OpBlock { target, had_colon } => {
                self.discard_pending_skippable_comments();
                if let BlockTarget::Replace { range, register } = target {
                    validate_range(
                        range,
                        token.line_num,
                        AbsoluteRangeOp::Replace,
                        register.as_deref(),
                    )?;
                }
                if let BlockTarget::Cut { range, register } = target {
                    validate_range(
                        range,
                        token.line_num,
                        AbsoluteRangeOp::Cut,
                        register.as_deref(),
                    )?;
                }
                // `:` exclusively promises body rows; ops that never take a body
                // reject it outright so the sigil keeps one meaning.
                if *had_colon
                    && matches!(
                        target,
                        BlockTarget::Cut { .. } | BlockTarget::CutBlock { .. }
                    )
                {
                    self.warn_once(CUT_COLON_IGNORED_WARNING);
                }
                if *had_colon
                    && !matches!(target, BlockTarget::Rem | BlockTarget::Move { .. })
                    && target.register().is_some()
                {
                    return Err(format!("line {}: {COLON_ON_REGISTER_PUT}", token.line_num));
                }
                if matches!(target, BlockTarget::Rem) {
                    self.flush_pending()?;
                    return self.set_file_op(FileOp::Rem, token.line_num);
                }
                if let BlockTarget::Move { dest } = target {
                    self.flush_pending()?;
                    return self.set_file_op(FileOp::Move { dest: dest.clone() }, token.line_num);
                }
                self.flush_pending()?;
                self.pending = Some(Pending {
                    target: target.clone(),
                    line_num: token.line_num,
                    payloads: Vec::new(),
                    had_colon: *had_colon,
                    deferred_blanks: Vec::new(),
                });
                Ok(())
            }
        }
    }

    pub fn end(mut self) -> Result<ParsedSection, String> {
        self.consume_pending_skippable_comments()?;
        self.flush_pending()?;
        self.validate_file_op()?;
        self.normalize_overlapping_ranges()?;
        Ok(ParsedSection {
            edits: self.edits,
            file_op: self.file_op,
            warnings: self.warnings,
        })
    }

    fn set_file_op(&mut self, file_op: FileOp, line_num: u64) -> Result<(), String> {
        if self.file_op.is_some() {
            return Err(format!(
                "line {line_num}: only one file-level op (`REM` or `MV`) per section. Merge them under one header."
            ));
        }
        if matches!(file_op, FileOp::Rem) && !self.edits.is_empty() {
            return Err(format!("line {line_num}: {REM_TAKES_NO_BODY}"));
        }
        self.file_op = Some(file_op);
        Ok(())
    }

    fn validate_file_op(&self) -> Result<(), String> {
        if matches!(self.file_op, Some(FileOp::Rem)) && !self.edits.is_empty() {
            return Err(
                "`REM` deletes the whole file and cannot be combined with line ops.".to_owned(),
            );
        }
        Ok(())
    }

    fn normalize_overlapping_ranges(&mut self) -> Result<(), String> {
        struct ConcreteHunk {
            line_num: u64,
            source_lines: HashSet<u64>,
            clipboard_dependent: bool,
        }
        let mut hunks: HashMap<u64, ConcreteHunk> = HashMap::new();
        let mut order: Vec<u64> = Vec::new();
        for edit in &self.edits {
            let line_num = edit.line_num();
            let entry = hunks.entry(line_num).or_insert_with(|| {
                order.push(line_num);
                ConcreteHunk {
                    line_num,
                    source_lines: HashSet::new(),
                    clipboard_dependent: false,
                }
            });
            match edit {
                Edit::Cut { .. } => entry.clipboard_dependent = true,
                Edit::Paste {
                    at: PasteTarget::Span { range },
                    ..
                } => {
                    entry.clipboard_dependent = true;
                    for line in range.start.line..=range.end.line {
                        entry.source_lines.insert(line);
                    }
                }
                Edit::Delete { anchor, .. } => {
                    entry.source_lines.insert(anchor.line);
                }
                _ => {}
            }
        }

        let mut owner_by_line: HashMap<u64, u64> = HashMap::new();
        let mut dropped: HashSet<u64> = HashSet::new();
        for hunk_line in order {
            let Some(hunk) = hunks.get(&hunk_line) else {
                continue;
            };
            if hunk.source_lines.is_empty() {
                continue;
            }
            let mut overlap_owner: Option<u64> = None;
            let mut multiple_owners = false;
            let mut first_overlap: Option<u64> = None;
            let mut sorted_lines: Vec<u64> = hunk.source_lines.iter().copied().collect();
            sorted_lines.sort_unstable();
            for &line in &sorted_lines {
                if let Some(&owner) = owner_by_line.get(&line) {
                    match overlap_owner {
                        None => overlap_owner = Some(owner),
                        Some(existing) if existing != owner => multiple_owners = true,
                        _ => {}
                    }
                    first_overlap.get_or_insert(line);
                }
            }
            let Some(previous_line) = overlap_owner else {
                for &line in &sorted_lines {
                    owner_by_line.insert(line, hunk_line);
                }
                continue;
            };
            let previous = hunks.get(&previous_line);
            let exact = !multiple_owners
                && previous.is_some_and(|previous| {
                    previous.source_lines.len() == hunk.source_lines.len()
                        && hunk
                            .source_lines
                            .iter()
                            .all(|line| previous.source_lines.contains(line))
                });
            if exact && previous.is_some_and(|previous| !previous.clipboard_dependent) {
                dropped.insert(previous_line);
                owner_by_line.retain(|_, owner| *owner != previous_line);
                for &line in &sorted_lines {
                    owner_by_line.insert(line, hunk_line);
                }
                self.warn_once(REPLACE_PAIR_COALESCED_WARNING);
                continue;
            }
            return Err(format!(
                "line {}: anchor line {} is already targeted by another hunk on line {previous_line}. \
Issue ONE hunk per range; payload is only the final desired content, never a before/after pair. \
A CUT over the same lines as a PUT is one PUT over that range carrying the final body.",
                hunk.line_num,
                first_overlap.unwrap_or(0)
            ));
        }
        if !dropped.is_empty() {
            self.edits
                .retain(|edit| !dropped.contains(&edit.line_num()));
        }
        Ok(())
    }

    fn handle_literal_payload(&mut self, text: &str, line_num: u64) -> Result<(), String> {
        let Some(pending) = self.pending.as_mut() else {
            if self.file_op.is_some() {
                return Err(format!("line {line_num}: {MOVE_TAKES_NO_BODY}"));
            }
            return Err(format!(
                "line {line_num}: payload line has no preceding hunk header. Got {:?}.",
                format!("+{text}")
            ));
        };
        if let Some(message) = bodyless_target_message(&pending.target, pending.had_colon) {
            return Err(format!("line {line_num}: {message}"));
        }
        Self::commit_deferred_blanks(pending)?;
        // An op written with the payload prefix inserts as literal text. Correct for `+TEXT`,
        // but it silently plants a `CUT …` line in the file, so name it when it happens.
        if is_hunk_header_text(text) {
            self.warnings.push(literal_op_row_warning(line_num, text));
        }
        if let Some(pending) = self.pending.as_mut() {
            pending.payloads.push(PayloadRow {
                text: text.to_owned(),
                line_num,
                minus: false,
            });
        }
        Ok(())
    }

    fn handle_raw(&mut self, text: &str, line_num: u64) -> Result<(), String> {
        if self.pending.is_none() && is_read_metadata_line(text) {
            self.warn_once(READ_METADATA_IGNORED_WARNING);
            return Ok(());
        }
        if let Some(contamination) = detect_apply_patch_contamination(text) {
            return Err(format!("line {line_num}: {contamination}"));
        }
        if self.file_op.is_some() {
            return Err(format!("line {line_num}: {MOVE_TAKES_NO_BODY}"));
        }
        if self.pending.is_some() {
            if text.trim().is_empty() {
                self.handle_blank(text, line_num);
                return Ok(());
            }
            let (message, minus) = {
                let Some(pending) = self.pending.as_ref() else {
                    return Ok(());
                };
                (
                    bodyless_target_message(&pending.target, pending.had_colon),
                    text.trim_start().starts_with('-'),
                )
            };
            if let Some(message) = message {
                return Err(format!("line {line_num}: {message}"));
            }
            if !minus {
                return Err(format!("line {line_num}: {BARE_BODY_ROW_REFUSED}"));
            }
            if let Some(pending) = self.pending.as_mut() {
                Self::commit_deferred_blanks(pending)?;
                pending.payloads.push(PayloadRow {
                    text: text.to_owned(),
                    line_num,
                    minus,
                });
            }
            return Ok(());
        }
        if text.trim().is_empty() {
            return Ok(());
        }
        if let Some(bare_range) = parse_top_level_bare_range_header(text) {
            validate_range(&bare_range, line_num, AbsoluteRangeOp::Replace, None)?;
            self.pending = Some(Pending {
                target: BlockTarget::Replace {
                    range: bare_range,
                    register: None,
                },
                line_num,
                payloads: Vec::new(),
                had_colon: true,
                deferred_blanks: Vec::new(),
            });
            self.warn_once(BARE_RANGE_AUTO_PUT_WARNING);
            return Ok(());
        }
        if let Some((snapshot_line, snapshot_text)) = parse_top_level_snapshot_row(text) {
            // Each recovered row becomes a single-line replacement, so a repeated line number
            // is not a set of replacements but a body written under one number.
            if self.recovered_snapshot_lines.contains(&snapshot_line) {
                return Err(format!(
                    "line {line_num}: {}",
                    repeated_snapshot_row_message(snapshot_line)
                ));
            }
            self.recovered_snapshot_lines.insert(snapshot_line);
            let range = ParsedRange {
                start: Anchor {
                    line: snapshot_line,
                },
                end: Anchor {
                    line: snapshot_line,
                },
            };
            validate_range(&range, line_num, AbsoluteRangeOp::Replace, None)?;
            self.push_insert(
                Cursor::BeforeAnchor {
                    anchor: Anchor {
                        line: snapshot_line,
                    },
                },
                snapshot_text.to_owned(),
                line_num,
                true,
            );
            self.push_delete_range(&range, line_num);
            self.warn_once(SNAPSHOT_ROWS_AUTO_PUT_WARNING);
            return Ok(());
        }
        Err(format!(
            "line {line_num}: payload line has no preceding hunk header. \
Use `PUT N.=M:`, `CUT N.=M`, or `PUT <N:`/`PUT >N:` above the body. Got {text:?}."
        ))
    }

    fn handle_blank(&mut self, text: &str, line_num: u64) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if bodyless_target_message(&pending.target, pending.had_colon).is_some() {
            return;
        }
        if pending.payloads.is_empty() {
            return;
        }
        pending.deferred_blanks.push(PayloadRow {
            text: text.to_owned(),
            line_num,
            minus: false,
        });
    }

    /// A blank row is a separator when it ends the body and a bare row when more body follows.
    fn commit_deferred_blanks(pending: &mut Pending) -> Result<(), String> {
        match pending.deferred_blanks.first() {
            Some(blank) => Err(format!("line {}: {BLANK_BODY_ROW_REFUSED}", blank.line_num)),
            None => Ok(()),
        }
    }

    fn resolve_minus_rows(
        payloads: &mut Vec<PayloadRow>,
        warnings: &mut Vec<String>,
    ) -> Result<(), String> {
        let mut first_minus: Option<u64> = None;
        let mut all_bullet_shaped = true;
        let mut has_explicit = false;
        let mut has_explicit_bullet = false;
        for row in payloads.iter() {
            if row.minus {
                first_minus.get_or_insert(row.line_num);
                all_bullet_shaped &= is_md_bullet_row(&row.text);
            } else {
                has_explicit = true;
                has_explicit_bullet |= is_md_bullet_row(&row.text);
            }
        }
        let Some(first_minus) = first_minus else {
            return Ok(());
        };
        if all_bullet_shaped && (!has_explicit || has_explicit_bullet) {
            if !warnings
                .iter()
                .any(|warning| warning == MINUS_BULLET_AUTO_PIPED_WARNING)
            {
                warnings.push(MINUS_BULLET_AUTO_PIPED_WARNING.to_owned());
            }
            return Ok(());
        }
        if has_explicit && !all_bullet_shaped {
            payloads.retain(|row| !row.minus);
            if !warnings
                .iter()
                .any(|warning| warning == DIFF_OLD_ROWS_IGNORED_WARNING)
            {
                warnings.push(DIFF_OLD_ROWS_IGNORED_WARNING.to_owned());
            }
            return Ok(());
        }
        Err(format!("line {first_minus}: {MINUS_ROW_REJECTED}"))
    }

    fn push_insert(&mut self, cursor: Cursor, text: String, line_num: u64, replacement: bool) {
        let index = self.edit_index;
        self.edit_index += 1;
        self.edits.push(Edit::Insert {
            cursor,
            text,
            line_num,
            index,
            replacement,
            block_start: None,
        });
    }

    fn push_delete(&mut self, anchor: Anchor, line_num: u64) {
        let index = self.edit_index;
        self.edit_index += 1;
        self.edits.push(Edit::Delete {
            anchor,
            line_num,
            index,
            old_assertion: None,
        });
    }

    fn push_delete_range(&mut self, range: &ParsedRange, line_num: u64) {
        for line in range.start.line..=range.end.line {
            self.push_delete(Anchor { line }, line_num);
        }
    }

    fn push_cut(&mut self, range: ParsedRange, line_num: u64, register: Option<String>) {
        let index = self.edit_index;
        self.edit_index += 1;
        self.edits.push(Edit::Cut {
            range,
            register,
            line_num,
            index,
        });
        // Capture before ordinary per-line deletes are applied. Keeping deletion
        // as low-level edits preserves overlap validation and recovery remapping.
        self.push_delete_range(&range, line_num);
    }

    fn push_paste(&mut self, at: PasteTarget, register: Option<String>, line_num: u64) {
        let index = self.edit_index;
        self.edit_index += 1;
        self.edits.push(Edit::Paste {
            at,
            register,
            line_num,
            index,
            block_start: None,
        });
    }

    fn push_block(
        &mut self,
        anchor: Anchor,
        payloads: &[PayloadRow],
        line_num: u64,
        mode: BlockMode,
        register: Option<String>,
    ) {
        let index = self.edit_index;
        self.edit_index += 1;
        self.edits.push(Edit::Block {
            anchor,
            payloads: payloads.iter().map(|row| row.text.clone()).collect(),
            mode,
            register,
            line_num,
            index,
        });
    }

    fn emit_payload_rows(
        &mut self,
        cursor: Cursor,
        payloads: &[PayloadRow],
        line_num: u64,
        replacement: bool,
    ) {
        for payload in payloads {
            self.push_insert(cursor, payload.text.clone(), line_num, replacement);
        }
    }

    fn flush_pending(&mut self) -> Result<(), String> {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        Self::resolve_minus_rows(&mut pending.payloads, &mut self.warnings)?;
        let Pending {
            target,
            line_num,
            payloads,
            had_colon,
            ..
        } = pending;
        match target {
            BlockTarget::Rem | BlockTarget::Move { .. } => Ok(()),
            BlockTarget::Cut { range, register } => {
                self.push_cut(range, line_num, register);
                Ok(())
            }
            BlockTarget::CutBlock { anchor, register } => {
                self.push_block(anchor, &[], line_num, BlockMode::Cut, register);
                Ok(())
            }
            // Span targets: body writes, register pastes over the span. The anonymous
            // register never does — too easy to fire by forgetting `:` + body on a replace.
            BlockTarget::Replace { range, register } => {
                if let Some(register) = register {
                    self.push_paste(PasteTarget::Span { range }, Some(register), line_num);
                    return Ok(());
                }
                if payloads.is_empty() {
                    if !had_colon {
                        return Err(format!("line {line_num}: {COLONLESS_SPAN_PUT}"));
                    }
                    return Err(format!(
                        "line {line_num}: {}",
                        empty_put_message(
                            &format_replace_header(range.start.line, range.end.line),
                            &format_cut_header(range.start.line, range.end.line),
                        )
                    ));
                }
                let cursor = Cursor::BeforeAnchor {
                    anchor: range.start,
                };
                self.emit_payload_rows(cursor, &payloads, line_num, true);
                self.push_delete_range(&range, line_num);
                Ok(())
            }
            BlockTarget::Block { anchor, register } => {
                if let Some(register) = register {
                    self.push_block(anchor, &[], line_num, BlockMode::Replace, Some(register));
                    return Ok(());
                }
                if payloads.is_empty() {
                    if !had_colon {
                        return Err(format!("line {line_num}: {COLONLESS_SPAN_PUT}"));
                    }
                    return Err(format!(
                        "line {line_num}: {}",
                        empty_put_message(
                            &format!("PUT {}*:", anchor.line),
                            &format!("CUT {}*", anchor.line),
                        )
                    ));
                }
                self.push_block(anchor, &payloads, line_num, BlockMode::Replace, None);
                Ok(())
            }
            // Gap targets: body inserts, register pastes, and the colonless
            // bodyless form is an anonymous paste.
            BlockTarget::InsertAfterBlock { anchor, register } => {
                if register.is_some() || (!had_colon && payloads.is_empty()) {
                    self.push_block(anchor, &[], line_num, BlockMode::PasteAfter, register);
                    return Ok(());
                }
                if payloads.is_empty() {
                    return Err(format!("line {line_num}: {EMPTY_INSERT}"));
                }
                self.push_block(anchor, &payloads, line_num, BlockMode::InsertAfter, None);
                Ok(())
            }
            BlockTarget::InsertBefore { .. }
            | BlockTarget::InsertAfter { .. }
            | BlockTarget::Bof { .. }
            | BlockTarget::Eof { .. } => {
                let (cursor, register) = match target {
                    BlockTarget::InsertBefore { anchor, register } => {
                        (Cursor::BeforeAnchor { anchor }, register)
                    }
                    BlockTarget::InsertAfter { anchor, register } => {
                        (Cursor::AfterAnchor { anchor }, register)
                    }
                    BlockTarget::Bof { register } => (Cursor::Bof, register),
                    BlockTarget::Eof { register } => (Cursor::Eof, register),
                    _ => return Ok(()),
                };
                if register.is_some() || (!had_colon && payloads.is_empty()) {
                    self.push_paste(PasteTarget::Gap { cursor }, register, line_num);
                    return Ok(());
                }
                if payloads.is_empty() {
                    return Err(format!("line {line_num}: {EMPTY_INSERT}"));
                }
                self.emit_payload_rows(cursor, &payloads, line_num, false);
                Ok(())
            }
        }
    }
}
