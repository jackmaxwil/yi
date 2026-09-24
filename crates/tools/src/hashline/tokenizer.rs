use super::format::{
    HL_CUT_KEYWORD, HL_FILE_HASH_LENGTH, HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX,
    HL_MOVE_KEYWORD, HL_PUT_KEYWORD, HL_REM_KEYWORD, describe_anchor_examples,
};
use super::messages::{ABORT_MARKER, BEGIN_PATCH_MARKER, END_PATCH_MARKER};
use super::types::{Anchor, ParsedRange};

fn is_whitespace(byte: u8) -> bool {
    byte == b' ' || (b'\t'..=b'\r').contains(&byte)
}

fn skip_whitespace(line: &[u8], mut index: usize, end: usize) -> usize {
    while index < end && is_whitespace(line[index]) {
        index += 1;
    }
    index
}

fn trim_end_index(line: &[u8]) -> usize {
    let mut end = line.len();
    while end > 0 && is_whitespace(line[end - 1]) {
        end -= 1;
    }
    end
}

fn marker_line_equals(line: &str, marker: &str) -> bool {
    let end = trim_end_index(line.as_bytes());
    end == marker.len() && line.starts_with(marker)
}

pub fn split_hashline_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![""];
    }
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte != b'\n' {
            continue;
        }
        let mut stop = index;
        if stop > start && bytes[stop - 1] == b'\r' {
            stop -= 1;
        }
        lines.push(&text[start..stop]);
        start = index + 1;
    }
    if start < text.len() {
        let mut stop = text.len();
        if stop > start && bytes[stop - 1] == b'\r' {
            stop -= 1;
        }
        lines.push(&text[start..stop]);
    }
    lines
}

struct NumberScan {
    line: u64,
    next_index: usize,
}

fn scan_line_number(line: &[u8], index: usize, end: usize) -> Option<NumberScan> {
    if index >= end || !(b'1'..=b'9').contains(&line[index]) {
        return None;
    }
    let mut value: u64 = 0;
    let mut next_index = index;
    while next_index < end {
        let byte = line[next_index];
        if !byte.is_ascii_digit() {
            break;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
        next_index += 1;
    }
    Some(NumberScan {
        line: value,
        next_index,
    })
}

pub fn parse_lid(raw: &str, line_num: u64) -> Result<Anchor, String> {
    let bytes = raw.as_bytes();
    let end = trim_end_index(bytes);
    let number_start = skip_whitespace(bytes, 0, end);
    let number = scan_line_number(bytes, number_start, end);
    match number {
        Some(number) if skip_whitespace(bytes, number.next_index, end) == end => {
            Ok(Anchor { line: number.line })
        }
        _ => Err(format!(
            "line {line_num}: expected a line number such as {}; got {raw:?}. Use {HL_FILE_PREFIX}PATH{HL_FILE_HASH_SEP}hash{HL_FILE_SUFFIX} from your latest read for file-version binding.",
            describe_anchor_examples("119")
        )),
    }
}

struct RangeScan {
    range: ParsedRange,
    next_index: usize,
    had_separator: bool,
}

const ELLIPSIS: char = '\u{2026}';

fn separator_char_len(line: &str, index: usize) -> Option<usize> {
    let rest = &line[index..];
    if rest.starts_with(ELLIPSIS) {
        return Some(ELLIPSIS.len_utf8());
    }
    match rest.as_bytes().first() {
        Some(b'-') | Some(b'.') | Some(b'=') => Some(1),
        _ => None,
    }
}

fn scan_range_separator(line: &str, index: usize, end: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut cursor = index;
    let mut consumed = false;
    while cursor < end {
        if let Some(len) = separator_char_len(line, cursor) {
            cursor += len;
            consumed = true;
            continue;
        }
        if is_whitespace(bytes[cursor]) {
            cursor += 1;
            consumed = true;
            continue;
        }
        break;
    }
    if !consumed || cursor >= end || !(b'1'..=b'9').contains(&bytes[cursor]) {
        return None;
    }
    Some(cursor)
}

fn scan_dangling_separator(line: &str, index: usize, end: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut cursor = index;
    let mut saw_separator = false;
    while cursor < end {
        if let Some(len) = separator_char_len(line, cursor) {
            cursor += len;
            saw_separator = true;
            continue;
        }
        if is_whitespace(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        break;
    }
    if !saw_separator {
        return None;
    }
    if cursor < end {
        let byte = bytes[cursor];
        if byte != b':' && byte != b'@' {
            return None;
        }
    }
    Some(cursor)
}

fn scan_header_range(
    line: &str,
    index: usize,
    end: usize,
    allow_single: bool,
) -> Option<RangeScan> {
    let bytes = line.as_bytes();
    let number_start = skip_whitespace(bytes, index, end);
    let start = scan_line_number(bytes, number_start, end)?;
    let Some(after_first) = scan_range_separator(line, start.next_index, end) else {
        if !allow_single {
            return None;
        }
        if let Some(dangling) = scan_dangling_separator(line, start.next_index, end) {
            return Some(RangeScan {
                range: ParsedRange {
                    start: Anchor { line: start.line },
                    end: Anchor { line: start.line },
                },
                next_index: dangling,
                had_separator: true,
            });
        }
        return Some(RangeScan {
            range: ParsedRange {
                start: Anchor { line: start.line },
                end: Anchor { line: start.line },
            },
            next_index: skip_whitespace(bytes, start.next_index, end),
            had_separator: false,
        });
    };
    let end_number = scan_line_number(bytes, after_first, end)?;
    Some(RangeScan {
        range: ParsedRange {
            start: Anchor { line: start.line },
            end: Anchor {
                line: end_number.line,
            },
        },
        next_index: skip_whitespace(bytes, end_number.next_index, end),
        had_separator: true,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockTarget {
    Replace {
        range: ParsedRange,
        register: Option<String>,
    },
    Block {
        anchor: Anchor,
        register: Option<String>,
    },
    InsertBefore {
        anchor: Anchor,
        register: Option<String>,
    },
    InsertAfter {
        anchor: Anchor,
        register: Option<String>,
    },
    InsertAfterBlock {
        anchor: Anchor,
        register: Option<String>,
    },
    Cut {
        range: ParsedRange,
        register: Option<String>,
    },
    CutBlock {
        anchor: Anchor,
        register: Option<String>,
    },
    Bof {
        register: Option<String>,
    },
    Eof {
        register: Option<String>,
    },
    Rem,
    Move {
        dest: String,
    },
}

impl BlockTarget {
    fn with_register(self, name: String) -> Self {
        match self {
            Self::Replace { range, .. } => Self::Replace {
                range,
                register: Some(name),
            },
            Self::Block { anchor, .. } => Self::Block {
                anchor,
                register: Some(name),
            },
            Self::InsertBefore { anchor, .. } => Self::InsertBefore {
                anchor,
                register: Some(name),
            },
            Self::InsertAfter { anchor, .. } => Self::InsertAfter {
                anchor,
                register: Some(name),
            },
            Self::InsertAfterBlock { anchor, .. } => Self::InsertAfterBlock {
                anchor,
                register: Some(name),
            },
            Self::Cut { range, .. } => Self::Cut {
                range,
                register: Some(name),
            },
            Self::CutBlock { anchor, .. } => Self::CutBlock {
                anchor,
                register: Some(name),
            },
            Self::Bof { .. } => Self::Bof {
                register: Some(name),
            },
            Self::Eof { .. } => Self::Eof {
                register: Some(name),
            },
            other => other,
        }
    }

    pub fn register(&self) -> Option<&str> {
        match self {
            Self::Replace { register, .. }
            | Self::Block { register, .. }
            | Self::InsertBefore { register, .. }
            | Self::InsertAfter { register, .. }
            | Self::InsertAfterBlock { register, .. }
            | Self::Cut { register, .. }
            | Self::CutBlock { register, .. }
            | Self::Bof { register }
            | Self::Eof { register } => register.as_deref(),
            Self::Rem | Self::Move { .. } => None,
        }
    }
}

pub struct TargetScan {
    pub target: BlockTarget,
    pub had_colon: bool,
}

fn scan_keyword(line: &[u8], index: usize, end: usize, keyword: &str) -> Option<usize> {
    let bytes = keyword.as_bytes();
    if end.saturating_sub(index) < bytes.len() || &line[index..index + bytes.len()] != bytes {
        return None;
    }
    let next = index + bytes.len();
    if next < end {
        let byte = line[next];
        if !is_whitespace(byte) && byte != b':' {
            return None;
        }
    }
    Some(next)
}

struct ColonScan {
    next_index: usize,
    had_colon: bool,
}

fn consume_optional_colon(line: &[u8], index: usize, end: usize) -> ColonScan {
    let cursor = skip_whitespace(line, index, end);
    if cursor < end && line[cursor] == b':' {
        ColonScan {
            next_index: skip_whitespace(line, cursor + 1, end),
            had_colon: true,
        }
    } else {
        ColonScan {
            next_index: cursor,
            had_colon: false,
        }
    }
}

/// Maximum accepted register-name length; anything longer fails the header parse.
const REGISTER_NAME_MAX: usize = 64;

fn is_register_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn scan_register(line: &str, index: usize, end: usize) -> Option<(String, usize)> {
    let bytes = line.as_bytes();
    if index >= end || bytes[index] != b'@' {
        return None;
    }
    let start = index + 1;
    let mut cursor = start;
    while cursor < end && is_register_name_byte(bytes[cursor]) {
        cursor += 1;
    }
    if cursor == start || cursor - start > REGISTER_NAME_MAX {
        return None;
    }
    Some((line[start..cursor].to_owned(), cursor))
}

fn finish_target_scan(
    line: &str,
    index: usize,
    end: usize,
    mut target: BlockTarget,
) -> Option<(TargetScan, usize)> {
    let bytes = line.as_bytes();
    let mut cursor = skip_whitespace(bytes, index, end);
    if let Some((name, next_index)) = scan_register(line, cursor, end) {
        target = target.with_register(name);
        cursor = next_index;
    }
    let colon = consume_optional_colon(bytes, cursor, end);
    Some((
        TargetScan {
            target,
            had_colon: colon.had_colon,
        },
        colon.next_index,
    ))
}

fn scan_put_target(line: &str, index: usize, end: usize) -> Option<(TargetScan, usize)> {
    let bytes = line.as_bytes();
    let cursor = skip_whitespace(bytes, index, end);
    if cursor >= end {
        return None;
    }
    let sigil = bytes[cursor];
    if sigil == b'<' || sigil == b'>' {
        let is_after = sigil == b'>';
        let probe = skip_whitespace(bytes, cursor + 1, end);
        if is_after && probe < end && bytes[probe] == b'$' {
            return finish_target_scan(line, probe + 1, end, BlockTarget::Eof { register: None });
        }
        let anchor = scan_line_number(bytes, probe, end)?;
        let mut next = anchor.next_index;
        let mut block = false;
        if next < end && bytes[next] == b'*' {
            block = true;
            next += 1;
        }
        if is_after {
            let target = if block {
                BlockTarget::InsertAfterBlock {
                    anchor: Anchor { line: anchor.line },
                    register: None,
                }
            } else {
                BlockTarget::InsertAfter {
                    anchor: Anchor { line: anchor.line },
                    register: None,
                }
            };
            return finish_target_scan(line, next, end, target);
        }
        // `<N*` is the same gap as `<N`, so the star drops. `<1` is head, mapped to `bof` so
        // it stays position-stable, never anchor-scoped, and works on empty files.
        let target = if anchor.line == 1 {
            BlockTarget::Bof { register: None }
        } else {
            BlockTarget::InsertBefore {
                anchor: Anchor { line: anchor.line },
                register: None,
            }
        };
        return finish_target_scan(line, next, end, target);
    }
    let range = scan_header_range(line, cursor, end, true)?;
    let next = range.next_index;
    if next < end && bytes[next] == b'*' {
        // Block locators are single opening lines (`N*`), never ranges.
        if range.had_separator {
            return None;
        }
        return finish_target_scan(
            line,
            next + 1,
            end,
            BlockTarget::Block {
                anchor: Anchor {
                    line: range.range.start.line,
                },
                register: None,
            },
        );
    }
    finish_target_scan(
        line,
        next,
        end,
        BlockTarget::Replace {
            range: range.range,
            register: None,
        },
    )
}

fn scan_cut_target(line: &str, index: usize, end: usize) -> Option<(TargetScan, usize)> {
    let bytes = line.as_bytes();
    let range = scan_header_range(line, index, end, true)?;
    let next = range.next_index;
    if next < end && bytes[next] == b'*' {
        if range.had_separator {
            return None;
        }
        return finish_target_scan(
            line,
            next + 1,
            end,
            BlockTarget::CutBlock {
                anchor: Anchor {
                    line: range.range.start.line,
                },
                register: None,
            },
        );
    }
    finish_target_scan(
        line,
        next,
        end,
        BlockTarget::Cut {
            range: range.range,
            register: None,
        },
    )
}

fn unquote_path(path_text: &str) -> &str {
    if path_text.len() < 2 {
        return path_text;
    }
    let bytes = path_text.as_bytes();
    let first = bytes[0];
    let last = bytes[path_text.len() - 1];
    if (first == b'"' || first == b'\'') && first == last {
        return &path_text[1..path_text.len() - 1];
    }
    path_text
}

fn scan_move_dest(line: &str, index: usize, end: usize) -> Option<String> {
    let bytes = line.as_bytes();
    let cursor = skip_whitespace(bytes, index, end);
    if cursor >= end {
        return None;
    }
    let first = bytes[cursor];
    if first == b'"' || first == b'\'' {
        let mut next = cursor + 1;
        while next < end {
            let byte = bytes[next];
            if byte == b'\\' && next + 1 < end {
                next += 2;
                continue;
            }
            if byte == first {
                let after = skip_whitespace(bytes, next + 1, end);
                if after == end {
                    return Some(unquote_path(&line[cursor..next + 1]).to_owned());
                }
                return None;
            }
            next += 1;
        }
        return None;
    }
    Some(unquote_path(line[cursor..end].trim()).to_owned())
}

fn scan_hunk_anchor(line: &str, start: usize, end: usize) -> Option<(TargetScan, usize)> {
    let bytes = line.as_bytes();
    let cursor = skip_whitespace(bytes, start, end);

    if let Some(rem_end) = scan_keyword(bytes, cursor, end, HL_REM_KEYWORD) {
        let next = skip_whitespace(bytes, rem_end, end);
        if next != end {
            return None;
        }
        return Some((
            TargetScan {
                target: BlockTarget::Rem,
                had_colon: false,
            },
            next,
        ));
    }
    if let Some(move_end) = scan_keyword(bytes, cursor, end, HL_MOVE_KEYWORD) {
        let dest = scan_move_dest(line, move_end, end)?;
        if dest.is_empty() {
            return None;
        }
        return Some((
            TargetScan {
                target: BlockTarget::Move { dest },
                had_colon: false,
            },
            end,
        ));
    }
    if let Some(put_end) = scan_keyword(bytes, cursor, end, HL_PUT_KEYWORD) {
        return scan_put_target(line, put_end, end);
    }
    if let Some(cut_end) = scan_keyword(bytes, cursor, end, HL_CUT_KEYWORD) {
        return scan_cut_target(line, cut_end, end);
    }
    None
}

pub fn try_parse_hunk_header(line: &str) -> Option<TargetScan> {
    let bytes = line.as_bytes();
    let end = trim_end_index(bytes);
    let start = skip_whitespace(bytes, 0, end);
    if start >= end {
        return None;
    }
    let (scan, next_index) = scan_hunk_anchor(line, start, end)?;
    if next_index != end {
        return None;
    }
    Some(scan)
}

pub fn is_hunk_header_text(text: &str) -> bool {
    let bytes = text.as_bytes();
    let end = trim_end_index(bytes);
    let lead = skip_whitespace(bytes, 0, end);
    let rest = &text[lead..];
    let is_hunk_lead = rest.starts_with(HL_PUT_KEYWORD)
        || rest.starts_with(HL_CUT_KEYWORD)
        || rest.starts_with(HL_REM_KEYWORD)
        || rest.starts_with(HL_MOVE_KEYWORD);
    is_hunk_lead && try_parse_hunk_header(text).is_some()
}

/// `PUT 40.:=40:` read as `PUT 40.=40:`: kept only when every one-mark repair names one op.
pub fn near_miss_hunk_header(text: &str) -> Option<(String, TargetScan)> {
    let lead = text.trim_start();
    let keyword = [
        HL_PUT_KEYWORD,
        HL_CUT_KEYWORD,
        HL_REM_KEYWORD,
        HL_MOVE_KEYWORD,
    ]
    .into_iter()
    .any(|keyword| lead.starts_with(keyword));
    if !keyword {
        return None;
    }
    let mut found: Option<(String, TargetScan)> = None;
    for (skip, mark) in text.chars().enumerate() {
        if mark.is_alphanumeric() || mark.is_whitespace() {
            continue;
        }
        let repaired: String = text
            .chars()
            .enumerate()
            .filter_map(|(at, kept)| (at != skip).then_some(kept))
            .collect();
        let Some(scan) = try_parse_hunk_header(&repaired) else {
            continue;
        };
        match &found {
            Some((_, prior))
                if prior.target != scan.target || prior.had_colon != scan.had_colon =>
            {
                return None;
            }
            Some(_) => {}
            None => found = Some((repaired, scan)),
        }
    }
    found
}

pub struct ParsedHeader {
    pub path: String,
    pub file_hash: Option<String>,
}

pub fn try_parse_header(line: &str) -> Option<ParsedHeader> {
    if !line.starts_with(HL_FILE_PREFIX) {
        return None;
    }
    let bytes = line.as_bytes();
    let end = trim_end_index(bytes);
    let prefix_len = HL_FILE_PREFIX.len();
    let suffix_len = HL_FILE_SUFFIX.len();
    if prefix_len + suffix_len >= end {
        return None;
    }
    if !line[..end].ends_with(HL_FILE_SUFFIX) {
        return None;
    }
    let body_end = end - suffix_len;
    if prefix_len >= body_end {
        return None;
    }

    // The snapshot tag is the trailing `#XXXX` block inside the bracketed header, detected
    // from the suffix so the path may contain whitespace (`OneDrive - Company/file.ts`).
    let mut path_end = body_end;
    let mut file_hash = None;
    if body_end > HL_FILE_HASH_LENGTH {
        let trailing_hash_start = body_end - HL_FILE_HASH_LENGTH - 1;
        if trailing_hash_start >= prefix_len
            && bytes[trailing_hash_start] == b'#'
            && bytes[trailing_hash_start + 1..body_end]
                .iter()
                .all(u8::is_ascii_hexdigit)
        {
            path_end = trailing_hash_start;
            file_hash = Some(line[trailing_hash_start + 1..body_end].to_uppercase());
        }
    }

    // `#` is the path/tag separator and is banned inside a filename, so any `#` left in the
    // path body — a short, non-hex, over-long or stale tag — means a malformed header.
    if bytes[prefix_len..path_end].contains(&b'#') {
        return None;
    }

    if path_end == prefix_len {
        return None;
    }
    Some(ParsedHeader {
        path: line[prefix_len..path_end].to_owned(),
        file_hash,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Blank,
    EnvelopeBegin,
    EnvelopeEnd,
    Abort,
    Header {
        path: String,
        file_hash: Option<String>,
    },
    OpBlock {
        target: BlockTarget,
        had_colon: bool,
    },
    PayloadLiteral {
        text: String,
    },
    Raw {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line_num: u64,
}

pub fn classify_line(line: &str, line_num: u64) -> Token {
    let kind = classify_line_kind(line);
    Token { kind, line_num }
}

fn classify_line_kind(line: &str) -> TokenKind {
    if line.is_empty() {
        return TokenKind::Blank;
    }
    if marker_line_equals(line, BEGIN_PATCH_MARKER) {
        return TokenKind::EnvelopeBegin;
    }
    if marker_line_equals(line, END_PATCH_MARKER) {
        return TokenKind::EnvelopeEnd;
    }
    if marker_line_equals(line, ABORT_MARKER) {
        return TokenKind::Abort;
    }
    if line.starts_with(HL_FILE_PREFIX)
        && let Some(header) = try_parse_header(line)
    {
        return TokenKind::Header {
            path: header.path,
            file_hash: header.file_hash,
        };
    }
    let bytes = line.as_bytes();
    let lead = skip_whitespace(bytes, 0, bytes.len());
    let rest = &line[lead..];
    let is_hunk_lead = rest.starts_with(HL_PUT_KEYWORD)
        || rest.starts_with(HL_CUT_KEYWORD)
        || rest.starts_with(HL_REM_KEYWORD)
        || rest.starts_with(HL_MOVE_KEYWORD);
    if is_hunk_lead && let Some(hunk) = try_parse_hunk_header(line) {
        return TokenKind::OpBlock {
            target: hunk.target,
            had_colon: hunk.had_colon,
        };
    }
    if bytes[0] == b'+' {
        return TokenKind::PayloadLiteral {
            text: line[1..].to_owned(),
        };
    }
    TokenKind::Raw {
        text: line.to_owned(),
    }
}

pub fn tokenize_all(text: &str) -> Vec<Token> {
    if text.is_empty() {
        return Vec::new();
    }
    split_hashline_lines(text)
        .into_iter()
        .enumerate()
        .map(|(index, line)| classify_line(line, index as u64 + 1))
        .collect()
}
