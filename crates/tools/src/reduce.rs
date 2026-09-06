use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reduced {
    pub text: String,
    pub raw_bytes: usize,
    pub out_bytes: usize,
    pub recovery: Option<PathBuf>,
}

const HEAD_LINES: usize = 80;
const TAIL_LINES: usize = 40;
const GREP_LINES: usize = 60;

// Output this small is already cheap, and a reducer that rewrites it only
// costs the model a marker and a path to nowhere useful.
const REDUCE_FLOOR: usize = 2_048;

/// The user asked for the whole thing; reducing answers a different question.
const RAW_FLAGS: [&str; 6] = ["-v", "--verbose", "--nocapture", "--porcelain", "-la", "-C"];

/// Every path runs through [`never_worse`] and a lossy result is tee'd, so the full text is
/// one `read` away. `max_lines` moves the line caps, never the upstream byte ceiling.
pub fn reduce(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    recovery_dir: Option<&Path>,
    max_lines: Option<usize>,
) -> Reduced {
    let raw = join_streams(stdout, stderr);
    let raw_bytes = raw.len();
    if raw_bytes <= REDUCE_FLOOR || skip_reduction(command) {
        return Reduced {
            out_bytes: raw_bytes,
            text: raw,
            raw_bytes,
            recovery: None,
        };
    }
    let stripped = strip_ansi(&raw);
    let budget = max_lines.unwrap_or(HEAD_LINES + TAIL_LINES);
    let filtered = match program(command) {
        Some("cargo") => cargo(&stripped, exit_code, budget),
        Some("git") => generic(&stripped, budget),
        Some("grep" | "rg" | "ag") => cap_lines(&stripped, max_lines.unwrap_or(GREP_LINES)),
        _ => generic(&stripped, budget),
    };
    let text = never_worse(&raw, filtered);
    let out_bytes = text.len();
    let recovery = if out_bytes < raw_bytes {
        recovery_dir.and_then(|dir| tee(dir, &raw))
    } else {
        None
    };
    // T19: lossy with nowhere to recover from is worse than unreduced.
    if out_bytes < raw_bytes && recovery.is_none() {
        return Reduced {
            out_bytes: raw_bytes,
            text: raw,
            raw_bytes,
            recovery: None,
        };
    }
    let text = match &recovery {
        Some(path) => format!("{text}\n[full output: {}]", path.display()),
        None => text,
    };
    Reduced {
        out_bytes: text.len(),
        text,
        raw_bytes,
        recovery,
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
fn cargo(text: &str, exit_code: i32, budget: usize) -> String {
    if exit_code != 0 {
        return cap_lines(text, budget);
    }
    let kept: Vec<&str> = text
        .lines()
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
        return generic(text, budget);
    }
    kept.join("\n")
}

fn generic(text: &str, budget: usize) -> String {
    let deduped = collapse_repeats(text);
    cap_lines(&deduped, budget)
}

fn collapse_repeats(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut previous: Option<&str> = None;
    let mut repeats = 0_usize;
    for line in text.lines() {
        if Some(line) == previous {
            repeats = repeats.saturating_add(1);
            continue;
        }
        if repeats > 0 {
            out.push(format!("[previous line repeated {repeats} more times]"));
            repeats = 0;
        }
        out.push(line.to_owned());
        previous = Some(line);
    }
    if repeats > 0 {
        out.push(format!("[previous line repeated {repeats} more times]"));
    }
    out.join("\n")
}

fn cap_lines(text: &str, cap: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= cap {
        return text.to_owned();
    }
    // Incident: the split was the fixed HEAD_LINES/TAIL_LINES, so a smaller
    // cap emitted more than it named (GREP_LINES 60 emitted 120).
    let head = (cap.saturating_mul(2) / 3).max(1).min(lines.len());
    let tail = cap
        .saturating_sub(head)
        .min(lines.len().saturating_sub(head));
    let dropped = lines.len().saturating_sub(head).saturating_sub(tail);
    let mut out: Vec<&str> = lines.get(..head).unwrap_or_default().to_vec();
    let marker = format!(
        "[{dropped} lines omitted: {}-{}]",
        head.saturating_add(1),
        head.saturating_add(dropped)
    );
    out.push(&marker);
    out.extend(
        lines
            .get(lines.len().saturating_sub(tail)..)
            .unwrap_or_default(),
    );
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

/// Tee'd so the model can open the full text with `read`.
fn tee(dir: &Path, raw: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let id = xxhash_rust::xxh32::xxh32(raw.as_bytes(), 0);
    let path = dir.join(format!("{id:08x}.txt"));
    std::fs::write(&path, raw).ok()?;
    Some(path)
}
