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
pub const REDUCE_FLOOR: usize = 8_192;

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
    let stripped = compress(&strip_ansi(&raw));
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
    // Output that is lossy with nowhere to recover from is worse than unreduced.
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

/// Lossy before omission: the last frame of `\r` progress, collapsed whitespace, a repeated
/// line counted on its first occurrence (ponytail: global, per-block if a rollout cares).
pub fn compress(text: &str) -> String {
    let mut kept: Vec<(String, usize)> = Vec::new();
    let mut first_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut last_progress: Option<usize> = None;
    let mut blank_run = false;
    for raw in text.lines() {
        let line = raw.rsplit('\r').next().unwrap_or(raw).trim_end();
        if line.is_empty() {
            if !blank_run {
                kept.push((String::new(), 0));
            }
            blank_run = true;
            continue;
        }
        blank_run = false;
        if line.len() >= 8
            && let Some(&index) = first_seen.get(line)
        {
            if let Some((_, count)) = kept.get_mut(index) {
                *count = count.saturating_add(1);
            }
            continue;
        }
        if is_progress(line) {
            if let Some((stale, _)) = last_progress.and_then(|index| kept.get_mut(index)) {
                stale.clear();
            }
            last_progress = Some(kept.len());
        }
        first_seen.insert(line.to_owned(), kept.len());
        kept.push((line.to_owned(), 0));
    }
    let lines: Vec<String> = kept
        .into_iter()
        .enumerate()
        .filter(|(index, (line, _))| !line.is_empty() || last_progress != Some(*index))
        .map(|(_, (line, count))| {
            if count > 0 {
                format!("{line} [×{}]", count.saturating_add(1))
            } else {
                line
            }
        })
        .collect();
    lines.join("\n")
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
        return text.to_owned();
    }
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
