use super::clipboard::{OnEmptyPaste, resolve_clipboard_edits};
use super::messages::{UNRESOLVED_BLOCK_INTERNAL, UNRESOLVED_CLIPBOARD_INTERNAL};
use super::types::{Anchor, ApplyResult, Clipboard, Cursor, Edit};

fn edit_anchors(edit: &Edit) -> Vec<Anchor> {
    match edit {
        Edit::Delete { anchor, .. }
        | Edit::Insert {
            cursor: Cursor::BeforeAnchor { anchor } | Cursor::AfterAnchor { anchor },
            ..
        } => vec![*anchor],
        _ => Vec::new(),
    }
}

/// `split('\n')` yields a trailing "" sentinel: addressable for append-past-end inserts but
/// not content, so deletes there are dropped and EOF ranges stop at the last real line.
fn trailing_phantom_line(file_lines: &[String]) -> u64 {
    if file_lines.len() > 1 && file_lines.last().is_some_and(String::is_empty) {
        file_lines.len() as u64
    } else {
        0
    }
}

fn validate_line_bounds(edits: &[Edit], file_lines: &[String]) -> Result<(), String> {
    for edit in edits {
        for anchor in edit_anchors(edit) {
            if anchor.line < 1 || anchor.line > file_lines.len() as u64 {
                return Err(format!(
                    "Line {} does not exist (file has {} lines)",
                    anchor.line,
                    file_lines.len()
                ));
            }
        }
    }
    Ok(())
}

fn insert_at_start(file_lines: &mut Vec<String>, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    if file_lines.len() == 1 && file_lines[0].is_empty() {
        *file_lines = lines;
        return;
    }
    file_lines.splice(0..0, lines);
}

fn insert_at_end(file_lines: &mut Vec<String>, lines: Vec<String>) -> Option<u64> {
    if lines.is_empty() {
        return None;
    }
    if file_lines.len() == 1 && file_lines[0].is_empty() {
        *file_lines = lines;
        return Some(1);
    }
    let has_trailing_newline = file_lines.last().is_some_and(String::is_empty);
    let insert_index = if has_trailing_newline {
        file_lines.len() - 1
    } else {
        file_lines.len()
    };
    file_lines.splice(insert_index..insert_index, lines);
    Some(insert_index as u64 + 1)
}

struct Bucketed {
    line: u64,
    before: Vec<String>,
    replacement: Vec<String>,
    after: Vec<String>,
    delete: bool,
}

fn materialize_edits(
    original_lines: &[String],
    edits: &[Edit],
) -> Result<(String, Option<u64>), String> {
    let mut file_lines: Vec<String> = original_lines.to_vec();
    let mut first_changed: Option<u64> = None;
    let track = |line: u64, first_changed: &mut Option<u64>| {
        if first_changed.is_none_or(|existing| line < existing) {
            *first_changed = Some(line);
        }
    };

    let mut bof_lines: Vec<String> = Vec::new();
    let mut eof_lines: Vec<String> = Vec::new();
    let mut by_line: Vec<Bucketed> = Vec::new();
    let bucket_for = |line: u64, by_line: &mut Vec<Bucketed>| -> usize {
        if let Some(position) = by_line.iter().position(|bucket| bucket.line == line) {
            position
        } else {
            by_line.push(Bucketed {
                line,
                before: Vec::new(),
                replacement: Vec::new(),
                after: Vec::new(),
                delete: false,
            });
            by_line.len() - 1
        }
    };

    for edit in edits {
        match edit {
            Edit::Insert {
                cursor: Cursor::Bof,
                text,
                ..
            } => bof_lines.push(text.clone()),
            Edit::Insert {
                cursor: Cursor::Eof,
                text,
                ..
            } => eof_lines.push(text.clone()),
            Edit::Insert {
                cursor: Cursor::BeforeAnchor { anchor },
                text,
                replacement,
                ..
            } => {
                let position = bucket_for(anchor.line, &mut by_line);
                if *replacement {
                    by_line[position].replacement.push(text.clone());
                } else {
                    by_line[position].before.push(text.clone());
                }
            }
            Edit::Insert {
                cursor: Cursor::AfterAnchor { anchor },
                text,
                ..
            } => {
                let position = bucket_for(anchor.line, &mut by_line);
                by_line[position].after.push(text.clone());
            }
            Edit::Delete { anchor, .. } => {
                let position = bucket_for(anchor.line, &mut by_line);
                by_line[position].delete = true;
            }
            Edit::Block { .. } => return Err(UNRESOLVED_BLOCK_INTERNAL.to_owned()),
            Edit::Cut { .. } | Edit::Paste { .. } => {
                return Err(UNRESOLVED_CLIPBOARD_INTERNAL.to_owned());
            }
        }
    }

    // Apply per-line buckets bottom-up so earlier indices stay valid.
    by_line.sort_by(|left, right| right.line.cmp(&left.line));
    for bucket in by_line {
        if bucket.before.is_empty()
            && bucket.replacement.is_empty()
            && bucket.after.is_empty()
            && !bucket.delete
        {
            continue;
        }
        let index = (bucket.line - 1) as usize;
        let current = file_lines.get(index).cloned().unwrap_or_default();
        let mut replacement: Vec<String> = Vec::new();
        replacement.extend(bucket.before);
        replacement.extend(bucket.replacement);
        if !bucket.delete {
            replacement.push(current);
        }
        replacement.extend(bucket.after);
        if index < file_lines.len() {
            file_lines.splice(index..=index, replacement);
        } else {
            file_lines.extend(replacement);
        }
        track(bucket.line, &mut first_changed);
    }

    if !bof_lines.is_empty() {
        insert_at_start(&mut file_lines, bof_lines);
        track(1, &mut first_changed);
    }
    if let Some(changed) = insert_at_end(&mut file_lines, eof_lines) {
        track(changed, &mut first_changed);
    }

    Ok((file_lines.join("\n"), first_changed))
}

pub fn apply_edits(
    text: &str,
    edits: Vec<Edit>,
    clipboard: &mut Clipboard,
    on_empty_paste: OnEmptyPaste,
) -> Result<ApplyResult, String> {
    if edits.is_empty() {
        return Ok(ApplyResult {
            text: text.to_owned(),
            first_changed_line: None,
            warnings: Vec::new(),
        });
    }
    let file_lines: Vec<String> = text.split('\n').map(str::to_owned).collect();

    // Clipboard pre-pass: capture cut ranges from the original lines and
    // expand paste edits into plain inserts in authored order.
    let mut warnings: Vec<String> = Vec::new();
    let concrete =
        resolve_clipboard_edits(edits, &file_lines, clipboard, on_empty_paste, &mut warnings)?;

    let phantom = trailing_phantom_line(&file_lines);
    let target_edits: Vec<Edit> = concrete
        .into_iter()
        .filter(|edit| {
            !matches!(edit, Edit::Delete { anchor, .. } if phantom != 0 && anchor.line == phantom)
        })
        .collect();
    validate_line_bounds(&target_edits, &file_lines)?;
    let (result_text, first_changed_line) = materialize_edits(&file_lines, &target_edits)?;
    Ok(ApplyResult {
        text: result_text,
        first_changed_line,
        warnings,
    })
}
