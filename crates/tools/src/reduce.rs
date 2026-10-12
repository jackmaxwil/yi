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
/// Lines kept around each error or failure the head and tail would drop: a Python traceback
/// names its file three lines above the error.
const SIGNAL_CONTEXT: usize = 3;
const SIGNAL_ROWS: usize = 60;
const GREP_PER_FILE: usize = 3;

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

impl Numbering {
    fn suffix(self) -> &'static str {
        match self {
            Self::File => "",
            Self::CutCapture => " of the cut capture, not the file",
        }
    }
}

/// One shown line and the 1-based line of the source it came from.
struct Row {
    text: String,
    line: usize,
    /// The first source line a counted row stands for; `line` is its last.
    first: usize,
}

impl Row {
    fn new(text: String, line: usize) -> Self {
        Self {
            text,
            line,
            first: line,
        }
    }

    fn copy(&self) -> Self {
        Self {
            text: self.text.clone(),
            ..*self
        }
    }
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
        Some("grep" | "rg" | "ag") => {
            let rows = per_file(&rows, numbering);
            cap_lines(&rows, max_lines.unwrap_or(GREP_LINES), numbering, false)
        }
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
        return cap_lines(&collapse_passes(rows), budget, numbering, true);
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
    cap_lines(
        &collapse_passes(&collapse_repeats(rows)),
        budget,
        numbering,
        true,
    )
}

/// A line naming an error or a failure: kept from the middle a head and tail would drop.
fn is_signal(line: &str) -> bool {
    const SIGNALS: [&str; 7] = [
        "error",
        "fail",
        "panicked",
        "traceback",
        "exception",
        "fatal",
        "assert",
    ];
    // Incident: a HOME under `.yi-homes/keep-failed` made every line naming a path in it a signal.
    // A word with two slashes is a path, wherever it starts, and only its last component speaks.
    (line.to_ascii_lowercase().split_whitespace())
        .map(|word| match word.matches('/').count() {
            0 | 1 => word,
            _ => word.rsplit('/').next().unwrap_or(word),
        })
        .any(|word| SIGNALS.iter().any(|signal| word.contains(signal)))
}

/// A test runner's line for one passing test: cargo, pytest, jest. A `PASSED` that shares its
/// line with a failure is not one; a test named for errors that ends `... ok` is.
pub fn is_pass(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with("test ") && line.ends_with(" ... ok"))
        || ((line.contains(" PASSED") || line.starts_with("PASSED ")) && !is_signal(line))
        || line.starts_with('✓')
        || line.starts_with('✔')
}

/// Runs of passing-test lines as one counted row; a failure among them stays a line of its own.
fn collapse_passes(rows: &[Row]) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    let mut run: Vec<&Row> = Vec::new();
    let flush = |run: &mut Vec<&Row>, out: &mut Vec<Row>| {
        match run.as_slice() {
            [] => {}
            [only] => out.push(only.copy()),
            [first, .., last] => out.push(Row {
                text: format!(
                    "[{} passing tests omitted: {}-{}]",
                    run.len(),
                    first.first,
                    last.line
                ),
                line: last.line,
                first: first.first,
            }),
        }
        run.clear();
    };
    for row in rows {
        if is_pass(&row.text) {
            run.push(row);
        } else {
            flush(&mut run, &mut out);
            out.push(row.copy());
        }
    }
    flush(&mut run, &mut out);
    out
}

/// grep's `path:line:text` matches grouped by file, at most [`GREP_PER_FILE`] each, so one busy
/// file cannot push every other out of the head; output naming no file is left as it is.
fn per_file(rows: &[Row], numbering: Numbering) -> Vec<Row> {
    let file = |text: &str| {
        let (path, rest) = text.split_once(':')?;
        let (number, _) = rest.split_once(':')?;
        let numbered = !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit());
        // A log's `2026-10-10T11:30:` reads as `path:line:` too; a path has a `/` or a `.`.
        let pathlike = path.contains(['/', '.']) && !path.contains(char::is_whitespace);
        (numbered && pathlike).then(|| path.to_owned())
    };
    let mut groups: Vec<(Option<String>, &[Row])> = Vec::new();
    let mut index = 0;
    while let Some(row) = rows.get(index) {
        let path = file(&row.text);
        let count = (rows.get(index..).unwrap_or_default().iter())
            .take_while(|next| path.is_some() && file(&next.text) == path)
            .count()
            .max(1);
        groups.push((
            path,
            rows.get(index..index.saturating_add(count))
                .unwrap_or_default(),
        ));
        index = index.saturating_add(count);
    }
    if groups.iter().filter(|(path, _)| path.is_some()).count() < 2 {
        return rows.iter().map(Row::copy).collect();
    }
    let of = numbering.suffix();
    let mut out: Vec<Row> = Vec::new();
    for (path, group) in groups {
        out.extend(group.iter().take(GREP_PER_FILE).map(Row::copy));
        if let (Some(path), Some(first), Some(last)) =
            (&path, group.get(GREP_PER_FILE), group.last())
        {
            let more = group.len().saturating_sub(GREP_PER_FILE);
            let text = format!(
                "[+{more} more in {path} (GREP_PER_FILE {GREP_PER_FILE}): output lines {}-{}{of}]",
                first.first, last.line
            );
            out.push(Row {
                text,
                line: last.line,
                first: first.first,
            });
        }
    }
    out
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
    let lines: Vec<&str> = text.lines().map(frame).collect();
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
        let line = frame(raw);
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
        // Invariant: a pytest -v line ends `[ 54%]`, yet the next line does not overwrite it.
        if is_progress(line) && !is_signal(line) && !is_pass(line) {
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
        .map(|(_, (line, count, number))| {
            Row::new(
                if count > 0 {
                    format!("{line} [×{}]", count.saturating_add(1))
                } else {
                    line
                },
                number,
            )
        })
        .collect()
}

/// A source line as a row shows it: the last frame of `\r` progress, trailing blanks dropped;
/// strip ANSI from the text first, as the reducer does.
pub fn frame(raw: &str) -> &str {
    raw.rsplit('\r').next().unwrap_or(raw).trim_end()
}

/// A line of the raw output as a row shows it: its CRLF end dropped as `str::lines` drops it,
/// ANSI stripped, then [`frame`].
pub fn shown_form(raw: &str) -> String {
    frame(&strip_ansi(raw.strip_suffix('\r').unwrap_or(raw))).to_owned()
}

/// The span of a row the reducer wrote for lines it left out; any other line, a program's own
/// `[retry: 1-4]` included, gives none.
pub fn omitted_span(row: &str) -> Option<(usize, usize)> {
    let label = row.strip_prefix('[')?.rsplit_once(": ")?.0;
    let counted = label.starts_with(|c: char| c.is_ascii_digit() || c == '+');
    let ours = counted && (label.contains(" omitted") || label.contains(" more in "));
    ours.then(|| row_span(row)).flatten()
}

/// A shown row as its source line reads, without the `[×N]` count of a repeated line.
pub fn source_form(row: &str) -> &str {
    let count = row
        .strip_suffix(']')
        .and_then(|rest| rest.rsplit_once(" [×"));
    match count {
        Some((line, digits))
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
        {
            line
        }
        _ => row,
    }
}

/// The `a-b` a reducer row ends with, in the file's numbering: what it omitted, counted or
/// pointed at; a row numbered against the cut capture names no file lines, so it gives none.
pub fn row_span(row: &str) -> Option<(usize, usize)> {
    let inner = row.strip_prefix('[')?.strip_suffix(']')?;
    let (_, range) = inner.rsplit_once(": ")?;
    let range = range.strip_prefix("output lines ").unwrap_or(range);
    let (from, to) = range.split_once('-')?;
    Some((from.parse().ok()?, to.parse().ok()?))
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
    let mut repeats: Option<(usize, usize, usize)> = None;
    let repeated = |(count, first, line): (usize, usize, usize)| Row {
        text: format!("[previous line repeated {count} more times]"),
        line,
        first,
    };
    for row in rows {
        if Some(row.text.as_str()) == previous {
            repeats = Some(match repeats {
                Some((count, first, _)) => (count.saturating_add(1), first, row.line),
                None => (1, row.first, row.line),
            });
            continue;
        }
        out.extend(repeats.take().map(repeated));
        out.push(row.copy());
        previous = Some(&row.text);
    }
    out.extend(repeats.map(repeated));
    out
}

fn cap_lines(rows: &[Row], cap: usize, numbering: Numbering, signals_kept: bool) -> String {
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
    // Incident: a syntax error at line 814 of 1,506 sat in the middle the head and tail dropped.
    let mut kept: Vec<usize> = Vec::new();
    for (at, line) in lines.iter().enumerate().take(first_tail).skip(head) {
        if !signals_kept || !is_signal(line) {
            continue;
        }
        let from = at.saturating_sub(SIGNAL_CONTEXT).max(head);
        let to = at
            .saturating_add(SIGNAL_CONTEXT)
            .min(first_tail.saturating_sub(1));
        let start = kept
            .last()
            .map_or(from, |last| from.max(last.saturating_add(1)));
        kept.extend(start..=to);
    }
    let shown = kept.len().min(SIGNAL_ROWS);
    let over = kept.split_off(shown);
    let of = numbering.suffix();
    let gap = |start: usize, end: usize| {
        let from = (start.checked_sub(1).and_then(|last| rows.get(last)))
            .map_or(1, |row| row.line.saturating_add(1));
        let to = match rows.get(end).filter(|_| end < lines.len()) {
            Some(row) => row.first.saturating_sub(1),
            None => rows.last().map_or(from, |row| row.line),
        };
        let dropped = to.saturating_add(1).saturating_sub(from);
        format!("[{dropped} lines omitted: {from}-{to}{of}]")
    };
    let mut out: Vec<String> = (lines.get(..head).unwrap_or_default().iter())
        .map(|line| (*line).to_owned())
        .collect();
    let mut next = head;
    for index in kept {
        if index > next {
            out.push(gap(next, index));
        }
        out.extend(lines.get(index).map(|line| (*line).to_owned()));
        next = index.saturating_add(1);
    }
    if !over.is_empty() {
        let lines_naming = (over.iter())
            .filter(|index| lines.get(**index).is_some_and(|line| is_signal(line)))
            .count();
        out.push(format!("[{shown} lines around errors and failures kept of {} (SIGNAL_ROWS {SIGNAL_ROWS}); {lines_naming} more such lines follow in the full output]", shown.saturating_add(over.len())));
    }
    if first_tail > next {
        out.push(gap(next, first_tail));
    }
    out.extend((lines.get(first_tail..).unwrap_or_default().iter()).map(|line| (*line).to_owned()));
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
