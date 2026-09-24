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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnEmptyPaste {
    Throw,
    Drop,
}

fn read_register(
    register: Option<&str>,
    span_target: bool,
    clipboard: &mut Clipboard,
    line_num: u64,
    on_empty: OnEmptyPaste,
    warnings: &mut Vec<String>,
) -> Result<Option<Vec<String>>, String> {
    if let Some(register) = register {
        if let Some(lines) = clipboard.named.get(register) {
            return Ok(Some(lines.clone()));
        }
        if on_empty == OnEmptyPaste::Drop {
            return Ok(None);
        }
        let known: Vec<String> = clipboard.named.keys().cloned().collect();
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
        return Ok(Some(Vec::new()));
    }
    if clipboard.pending_anon_cuts.len() > 1 {
        if on_empty == OnEmptyPaste::Drop {
            return Ok(None);
        }
        return Err(format!(
            "line {line_num}: {}",
            ambiguous_anonymous_paste_message(&clipboard.pending_anon_cuts)
        ));
    }
    let Some(lines) = clipboard.lines.clone() else {
        if on_empty == OnEmptyPaste::Drop {
            return Ok(None);
        }
        return Err(format!("line {line_num}: {EMPTY_PASTE}"));
    };
    clipboard.pending_anon_cuts.clear();
    Ok(Some(lines))
}

fn write_register(
    range: &super::types::ParsedRange,
    register: Option<&str>,
    line_num: u64,
    file_lines: &[String],
    clipboard: &mut Clipboard,
) -> Result<(), String> {
    if range.start.line < 1 || range.end.line > file_lines.len() as u64 {
        return Err(format!(
            "line {line_num}: `{}` is out of range (file has {} lines).",
            describe_cut(range, register),
            file_lines.len()
        ));
    }
    let captured: Vec<String> =
        file_lines[(range.start.line - 1) as usize..range.end.line as usize].to_vec();
    match register {
        Some(register) => {
            clipboard.named.insert(register.to_owned(), captured);
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
    on_empty: OnEmptyPaste,
    warnings: &mut Vec<String>,
) -> Result<Vec<Edit>, String> {
    if !has_clipboard_edit(&edits) {
        return Ok(edits);
    }
    let mut resolved: Vec<Edit> = Vec::new();
    let mut synth_index = 0usize;
    for edit in edits {
        match edit {
            Edit::Cut {
                range,
                register,
                line_num,
                ..
            } => {
                write_register(&range, register.as_deref(), line_num, file_lines, clipboard)?;
            }
            Edit::Paste {
                at,
                register,
                line_num,
                block_start,
                ..
            } => {
                let span_target = matches!(at, PasteTarget::Span { .. });
                let Some(lines) = read_register(
                    register.as_deref(),
                    span_target,
                    clipboard,
                    line_num,
                    on_empty,
                    warnings,
                )?
                else {
                    continue;
                };
                match at {
                    PasteTarget::Gap { cursor } => {
                        for text in lines {
                            resolved.push(Edit::Insert {
                                cursor,
                                text,
                                line_num,
                                index: synth_index,
                                replacement: false,
                                block_start,
                            });
                            synth_index += 1;
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
                                index: synth_index,
                                replacement: true,
                                block_start: None,
                            });
                            synth_index += 1;
                        }
                        for line in range.start.line..=range.end.line {
                            resolved.push(Edit::Delete {
                                anchor: Anchor { line },
                                line_num,
                                index: synth_index,
                                old_assertion: None,
                            });
                            synth_index += 1;
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
