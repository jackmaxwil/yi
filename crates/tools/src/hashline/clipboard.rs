use super::messages::{
    EMPTY_PASTE, ambiguous_anonymous_paste_message, empty_register_paste_warning,
    empty_register_span_paste_message,
};
use super::types::{Anchor, Clipboard, Cursor, Edit, PasteTarget};

fn describe_cut(range: &super::types::ParsedRange, register: Option<&str>) -> String {
    let span = if range.start.line == range.end.line {
        format!("{}", range.start.line)
    } else {
        format!("{}.={}", range.start.line, range.end.line)
    };
    let reg = register.map(|name| format!(" @{name}")).unwrap_or_default();
    format!("CUT {span}{reg}")
}

pub fn has_clipboard_edit(edits: &[Edit]) -> bool {
    edits.iter().any(|edit| match edit {
        Edit::Cut { .. } | Edit::Paste { .. } => true,
        Edit::Block { mode, register, .. } => {
            matches!(
                mode,
                super::types::BlockMode::Cut | super::types::BlockMode::PasteAfter
            ) || register.is_some()
        }
        _ => false,
    })
}

fn read_register(
    register: Option<&str>,
    span_target: bool,
    clipboard: &mut Clipboard,
    line_num: u64,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    if let Some(register) = register {
        if let Some(lines) = clipboard.named.get(register) {
            return Ok(lines.clone());
        }
        let mut known: Vec<(String, usize)> = clipboard
            .named
            .iter()
            .map(|(name, lines)| (name.clone(), lines.len()))
            .collect();
        known.sort();
        if span_target {
            return Err(format!(
                "line {line_num}: {}",
                empty_register_span_paste_message(register, &known)
            ));
        }
        warnings.push(format!(
            "line {line_num}: {}",
            empty_register_paste_warning(register, &known)
        ));
        return Ok(Vec::new());
    }
    if clipboard.pending_anon_cuts.len() > 1 {
        return Err(format!(
            "line {line_num}: {}",
            ambiguous_anonymous_paste_message(&clipboard.pending_anon_cuts)
        ));
    }
    let Some(lines) = clipboard.lines.clone() else {
        return Err(format!("line {line_num}: {EMPTY_PASTE}"));
    };
    clipboard.pending_anon_cuts.clear();
    Ok(lines)
}

fn write_register(
    range: &super::types::ParsedRange,
    register: Option<&str>,
    line_num: u64,
    file_lines: &[String],
    clipboard: &mut Clipboard,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let out_of_range = || {
        format!(
            "line {line_num}: `{}` is out of range (file has {} lines).",
            describe_cut(range, register),
            file_lines.len()
        )
    };
    let start = range.start.line.checked_sub(1).ok_or_else(out_of_range)?;
    let start = usize::try_from(start).map_err(|_| out_of_range())?;
    let end = usize::try_from(range.end.line).map_err(|_| out_of_range())?;
    let captured: Vec<String> = file_lines
        .get(start..end)
        .ok_or_else(out_of_range)?
        .to_vec();
    match register {
        Some(name) => {
            // A named register outlives the call, and pasting it was the only way to see it.
            let bytes = captured.iter().fold(0_usize, |sum, line| {
                sum.saturating_add(line.len()).saturating_add(1)
            });
            warnings.push(format!(
                "line {line_num}: `{}` captured lines {}-{} ({} lines, {bytes} bytes)",
                describe_cut(range, register),
                range.start.line,
                range.end.line,
                captured.len()
            ));
            clipboard.named.insert(name.to_owned(), captured);
        }
        None => {
            clipboard.lines = Some(captured);
            clipboard.pending_anon_cuts.push(describe_cut(range, None));
        }
    }
    Ok(())
}

pub fn resolve_clipboard_edits(
    edits: Vec<Edit>,
    file_lines: &[String],
    clipboard: &mut Clipboard,
    warnings: &mut Vec<String>,
) -> Result<Vec<Edit>, String> {
    if !has_clipboard_edit(&edits) {
        return Ok(edits);
    }
    let mut resolved: Vec<Edit> = Vec::new();
    for edit in edits {
        match edit {
            Edit::Cut {
                range,
                register,
                line_num,
            } => {
                write_register(
                    &range,
                    register.as_deref(),
                    line_num,
                    file_lines,
                    clipboard,
                    warnings,
                )?;
            }
            Edit::Paste {
                at,
                register,
                line_num,
            } => {
                let span_target = matches!(at, PasteTarget::Span { .. });
                let lines = read_register(
                    register.as_deref(),
                    span_target,
                    clipboard,
                    line_num,
                    warnings,
                )?;
                match at {
                    PasteTarget::Gap { cursor } => {
                        for text in lines {
                            resolved.push(Edit::Insert {
                                cursor,
                                text,
                                line_num,
                                replacement: false,
                            });
                        }
                    }
                    PasteTarget::Span { range } => {
                        if range.start.line < 1 || range.end.line > file_lines.len() as u64 {
                            let reg = register
                                .as_deref()
                                .map(|name| format!(" @{name}"))
                                .unwrap_or_default();
                            return Err(format!(
                                "line {line_num}: `PUT {}.={}{reg}` is out of range (file has {} lines).",
                                range.start.line,
                                range.end.line,
                                file_lines.len()
                            ));
                        }
                        let cursor = Cursor::BeforeAnchor {
                            anchor: range.start,
                        };
                        for text in lines {
                            resolved.push(Edit::Insert {
                                cursor,
                                text,
                                line_num,
                                replacement: true,
                            });
                        }
                        for line in range.start.line..=range.end.line {
                            resolved.push(Edit::Delete {
                                anchor: Anchor { line },
                                line_num,
                            });
                        }
                    }
                }
            }
            other => resolved.push(other),
        }
    }
    Ok(resolved)
}

/// Start a batch with persisted named registers but no anonymous state.
pub fn start_clipboard_batch(source: &Clipboard) -> Clipboard {
    Clipboard {
        lines: None,
        named: source.named.clone(),
        pending_anon_cuts: Vec::new(),
    }
}

pub fn fork_clipboard(source: &Clipboard) -> Clipboard {
    source.clone()
}

/// Only named registers persist across batches.
pub fn commit_clipboard(fork: Clipboard, target: &mut Clipboard) {
    for (name, lines) in fork.named {
        target.named.insert(name, lines);
    }
}
