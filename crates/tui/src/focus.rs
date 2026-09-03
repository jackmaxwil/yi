use crate::app::App;
use crate::cell::Cell;

pub(crate) enum FocusMove {
    Child,
    Parent,
    Next,
    Prev,
}

pub(crate) fn focus_move(app: &mut App, direction: FocusMove) {
    let order = app.task_order.clone();
    if order.is_empty() {
        return;
    }
    let current = app
        .focused
        .as_ref()
        .and_then(|id| order.iter().position(|x| x == id));
    let target = match (direction, current) {
        (FocusMove::Parent, _) => None,
        (FocusMove::Child, None) => order.first().cloned(),
        (FocusMove::Child, Some(_)) => return,
        (FocusMove::Next, Some(i)) => order.get(i + 1).cloned().or_else(|| order.first().cloned()),
        (FocusMove::Prev, Some(i)) => {
            if i == 0 {
                order.last().cloned()
            } else {
                order.get(i - 1).cloned()
            }
        }
        (FocusMove::Next | FocusMove::Prev, None) => order.first().cloned(),
    };
    set_focus(app, target);
}

pub(crate) fn set_focus(app: &mut App, target: Option<String>) {
    if app.focused == target {
        return;
    }
    if let Some(previous) = app.focused.take() {
        app.commit_cell(&Cell::Rule {
            text: "back to parent".to_owned(),
            accent_name: previous,
        });
    }
    if let Some(child_id) = target {
        let name = app
            .tasks
            .get(&child_id)
            .map(|s| s.cell.description.clone())
            .unwrap_or_default();
        let position = app
            .task_order
            .iter()
            .position(|id| id == &child_id)
            .map(|i| i + 1)
            .unwrap_or(1);
        app.commit_cell(&Cell::Rule {
            text: format!("subagent {name} ({position} of {})", app.task_order.len()),
            accent_name: child_id.clone(),
        });
        match app
            .tasks
            .get(&child_id)
            .and_then(|state| state.session.clone())
        {
            Some(session) => {
                let entries = crate::port::branch_of(&session);
                app.replay_entries(&entries);
            }
            None => app.pending_focus = Some(child_id.clone()),
        }
        app.focused = Some(child_id);
    }
    app.scheduler.request();
}
