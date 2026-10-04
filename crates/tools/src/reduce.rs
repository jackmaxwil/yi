use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reduced {
    pub text: String,
    pub raw_bytes: usize,
    pub out_bytes: usize,
    /// The `[full output: …]` pointer the text ends with.
    pub recovery: Option<String>,
}

const HEAD_LINES: usize = 80;
const TAIL_LINES: usize = 40;
const GREP_LINES: usize = 60;
const DIAGNOSTIC_LOOKAHEAD: usize = 3;
const WARNINGS_SHOWN: usize = 3;
const ERROR_BYTES: usize = 16 * 1024;
const SPILL_READ_LIMIT: u64 = 4 * 1024 * 1024;

// Output this small is already cheap, and a reducer that rewrites it only
// costs the model a marker and a path to nowhere useful.
pub const REDUCE_FLOOR: usize = 8_192;

/// The user asked for the whole thing; reducing answers a different question.
const RAW_FLAGS: [&str; 6] = ["-v", "--verbose", "--nocapture", "--porcelain", "-la", "-C"];

/// Every path runs through [`never_worse`]; a lossy result points at `kept` (a cut capture's
/// spill) or at a tee under `tee_dir`. `max_lines` moves the line caps, never the byte ceiling.
pub fn reduce(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    kept: Option<&str>,
    tee_dir: Option<&Path>,
    max_lines: Option<usize>,
) -> Reduced {
    let raw = join_streams(stdout, stderr);
    if raw.len() <= REDUCE_FLOOR || skip_reduction(command) {
        return whole(raw);
    }
    let budget = max_lines.unwrap_or(HEAD_LINES + TAIL_LINES);
    let select = |source: &str, numbering| {
        never_worse(
            source,
            select(command, source, exit_code, budget, max_lines, numbering),
        )
    };
    let spilled = kept.and_then(read_spill);
    let from_spill = spilled
        .as_deref()
        .map(|spill| (select(spill, Numbering::File), spill.len()))
        .filter(|(text, _)| text.len() < raw.len());
    let numbering = match kept {
        Some(_) => Numbering::CutCapture,
        None => Numbering::File,
    };
    let (text, raw_bytes) = from_spill.unwrap_or_else(|| (select(&raw, numbering), raw.len()));
    let out_bytes = text.len();
    let recovery = if out_bytes < raw_bytes {
        (kept.map(str::to_owned)).or_else(|| tee_dir.and_then(|dir| tee(dir, &raw)))
    } else {
        None
    };
    // Output that is lossy with nowhere to recover from is worse than unreduced.
    if out_bytes < raw_bytes && recovery.is_none() {
        return whole(raw);
    }
    let text = match &recovery {
        Some(note) => format!("{text}\n{note}"),
        None => text,
    };
    Reduced {
        out_bytes: text.len(),
        text,
        raw_bytes,
        recovery,
    }
}

fn whole(raw: String) -> Reduced {
    Reduced {
        out_bytes: raw.len(),
        raw_bytes: raw.len(),
        text: raw,
        recovery: None,
    }
}

/// The spill a cut capture names, read whole when small enough that its line numbers can be named.
fn read_spill(kept: &str) -> Option<String> {
    let (_, path) = crate::process::spill_path(kept)?;
    let small = std::fs::metadata(path).ok()?.len() <= SPILL_READ_LIMIT;
    let bytes = small.then(|| std::fs::read(path).ok()).flatten()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Which text an omitted-lines notice counts: the file the pointer names, or the cut capture.
#[derive(Clone, Copy)]
enum Numbering {
    File,
    CutCapture,
}

/// One shown line and the 1-based line of the source it came from.
struct Row {
    text: String,
    line: usize,
}

fn select(
    command: &str,
    source: &str,
    exit_code: i32,
    budget: usize,
    max_lines: Option<usize>,
    numbering: Numbering,
) -> String {
    let stripped = strip_ansi(source);
    if rustc_diagnostics(&stripped) {
        return diagnostics(&stripped, budget, numbering);
    }
    let rows = compress(&stripped, 0);
    match program(command) {
        Some("cargo") => cargo(&rows, exit_code, budget, numbering),
        Some("grep" | "rg" | "ag") => cap_lines(&rows, max_lines.unwrap_or(GREP_LINES), numbering),
        _ => generic(&rows, budget, numbering),
    }
}

/// A filter that made the output longer is not a filter.
fn never_worse(raw: &str, filtered: String) -> String {
    if filtered.len() < raw.len() {
        filtered
    } else {
        raw.to_owned()
    }
}

fn join_streams(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_owned(),
        (true, false) => stderr.to_owned(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn skip_reduction(command: &str) -> bool {
    command
        .split_whitespace()
        .any(|word| RAW_FLAGS.contains(&word))
}

fn program(command: &str) -> Option<&str> {
    command
        .split_whitespace()
        .find(|word| !word.contains('='))
        .map(|word| word.rsplit('/').next().unwrap_or(word))
}

/// A green cargo run is a progress log with one line that matters; a red one
/// is diagnostics, and dropping those to save bytes is how a reducer lies.
fn cargo(rows: &[Row], exit_code: i32, budget: usize, numbering: Numbering) -> String {
    if exit_code != 0 {
        return cap_lines(rows, budget, numbering);
    }
    let kept: Vec<&str> = rows
        .iter()
        .map(|row| row.text.as_str())
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("error")
                || trimmed.starts_with("warning")
                || trimmed.starts_with("test result:")
                || trimmed.starts_with("Finished")
                || trimmed.starts_with("running ")
        })
        .collect();
    if kept.is_empty() {
        return generic(rows, budget, numbering);
    }
    kept.join("\n")
}

fn generic(rows: &[Row], budget: usize, numbering: Numbering) -> String {
    cap_lines(&collapse_repeats(rows), budget, numbering)
}

fn is_header(line: &str) -> bool {
    ["error[", "error:", "warning[", "warning:"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn is_summary(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("error: could not compile")
        || (line.starts_with("warning:")
            && line.contains(" generated ")
            && line.contains(" warning"))
        || line.starts_with("For more information")
        || line.starts_with("test result:")
        || line.starts_with("Finished")
}

fn located(rows: &[&str]) -> bool {
    (rows.iter().skip(1).take(DIAGNOSTIC_LOOKAHEAD)).any(|row| row.trim_start().starts_with("--> "))
}

/// Recognised by shape, never by the command or the exit status: a masked `$?` or a `cd &&`
/// prefix must not route a build away from this.
fn rustc_diagnostics(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    (lines.iter().enumerate())
        .any(|(at, line)| is_header(line) && located(lines.get(at..).unwrap_or_default()))
}

struct Block<'a> {
    error: bool,
    rows: Vec<&'a str>,
    end: usize,
}

/// Every error block whole, a few warnings, the summaries; what follows the last located block
/// is program output and takes the generic reduction, numbered by its place in `text`.
fn diagnostics(text: &str, budget: usize, numbering: Numbering) -> String {
    let lines: Vec<&str> = (text.lines())
        .map(|line| line.rsplit('\r').next().unwrap_or(line).trim_end())
        .collect();
    let mut blocks: Vec<Block<'_>> = Vec::new();
    let mut open: Option<Block<'_>> = None;
    for (at, &line) in lines.iter().enumerate() {
        if is_header(line) && !is_summary(line) {
            blocks.extend(open.take());
            let error = line.starts_with("error");
            let rows = vec![line];
            let end = at.saturating_add(1);
            open = Some(Block { error, rows, end });
        } else if line.is_empty() || is_summary(line) {
            blocks.extend(open.take());
        } else if let Some(block) = &mut open {
            block.rows.push(line);
            block.end = at.saturating_add(1);
        }
    }
    blocks.extend(open);
    let tail_start = (blocks.iter())
        .filter(|block| located(&block.rows))
        .map(|block| block.end)
        .max()
        .unwrap_or_default();
    blocks.retain(|block| block.end <= tail_start);
    let before = lines.get(..tail_start).unwrap_or_default();
    let summary: Vec<&str> = before
        .iter()
        .copied()
        .filter(|line| is_summary(line))
        .collect();
    let block_rows = blocks.iter().map(|block| block.rows.len()).sum::<usize>();
    let other = (before.iter())
        .filter(|line| !line.is_empty() && !is_summary(line))
        .count()
        .saturating_sub(block_rows);
    let (errors, warnings): (Vec<Block<'_>>, Vec<Block<'_>>) =
        blocks.into_iter().partition(|block| block.error);
    let mut spent = 0_usize;
    let shown_errors: Vec<String> = (errors.iter().map(|block| block.rows.join("\n")))
        .enumerate()
        .take_while(|(index, block)| {
            spent = spent.saturating_add(block.len());
            *index == 0 || spent <= ERROR_BYTES
        })
        .map(|(_, block)| block)
        .collect();
    let error_cap = if shown_errors.len() < errors.len() {
        format!(" (ERROR_BYTES {ERROR_BYTES})")
    } else {
        String::new()
    };
    let shown_warnings = warnings.len().min(WARNINGS_SHOWN);
    let mut out = vec![format!(
        "[rustc diagnostics: {} of {} error blocks{error_cap}, {shown_warnings} of {} warning blocks (WARNINGS_SHOWN {WARNINGS_SHOWN}), {} summary lines, 0 of {other} other lines; every line is in the full output below]",
        shown_errors.len(),
        errors.len(),
        warnings.len(),
        summary.len(),
    )];
    out.extend(shown_errors);
    out.extend((warnings.iter().take(WARNINGS_SHOWN)).map(|block| block.rows.join("\n")));
    out.extend((!summary.is_empty()).then(|| summary.join("\n")));
    let tail = lines.get(tail_start..).unwrap_or_default().join("\n");
    let tail = generic(&compress(&tail, tail_start), budget, numbering);
    out.extend((!tail.trim().is_empty()).then_some(tail));
    out.join("\n\n")
}

/// Lossy before omission: the last frame of `\r` progress, collapsed whitespace, a repeated
/// line counted on its first occurrence (ponytail: global, per-block if a rollout cares).
fn compress(text: &str, first_line: usize) -> Vec<Row> {
    let mut kept: Vec<(String, usize, usize)> = Vec::new();
    let mut first_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut last_progress: Option<usize> = None;
    let mut blank_run = false;
    for (at, raw) in text.lines().enumerate() {
        let number = first_line.saturating_add(at).saturating_add(1);
        let line = raw.rsplit('\r').next().unwrap_or(raw).trim_end();
        if line.is_empty() {
            if !blank_run {
                kept.push((String::new(), 0, number));
            }
            blank_run = true;
            continue;
        }
        blank_run = false;
        if line.len() >= 8
            && let Some(&index) = first_seen.get(line)
        {
            if let Some((_, count, _)) = kept.get_mut(index) {
                *count = count.saturating_add(1);
            }
            continue;
        }
        if is_progress(line) {
            if let Some((stale, _, _)) = last_progress.and_then(|index| kept.get_mut(index)) {
                stale.clear();
            }
            last_progress = Some(kept.len());
        }
        first_seen.insert(line.to_owned(), kept.len());
        kept.push((line.to_owned(), 0, number));
    }
    kept.into_iter()
        .enumerate()
        .filter(|(index, (line, _, _))| !line.is_empty() || last_progress != Some(*index))
        .map(|(_, (line, count, number))| Row {
            text: if count > 0 {
                format!("{line} [×{}]", count.saturating_add(1))
            } else {
                line
            },
            line: number,
        })
        .collect()
}

/// A percentage (`42%`) or a bar (`[===>`, `━━`) marks a line its successor overwrites.
fn is_progress(line: &str) -> bool {
    if line.contains("[==") || line.contains("[##") || line.contains("━━") {
        return true;
    }
    let bytes = line.as_bytes();
    bytes.iter().enumerate().any(|(at, &byte)| {
        byte == b'%' && {
            let digits = bytes
                .get(..at)
                .unwrap_or_default()
                .iter()
                .rev()
                .take_while(|b| b.is_ascii_digit())
                .count();
            (1..=3).contains(&digits)
                && at
                    .checked_sub(digits.saturating_add(1))
                    .and_then(|before| bytes.get(before))
                    .is_none_or(|b| !b.is_ascii_alphanumeric())
        }
    })
}

fn collapse_repeats(rows: &[Row]) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    let mut previous: Option<&str> = None;
    let mut repeats: Option<(usize, usize)> = None;
    let repeated = |(count, line): (usize, usize)| Row {
        text: format!("[previous line repeated {count} more times]"),
        line,
    };
    for row in rows {
        if Some(row.text.as_str()) == previous {
            let count = repeats.map_or(1, |(count, _)| count.saturating_add(1));
            repeats = Some((count, row.line));
            continue;
        }
        out.extend(repeats.take().map(repeated));
        out.push(Row {
            text: row.text.clone(),
            line: row.line,
        });
        previous = Some(&row.text);
    }
    out.extend(repeats.map(repeated));
    out
}

fn cap_lines(rows: &[Row], cap: usize, numbering: Numbering) -> String {
    let lines: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
    // Incident: a fixed HEAD/TAIL split let GREP_LINES 60 emit 120 lines; the byte
    // budget widens both ends instead (a 700-line dump at 40 chars shows 200).
    let head_budget = REDUCE_FLOOR.saturating_mul(2) / 3;
    let fits = |lines: &mut dyn Iterator<Item = &&str>, budget: usize| {
        let mut spent = 0_usize;
        lines
            .take_while(|line| {
                spent = spent.saturating_add(line.len()).saturating_add(1);
                spent <= budget
            })
            .count()
    };
    let head = (cap.saturating_mul(2) / 3)
        .max(fits(&mut lines.iter(), head_budget))
        .max(1)
        .min(lines.len());
    let tail = cap
        .saturating_sub(cap.saturating_mul(2) / 3)
        .max(fits(
            &mut lines.iter().rev(),
            REDUCE_FLOOR.saturating_sub(head_budget),
        ))
        .min(lines.len().saturating_sub(head));
    if lines.len() <= head.saturating_add(tail) {
        return lines.join("\n");
    }
    let first_tail = lines.len().saturating_sub(tail);
    let from = (head.checked_sub(1).and_then(|last| rows.get(last)))
        .map_or(1, |row| row.line.saturating_add(1));
    let to = match rows.get(first_tail).filter(|_| tail > 0) {
        Some(row) => row.line.saturating_sub(1),
        None => rows.last().map_or(from, |row| row.line),
    };
    let dropped = to.saturating_add(1).saturating_sub(from);
    let of = match numbering {
        Numbering::File => "",
        Numbering::CutCapture => " of the cut capture, not the file",
    };
    let mut out: Vec<&str> = lines.get(..head).unwrap_or_default().to_vec();
    let marker = format!("[{dropped} lines omitted: {from}-{to}{of}]");
    out.push(&marker);
    out.extend(lines.get(first_tail..).unwrap_or_default());
    out.join("\n")
}

pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            out.push(character);
            continue;
        }
        if characters.peek() == Some(&'[') {
            characters.next();
            for escaped in characters.by_ref() {
                if escaped.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            characters.next();
        }
    }
    out
}

/// The whole joined text, for a reduction of a capture that was never cut.
fn tee(dir: &Path, raw: &str) -> Option<String> {
    let mut spill = crate::spill::Spill::new(Some(dir));
    spill.write(raw.as_bytes());
    spill.keep()
}
