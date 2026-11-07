use std::path::Path;
use std::time::{Duration, Instant};

use yi_types::plan::doc::Todo;
use yi_types::todo::{CheckLast, RowCheck, TodoList};

use super::names::{known_labels, known_phases, locate};

/// A check's last run: whether it passed, the wire word it leaves on the row, and what a red
/// printed.
pub struct Ran {
    pub green: bool,
    pub last: CheckLast,
    pub view: String,
}

/// Invariant: a check runs in a cleared environment like a goal's, so a green here means the
/// command works without the turn's own variables leaking into it.
pub fn run(
    command: &str,
    cwd: &Path,
    cancel: &yi_tools::CancelFlag,
    checker: &crate::goal::Checker,
) -> Result<Ran, String> {
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(crate::goal::DEFAULT_CHECK_TIMEOUT_MS))
        .unwrap_or_else(Instant::now);
    let capture =
        crate::goal::run_check_in(cwd, command, &[], None, deadline, Some(cancel), checker)?;
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
            if item.state.is_terminal() || stored.last.is_green() {
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
            .filter(|_| !item.state.is_terminal())
            .map(|check| super::TodoError::CheckNotPassed {
                label: item.label.to_string(),
                command: check.command,
            })
    };
    (targeted(list, target)?.into_iter().find_map(red)).map_or(Ok(()), Err)
}

/// The rows `target` names on `list`: a label's row, a phase's rows with their children, or all.
pub(super) fn targeted<'a>(
    list: &'a TodoList,
    target: &super::Target,
) -> Result<Vec<&'a Todo>, super::TodoError> {
    Ok(match target {
        super::Target::Label(label) => vec![
            list.items()
                .nth(locate(list, label.as_str())?)
                .ok_or_else(|| super::TodoError::NoSuchLabel {
                    label: label.to_string(),
                    known: known_labels(list),
                })?,
        ],
        super::Target::Phase(name) => list
            .phases
            .iter()
            .find(|phase| phase.name == *name)
            .ok_or_else(|| super::TodoError::NoSuchPhase {
                phase: name.to_string(),
                known: known_phases(list),
            })?
            .items
            .iter()
            .flat_map(|item| std::iter::once(item).chain(item.children.iter()))
            .collect(),
        super::Target::All => list.items().collect(),
    })
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
