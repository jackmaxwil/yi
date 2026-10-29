use std::path::Path;
use std::time::{Duration, Instant};

use yi_types::plan::doc::Todo;
use yi_types::todo::{CheckLast, RowCheck, TodoList};

use super::names::{known_labels, known_phases, locate};

/// A check is a cold build's worth of work at most, and never longer than the turn it holds.
pub const CHECK_TIMEOUT_MS: u64 = 600_000;

/// The words a shell would look up as programs: the first word of every segment, with a leading
/// `VAR=value` assignment and the shell's own punctuation skipped.
const BUILTINS: [&str; 22] = [
    ":", ".", "[", "alias", "bg", "break", "cd", "command", "continue", "echo", "eval", "exec",
    "exit", "export", "fg", "kill", "printf", "pwd", "read", "return", "set", "shift",
];

const KEYWORDS: [&str; 15] = [
    "case", "do", "done", "elif", "else", "esac", "fi", "for", "if", "in", "then", "time", "until",
    "while", "!",
];

/// The first program a command names that this host does not have, or None: the probe runs
/// before the command, so a check that could never pass is refused with nothing written.
pub fn missing_program(command: &str) -> Option<String> {
    let mut segments: Vec<Vec<String>> = vec![Vec::new()];
    let mut word = String::new();
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                for inner in chars.by_ref() {
                    if inner == c {
                        break;
                    }
                    word.push(inner);
                }
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            '&' | '|' | ';' | '\n' | '(' | ')' | '{' | '}' => {
                push_word(&mut segments, &mut word);
                segments.push(Vec::new());
                if c == '&' && chars.peek() == Some(&'&') {
                    chars.next();
                }
            }
            c if c.is_whitespace() => push_word(&mut segments, &mut word),
            c => word.push(c),
        }
    }
    push_word(&mut segments, &mut word);
    segments.into_iter().find_map(|words| {
        let first = words.into_iter().find(|word| {
            !word.contains('=')
                && !word.starts_with('-')
                && !word.starts_with('#')
                && !word.contains('$')
                && !KEYWORDS.contains(&word.as_str())
        })?;
        (!BUILTINS.contains(&first.as_str()) && !on_path(&first)).then_some(first)
    })
}

fn push_word(segments: &mut [Vec<String>], word: &mut String) {
    if let (Some(last), false) = (segments.last_mut(), word.is_empty()) {
        last.push(std::mem::take(word));
    }
}

/// A bare program name is looked up in `PATH`; a path is judged by the file it names.
fn on_path(word: &str) -> bool {
    if word.contains('/') {
        return executable(Path::new(word));
    }
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| executable(&dir.join(word))))
        .unwrap_or(false)
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}

/// A check's last run: whether it passed, the wire word it leaves on the row, and what a red
/// printed.
pub struct Ran {
    pub green: bool,
    pub last: CheckLast,
    pub view: String,
}

pub fn refused(command: &str) -> Option<String> {
    missing_program(command).map(|program| {
        format!(
            "this check names {program:?}, which is not on PATH; the check is refused before it runs, so nothing it would have written exists"
        )
    })
}

/// Invariant: a check runs in a cleared environment like a goal's, so a green here means the
/// command works without the turn's own variables leaking into it.
pub fn run(command: &str, cwd: &Path, cancel: &yi_tools::CancelFlag) -> Result<Ran, String> {
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(CHECK_TIMEOUT_MS))
        .unwrap_or_else(Instant::now);
    let capture = crate::goal::run_check_in(cwd, command, &[], None, deadline, Some(cancel), None)?;
    let view = crate::goal::check_view(command, cwd, None, &capture);
    let green = capture.exit_code == Some(0) && !capture.cancelled;
    let last = if green {
        CheckLast::green()
    } else {
        let digest = yi_types::plan::canonical::Digest::of(view.as_bytes()).hex();
        CheckLast::red(digest.get(..8).unwrap_or(&digest))
    };
    Ok(Ran { green, last, view })
}

/// The command a row carries, if it carries one.
pub fn command_of(item: &Todo) -> Option<String> {
    RowCheck::of(item).map(|check| check.command)
}

pub(super) fn survive(prior: &TodoList, merged: &TodoList) -> Result<(), super::TodoError> {
    for item in prior.items() {
        let Some(stored) = RowCheck::of(item) else {
            continue;
        };
        let label = item.label.to_string();
        let Some(row) = merged.items().find(|row| row.label == item.label) else {
            if stored.last.is_green() {
                continue;
            }
            return Err(super::TodoError::CheckNotPassed {
                label,
                command: stored.command,
            });
        };
        match RowCheck::of(row) {
            Some(check) if check == stored => {}
            _ => {
                return Err(super::TodoError::CheckRewritten {
                    label,
                    stored: stored.command,
                });
            }
        }
    }
    Ok(())
}

/// `rm` takes a row away without a reason, so it may not take one whose check has not passed.
pub(super) fn keeps_reds(list: &TodoList, target: &super::Target) -> Result<(), super::TodoError> {
    let red = |item: &Todo| {
        RowCheck::of(item)
            .filter(|check| !check.last.is_green())
            .map(|check| super::TodoError::CheckNotPassed {
                label: item.label.to_string(),
                command: check.command,
            })
    };
    match target {
        super::Target::Label(label) => red(list
            .items()
            .nth(locate(list, label.as_str())?)
            .ok_or_else(|| super::TodoError::NoSuchLabel {
                label: label.to_string(),
                known: known_labels(list),
            })?)
        .map_or(Ok(()), Err),
        super::Target::Phase(name) => {
            let phase = list
                .phases
                .iter()
                .find(|phase| phase.name == *name)
                .ok_or_else(|| super::TodoError::NoSuchPhase {
                    phase: name.to_string(),
                    known: known_phases(list),
                })?;
            phase
                .items
                .iter()
                .flat_map(|item| std::iter::once(item).chain(item.children.iter()))
                .find_map(red)
                .map_or(Ok(()), Err)
        }
        super::Target::All => (list.items().find_map(red)).map_or(Ok(()), Err),
    }
}

/// Invariant: a row's check is written once, at `start`, and no later call may replace it: the
/// red it earns is the record `done` and the stop ladder judge the row by.
pub(super) fn assign(item: &mut Todo, given: Option<&str>) -> Result<(), super::TodoError> {
    match (given, RowCheck::of(item)) {
        (Some(given), Some(stored)) if stored.command != given => {
            Err(super::TodoError::CheckRewritten {
                label: item.label.to_string(),
                stored: stored.command,
            })
        }
        (Some(given), _) => {
            RowCheck {
                command: given.to_owned(),
                last: CheckLast::default(),
            }
            .put(item);
            Ok(())
        }
        (None, _) => Ok(()),
    }
}

pub(super) fn mark(item: &mut Todo, last: CheckLast) {
    if let Some(mut stored) = RowCheck::of(item) {
        stored.last = last;
        stored.put(item);
    }
}
