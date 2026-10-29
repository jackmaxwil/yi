use super::messages::{
    AbsoluteRangeOp, BlockDiagnosticSuggestions, REPLACE_PAIR_COALESCED_WARNING,
    block_single_line_message, block_unresolved_message, insert_after_block_closer_lowered_warning,
    insert_after_block_unresolved_message, paste_after_block_closer_lowered_warning,
    paste_after_block_unresolved_message,
};
use super::types::{
    Anchor, BlockMode, BlockResolver, BlockSpan, Cursor, Edit, ParsedRange, PasteTarget,
};

#[derive(Clone, Copy)]
pub enum BraceSyntax {
    Rust,
    JavaScript,
    CStyle,
    Plain,
}

impl BraceSyntax {
    fn rust_lifetimes(self) -> bool {
        matches!(self, Self::Rust)
    }

    fn block_comments(self) -> bool {
        !matches!(self, Self::Plain)
    }

    fn regex_literals(self) -> bool {
        matches!(self, Self::JavaScript)
    }
}

/// The block opening at `line` spans to where the scanner returns to its prior depth. None
/// when the anchor opens nothing, or when the delimiters never balance.
pub fn brace_block_resolver(text: &str, line: u64, syntax: BraceSyntax) -> Option<BlockSpan> {
    let lines: Vec<&str> = text.split('\n').collect();
    let anchor_index = usize::try_from(line.checked_sub(1)?).ok()?;
    let anchor = lines.get(anchor_index)?;
    let mut depth: i64 = 0;
    let mut in_block_comment = false;
    let mut opened = false;
    let mut carry = strip_line_noise(anchor, syntax, &mut in_block_comment, &[]);
    for &byte in &carry {
        match byte {
            b'{' | b'[' | b'(' => {
                depth += 1;
                opened = true;
            }
            b'}' | b']' | b')' => depth -= 1,
            _ => {}
        }
    }
    if !opened || depth <= 0 {
        return None;
    }
    for (offset, candidate) in lines.iter().enumerate().skip(anchor_index + 1) {
        let out = strip_line_noise(candidate, syntax, &mut in_block_comment, &carry);
        for &byte in &out {
            match byte {
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' => {
                    depth -= 1;
                    if depth <= 0 {
                        return Some(BlockSpan {
                            start: line,
                            end: offset as u64 + 1,
                        });
                    }
                }
                _ => {}
            }
        }
        if !blank(&out) {
            carry = out;
        }
    }
    None
}

/// The block opening at `line` runs through every following line indented deeper than it,
/// trailing blank lines excluded. A line with nothing deeper below it opens nothing.
pub fn indent_block_resolver(text: &str, line: u64) -> Option<BlockSpan> {
    let lines: Vec<&str> = text.split('\n').collect();
    let anchor_index = usize::try_from(line.checked_sub(1)?).ok()?;
    let anchor = lines.get(anchor_index)?;
    if anchor.trim().is_empty() {
        return None;
    }
    let depth = indent_width(anchor);
    let mut signature_depth = delimiter_delta(anchor);
    let mut end: Option<usize> = None;
    for (index, candidate) in lines.iter().enumerate().skip(anchor_index.checked_add(1)?) {
        if signature_depth > 0 {
            signature_depth = signature_depth.saturating_add(delimiter_delta(candidate));
            continue;
        }
        if candidate.trim().is_empty() {
            continue;
        }
        if indent_width(candidate) <= depth {
            break;
        }
        end = Some(index);
    }
    let end = u64::try_from(end?.checked_add(1)?).ok()?;
    Some(BlockSpan { start: line, end })
}

fn delimiter_delta(line: &str) -> i64 {
    let mut in_block_comment = false;
    strip_line_noise(line, BraceSyntax::Plain, &mut in_block_comment, &[])
        .into_iter()
        .fold(0_i64, |depth, byte| match byte {
            b'{' | b'[' | b'(' => depth.saturating_add(1),
            b'}' | b']' | b')' => depth.saturating_sub(1),
            _ => depth,
        })
}

fn indent_width(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .map(|byte| if byte == b'\t' { 4 } else { 1 })
        .sum()
}

fn blank(out: &[u8]) -> bool {
    out.iter().all(u8::is_ascii_whitespace)
}

/// Delimiters inside string/char literals and line comments do not count. A lexical pass, not
/// a parser: multi-line strings and block comments fail closed via unbalanced depth.
fn strip_line_noise(
    line: &str,
    syntax: BraceSyntax,
    in_block_comment: &mut bool,
    carry: &[u8],
) -> Vec<u8> {
    let bytes = line.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string: Option<u8> = None;
    if *in_block_comment {
        let Some(end) = bytes.windows(2).position(|pair| pair == b"*/") else {
            return out;
        };
        index = end.saturating_add(2);
        *in_block_comment = false;
    }
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(quote) = in_string {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == quote {
                in_string = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\''
                if syntax.rust_lifetimes()
                    && bytes
                        .get(index + 1)
                        .is_some_and(|next| next.is_ascii_alphabetic() || *next == b'_')
                    && bytes.get(index + 2) != Some(&b'\'') =>
            {
                out.push(byte)
            }
            b'/' if syntax.regex_literals()
                && regex_can_start(if blank(&out) { carry } else { &out })
                && bytes
                    .get(index + 1)
                    .is_some_and(|next| *next != b'/' && *next != b'*') =>
            {
                if let Some(end) = regex_literal_end(bytes, index.saturating_add(1)) {
                    index = end;
                    continue;
                }
                out.push(byte);
            }
            b'/' if syntax.block_comments() && bytes.get(index + 1) == Some(&b'*') => {
                let Some(rest) = bytes.get(index.saturating_add(2)..) else {
                    break;
                };
                let Some(end) = rest.windows(2).position(|pair| pair == b"*/") else {
                    *in_block_comment = true;
                    break;
                };
                index = index
                    .saturating_add(2)
                    .saturating_add(end)
                    .saturating_add(2);
                continue;
            }
            b'"' | b'\'' | b'`' => in_string = Some(byte),
            b'/' if bytes.get(index + 1) == Some(&b'/') => break,
            b'#' if index == 0 || bytes[index - 1].is_ascii_whitespace() => break,
            _ => out.push(byte),
        }
        index += 1;
    }
    out
}

fn regex_can_start(out: &[u8]) -> bool {
    let Some((index, previous)) = out
        .iter()
        .enumerate()
        .rev()
        .find(|(_, byte)| !byte.is_ascii_whitespace())
    else {
        return false;
    };
    if matches!(
        *previous,
        b'(' | b'['
            | b'{'
            | b'='
            | b':'
            | b','
            | b';'
            | b'!'
            | b'?'
            | b'&'
            | b'|'
            | b'+'
            | b'-'
            | b'*'
            | b'%'
            | b'^'
            | b'~'
            | b'<'
            | b'>'
    ) {
        return true;
    }
    let Some(tail) = out.get(..=index) else {
        return false;
    };
    [
        b"return".as_slice(),
        b"throw".as_slice(),
        b"case".as_slice(),
        b"delete".as_slice(),
        b"void".as_slice(),
        b"typeof".as_slice(),
        b"instanceof".as_slice(),
        b"yield".as_slice(),
        b"await".as_slice(),
        b"else".as_slice(),
        b"do".as_slice(),
        b"in".as_slice(),
        b"of".as_slice(),
    ]
    .into_iter()
    .any(|keyword| {
        if !tail.ends_with(keyword) {
            return false;
        }
        let boundary = tail
            .len()
            .checked_sub(keyword.len())
            .and_then(|start| start.checked_sub(1))
            .and_then(|before| tail.get(before));
        boundary.is_none_or(|byte| !byte.is_ascii_alphanumeric() && !matches!(*byte, b'_' | b'$'))
    })
}

fn regex_literal_end(bytes: &[u8], mut cursor: usize) -> Option<usize> {
    let mut in_class = false;
    while let Some(byte) = bytes.get(cursor) {
        match *byte {
            b'\\' => cursor = cursor.saturating_add(2),
            b'[' => {
                in_class = true;
                cursor = cursor.saturating_add(1);
            }
            b']' => {
                in_class = false;
                cursor = cursor.saturating_add(1);
            }
            b'/' if !in_class => return cursor.checked_add(1),
            _ => cursor = cursor.saturating_add(1),
        }
    }
    None
}

fn is_closer_line(text: &str, line: u64) -> bool {
    let Some(candidate) = text.split('\n').nth(line.saturating_sub(1) as usize) else {
        return false;
    };
    let trimmed = candidate.trim();
    !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|byte| matches!(byte, b'}' | b']' | b')' | b';' | b',' | b' ' | b'\t'))
}

pub struct ResolvedBlocks {
    pub edits: Vec<Edit>,
    pub warnings: Vec<String>,
}

pub fn resolve_block_edits(
    edits: Vec<Edit>,
    path: &str,
    text: &str,
    resolver: &BlockResolver,
) -> Result<ResolvedBlocks, String> {
    if !edits.iter().any(|edit| matches!(edit, Edit::Block { .. })) {
        return Ok(ResolvedBlocks {
            edits,
            warnings: Vec::new(),
        });
    }
    let file_lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let mut out: Vec<Edit> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for edit in edits {
        let Edit::Block {
            anchor,
            payloads,
            mode,
            register,
            line_num,
        } = edit
        else {
            out.push(edit);
            continue;
        };
        let span = resolver(&super::types::BlockResolverRequest {
            path,
            text,
            line: anchor.line,
        });
        match mode {
            BlockMode::Replace | BlockMode::Cut => {
                let op = match mode {
                    BlockMode::Cut => AbsoluteRangeOp::Cut,
                    _ => AbsoluteRangeOp::Replace,
                };
                let Some(span) = span else {
                    return Err(format!(
                        "line {line_num}: {}",
                        block_unresolved_message(
                            anchor.line,
                            op,
                            Some(&file_lines),
                            BlockDiagnosticSuggestions::default(),
                            register.as_deref(),
                        )
                    ));
                };
                if span.end <= span.start {
                    return Err(format!(
                        "line {line_num}: {}",
                        block_single_line_message(anchor.line, mode, None)
                    ));
                }
                let range = ParsedRange {
                    start: Anchor { line: span.start },
                    end: Anchor { line: span.end },
                };
                let register_present = register.is_some();
                if matches!(mode, BlockMode::Cut) {
                    out.push(Edit::Cut {
                        range,
                        register,
                        line_num,
                    });
                } else if let Some(register) = register {
                    out.push(Edit::Paste {
                        at: PasteTarget::Span { range },
                        register: Some(register),
                        line_num,
                    });
                } else {
                    for text in &payloads {
                        out.push(Edit::Insert {
                            cursor: Cursor::BeforeAnchor {
                                anchor: Anchor { line: span.start },
                            },
                            text: text.clone(),
                            line_num,
                            replacement: true,
                        });
                    }
                }
                if !(matches!(mode, BlockMode::Replace) && register_present) {
                    for line in span.start..=span.end {
                        out.push(Edit::Delete {
                            anchor: Anchor { line },
                            line_num,
                        });
                    }
                }
            }
            BlockMode::InsertAfter | BlockMode::PasteAfter => {
                let (landing, warning) = match span {
                    Some(span) => (span.end, None),
                    None if is_closer_line(text, anchor.line) => (
                        anchor.line,
                        Some(match mode {
                            BlockMode::PasteAfter => {
                                paste_after_block_closer_lowered_warning(anchor.line)
                            }
                            _ => insert_after_block_closer_lowered_warning(anchor.line),
                        }),
                    ),
                    None => {
                        let message = match mode {
                            BlockMode::PasteAfter => {
                                paste_after_block_unresolved_message(anchor.line)
                            }
                            _ => insert_after_block_unresolved_message(anchor.line),
                        };
                        return Err(format!("line {line_num}: {message}"));
                    }
                };
                if let Some(warning) = warning {
                    warnings.push(warning);
                }
                let cursor = Cursor::AfterAnchor {
                    anchor: Anchor { line: landing },
                };
                if matches!(mode, BlockMode::PasteAfter) {
                    out.push(Edit::Paste {
                        at: PasteTarget::Gap { cursor },
                        register,
                        line_num,
                    });
                } else {
                    for text in &payloads {
                        out.push(Edit::Insert {
                            cursor,
                            text: text.clone(),
                            line_num,
                            replacement: false,
                        });
                    }
                }
            }
        }
    }
    // Block ops became lines just now; two hunks over one line are judged like ranged ones.
    if super::parser::coalesce_overlapping_ranges(&mut out)? {
        warnings.push(REPLACE_PAIR_COALESCED_WARNING.to_owned());
    }
    Ok(ResolvedBlocks {
        edits: out,
        warnings,
    })
}

pub fn heading_level(line: &str) -> Option<usize> {
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    ((1..=6).contains(&level) && line.as_bytes().get(level) == Some(&b' ')).then_some(level)
}

/// A Markdown section: the heading at or above `line` through the line before the next heading
/// of the same or a higher level. Fenced code is skipped, since `#` inside it is a comment.
pub fn markdown_section_resolver(text: &str, line: u64) -> Option<BlockSpan> {
    let lines: Vec<&str> = text.split('\n').collect();
    let index = usize::try_from(line).ok()?.checked_sub(1)?;
    let mut fenced = false;
    let mut headings: Vec<(usize, usize)> = Vec::new();
    for (at, text) in lines.iter().enumerate() {
        if text.starts_with("```") {
            fenced = !fenced;
        } else if !fenced && let Some(level) = heading_level(text) {
            headings.push((at, level));
        }
    }
    let (start, depth) = *headings.iter().rev().find(|(at, _)| *at <= index)?;
    let end = headings
        .iter()
        .find(|(at, level)| *at > start && *level <= depth)
        .map_or(lines.len(), |(at, _)| *at);
    Some(BlockSpan {
        start: u64::try_from(start).ok()?.saturating_add(1),
        end: u64::try_from(end).ok()?,
    })
}
