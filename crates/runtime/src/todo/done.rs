use yi_types::plan::doc::{TodoLabel, TodoState};
use yi_types::todo::TodoList;

use super::{Target, TodoError, each_target, illegal, named, resolve};

/// A `done` closes the rows it names, and nothing else: a bare `done` closes pending rows only, so
/// a row waiting on the user keeps its ask and the result note names every row it kept.
pub(super) fn done(
    list: &mut TodoList,
    target: &Target,
    evidence: Option<String>,
    notes: &mut Vec<String>,
) -> Result<(), TodoError> {
    let evidence = evidence
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    if let Target::Label(label) = target {
        let open = open_children(list, label)?;
        if !open.is_empty() {
            return Err(TodoError::ParentOpen {
                label: label.to_string(),
                open: open.join(", "),
            });
        }
    }
    let by_name = matches!(target, Target::Label(_));
    let mut kept: Vec<String> = Vec::new();
    each_target(list, target, |item| {
        let closed = item.state.is_terminal();
        if matches!(item.state, TodoState::Other(_)) || (by_name && closed) {
            return Err(illegal("done", item));
        }
        if closed {
            return Ok(());
        }
        if !by_name && matches!(item.state, TodoState::Blocked { .. }) {
            kept.push(named(item));
            return Ok(());
        }
        if evidence.is_none() {
            return Err(TodoError::NoEvidence {
                label: item.label.to_string(),
            });
        }
        item.state = TodoState::Done {
            output: None,
            resolution: None,
        };
        item.note = None;
        item.evidence.clone_from(&evidence);
        Ok(())
    })?;
    if !kept.is_empty() {
        notes.push(format!(
            "{} kept blocked: a done with no target closes pending rows only, so close each by name or `unblock` it",
            kept.join(", ")
        ));
    }
    Ok(())
}

fn open_children(list: &mut TodoList, label: &TodoLabel) -> Result<Vec<String>, TodoError> {
    let item = resolve(list, label)?;
    Ok(item
        .children
        .iter()
        .filter(|child| !child.state.is_terminal())
        .map(|child| format!("{:?}", child.label.as_str()))
        .collect())
}
