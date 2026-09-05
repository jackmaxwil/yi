use super::messages::{
    AbsoluteRangeOp, BLOCK_RESOLVER_UNAVAILABLE, BlockDiagnosticSuggestions,
    block_single_line_message, block_unresolved_message, insert_after_block_closer_lowered_warning,
    insert_after_block_unresolved_lowered_warning, paste_after_block_closer_lowered_warning,
    paste_after_block_unresolved_lowered_warning,
};
use super::types::{
    Anchor, BlockMode, BlockResolution, BlockResolver, BlockSpan, Cursor, Edit, ParsedRange,
    PasteTarget,
};

/// The block opening at `line` spans to where the scanner returns to its prior depth. None
/// when the anchor opens nothing, or when the delimiters never balance.
pub fn brace_block_resolver(text: &str, line: u64) -> Option<BlockSpan> {
    let lines: Vec<&str> = text.split('\n').collect();
    let anchor_index = usize::try_from(line.checked_sub(1)?).ok()?;
    let anchor = lines.get(anchor_index)?;
    let mut depth: i64 = 0;
    let mut opened = false;
    for byte in strip_line_noise(anchor) {
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
        for byte in strip_line_noise(candidate) {
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
    let mut end: Option<usize> = None;
    for (index, candidate) in lines.iter().enumerate().skip(anchor_index.checked_add(1)?) {
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

fn indent_width(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .map(|byte| if byte == b'\t' { 4 } else { 1 })
        .sum()
}

/// Delimiters inside string/char literals and line comments do not count. A lexical pass, not
/// a parser: multi-line strings and block comments fail closed via unbalanced depth.
fn strip_line_noise(line: &str) -> Vec<u8> {
    let bytes = line.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string: Option<u8> = None;
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
            b'"' | b'\'' | b'`' => in_string = Some(byte),
            b'/' if bytes.get(index + 1) == Some(&b'/') => break,
            b'#' if index == 0 || bytes[index - 1].is_ascii_whitespace() => break,
            _ => out.push(byte),
        }
        index += 1;
    }
    out
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
    pub resolutions: Vec<BlockResolution>,
}

pub fn resolve_block_edits(
    edits: Vec<Edit>,
    path: &str,
    text: &str,
    resolver: Option<&BlockResolver>,
) -> Result<ResolvedBlocks, String> {
    if !edits.iter().any(|edit| matches!(edit, Edit::Block { .. })) {
        return Ok(ResolvedBlocks {
            edits,
            warnings: Vec::new(),
            resolutions: Vec::new(),
        });
    }
    let file_lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let mut out: Vec<Edit> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut resolutions: Vec<BlockResolution> = Vec::new();
    let mut synth_index = edits.len();
    for edit in edits {
        let Edit::Block {
            anchor,
            payloads,
            mode,
            register,
            line_num,
            ..
        } = edit
        else {
            out.push(edit);
            continue;
        };
        let span = resolver.and_then(|resolve| {
            resolve(&super::types::BlockResolverRequest {
                path,
                text,
                line: anchor.line,
            })
        });
        match mode {
            BlockMode::Replace | BlockMode::Cut => {
                let op = match mode {
                    BlockMode::Cut => AbsoluteRangeOp::Cut,
                    _ => AbsoluteRangeOp::Replace,
                };
                let Some(span) = span else {
                    if resolver.is_none() {
                        return Err(format!("line {line_num}: {BLOCK_RESOLVER_UNAVAILABLE}"));
                    }
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
                resolutions.push(BlockResolution {
                    start: span.start,
                    end: span.end,
                    op: mode,
                });
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
                        index: synth_index,
                    });
                    synth_index += 1;
                } else if let Some(register) = register {
                    out.push(Edit::Paste {
                        at: PasteTarget::Span { range },
                        register: Some(register),
                        line_num,
                        index: synth_index,
                        block_start: Some(span.start),
                    });
                    synth_index += 1;
                } else {
                    for text in &payloads {
                        out.push(Edit::Insert {
                            cursor: Cursor::BeforeAnchor {
                                anchor: Anchor { line: span.start },
                            },
                            text: text.clone(),
                            line_num,
                            index: synth_index,
                            replacement: true,
                            block_start: Some(span.start),
                        });
                        synth_index += 1;
                    }
                }
                if !(matches!(mode, BlockMode::Replace) && register_present) {
                    for line in span.start..=span.end {
                        out.push(Edit::Delete {
                            anchor: Anchor { line },
                            line_num,
                            index: synth_index,
                            old_assertion: None,
                        });
                        synth_index += 1;
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
                    None => (
                        anchor.line,
                        Some(match mode {
                            BlockMode::PasteAfter => {
                                paste_after_block_unresolved_lowered_warning(anchor.line)
                            }
                            _ => insert_after_block_unresolved_lowered_warning(anchor.line),
                        }),
                    ),
                };
                if let Some(warning) = warning {
                    warnings.push(warning);
                } else if let Some(span) = span {
                    resolutions.push(BlockResolution {
                        start: span.start,
                        end: span.end,
                        op: mode,
                    });
                }
                let cursor = Cursor::AfterAnchor {
                    anchor: Anchor { line: landing },
                };
                if matches!(mode, BlockMode::PasteAfter) {
                    out.push(Edit::Paste {
                        at: PasteTarget::Gap { cursor },
                        register,
                        line_num,
                        index: synth_index,
                        block_start: span.map(|span| span.start),
                    });
                    synth_index += 1;
                } else {
                    for text in &payloads {
                        out.push(Edit::Insert {
                            cursor,
                            text: text.clone(),
                            line_num,
                            index: synth_index,
                            replacement: false,
                            block_start: span.map(|span| span.start),
                        });
                        synth_index += 1;
                    }
                }
            }
        }
    }
    Ok(ResolvedBlocks {
        edits: out,
        warnings,
        resolutions,
    })
}
