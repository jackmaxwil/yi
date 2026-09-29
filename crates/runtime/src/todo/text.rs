use yi_types::plan::doc::{AgentId, BlockedOn, Todo, TodoState, TodoStateName};
use yi_types::todo::{PhaseName, TodoId, TodoList, TodoPhase};

use super::{DEFAULT_PHASE, TodoError};

pub const NEXT_LINES: usize = 3;

fn marker(state: &TodoState) -> &'static str {
    match state {
        TodoState::Pending => "[ ]",
        TodoState::Running { .. } => "[>]",
        TodoState::Blocked { .. } => "[!]",
        TodoState::Done { .. } => "[x]",
        TodoState::Abandoned | TodoState::Failed { .. } => "[-]",
        TodoState::Other(_) => "[?]",
    }
}

fn state_of(mark: char) -> Option<TodoState> {
    match mark {
        ' ' => Some(TodoState::Pending),
        '>' => Some(TodoState::Running {
            by: AgentId::owner(),
        }),
        'x' | 'X' => Some(TodoState::Done {
            output: None,
            resolution: None,
        }),
        '-' | '~' => Some(TodoState::Abandoned),
        '!' => Some(TodoState::Blocked {
            on: BlockedOn::User,
            note: String::new(),
        }),
        _ => None,
    }
}

fn row(raw: &str, line: usize) -> Result<(usize, Todo), TodoError> {
    let indent = raw.len().saturating_sub(raw.trim_start().len());
    let body = raw.trim_start();
    let bad = || TodoError::Checklist {
        line,
        text: raw.to_owned(),
    };
    let rest = body.strip_prefix("- [").ok_or_else(bad)?;
    let mut chars = rest.chars();
    let mark = chars.next().ok_or_else(bad)?;
    let rest = chars.as_str().strip_prefix("] ").ok_or_else(bad)?;
    let state = state_of(mark).ok_or_else(bad)?;
    let text = rest.trim();
    let (id, text) = match text
        .split_once(' ')
        .and_then(|(token, tail)| TodoId::parse(token).map(|id| (id, tail)))
    {
        Some((id, tail)) => (Some(id), tail),
        None => (None, text),
    };
    let mut item = Todo::from_text(text)?;
    item.id = id;
    item.state = state;
    Ok((indent, item))
}

pub fn parse(source: &str) -> Result<TodoList, TodoError> {
    let mut list = TodoList::default();
    for (index, raw) in source.lines().enumerate() {
        let line = index.saturating_add(1);
        if raw.trim().is_empty() {
            continue;
        }
        if let Some(name) = raw
            .trim()
            .strip_prefix("## ")
            .or_else(|| raw.trim().strip_prefix("# "))
        {
            list.phases.push(TodoPhase {
                name: PhaseName::new(name)?,
                items: Vec::new(),
                extra: serde_json::Map::new(),
            });
            continue;
        }
        let (indent, item) = row(raw, line)?;
        if list.phases.is_empty() {
            list.phases.push(TodoPhase {
                name: PhaseName::new(DEFAULT_PHASE)?,
                items: Vec::new(),
                extra: serde_json::Map::new(),
            });
        }
        let Some(phase) = list.phases.last_mut() else {
            continue;
        };
        if indent == 0 {
            phase.items.push(item);
        } else if indent <= 2 {
            let parent = phase.items.last_mut().ok_or_else(|| TodoError::Checklist {
                line,
                text: raw.to_owned(),
            })?;
            parent.children.push(item);
        } else {
            return Err(TodoError::TooDeep { line });
        }
    }
    if list.items().next().is_none() {
        return Err(TodoError::Empty);
    }
    if let Some(label) = list.duplicates().first() {
        return Err(TodoError::Duplicate {
            label: label.to_string(),
        });
    }
    Ok(list)
}

pub fn merge(old: &TodoList, mut new: TodoList) -> TodoList {
    new.next_id = old.next_id;
    for phase in &mut new.phases {
        for item in &mut phase.items {
            carry(old, item);
            for child in &mut item.children {
                carry(old, child);
            }
        }
    }
    new
}

/// The prior row is the one the row's id names among those with its label, else the first.
fn carry(old: &TodoList, row: &mut Todo) {
    let named = |prior: &&Todo| prior.label == row.label && row.id.is_some() && prior.id == row.id;
    let Some(prior) = old
        .items()
        .find(named)
        .or_else(|| old.items().find(|prior| prior.label == row.label))
    else {
        return;
    };
    if row.id.is_none() {
        row.id = prior.id.clone();
    }
    if row.cites.intent.is_empty() {
        row.cites.intent = prior.cites.intent.clone();
    }
    if TodoStateName::of(&prior.state) == TodoStateName::of(&row.state) {
        row.state = prior.state.clone();
        row.note = row.note.take().or_else(|| prior.note.clone());
        row.evidence = prior.evidence.clone();
        row.ask = prior.ask.clone();
    }
}

pub fn name(item: &Todo) -> String {
    match &item.id {
        Some(id) => id.to_string(),
        None => format!("{:?}", item.label.as_str()),
    }
}

fn row_text(item: &Todo) -> String {
    let id = item
        .id
        .as_ref()
        .map(|id| format!("{id} "))
        .unwrap_or_default();
    let cut = if item.is_cut() { "…" } else { "" };
    format!(
        "{} {id}{}{cut}{}{}",
        marker(&item.state),
        item.label,
        suffix(item),
        asked(item)
    )
}

pub fn asked(item: &Todo) -> String {
    match (&item.state, &item.ask) {
        (TodoState::Blocked { .. }, Some(ask)) => format!(" — {ask}"),
        _ => String::new(),
    }
}

pub fn suffix(item: &Todo) -> String {
    match (&item.state, &item.note) {
        (TodoState::Blocked { on, note }, _) if note.is_empty() => {
            format!(" (blocked on {})", on.as_str())
        }
        (TodoState::Blocked { on, note }, _) => {
            format!(" (blocked on {on}: {note})", on = on.as_str())
        }
        (TodoState::Abandoned, Some(note)) => format!(" (dropped: {})", note.as_str()),
        (TodoState::Failed { cause, .. }, _) => format!(" (failed: {cause})"),
        _ => String::new(),
    }
}

pub fn header(list: &TodoList) -> String {
    let progress = list.progress();
    let mut line = format!("Todos {}/{}", progress.done, progress.total);
    let planned = super::mirror::plan_of(list).and_then(|_| {
        list.items().find(|item| {
            matches!(item.state, TodoState::Running { .. })
                && item.extra.contains_key(super::mirror::PLAN_KEY)
        })
    });
    if let Some(running) = planned.or_else(|| list.running()) {
        let cut = if running.is_cut() { "…" } else { "" };
        line.push_str(&format!(" · running: {}{cut}", running.label));
    }
    if progress.blocked > 0 {
        line.push_str(&format!(" · {} blocked", progress.blocked));
    }
    line
}

pub fn checklist(list: &TodoList) -> Vec<String> {
    let mut out = Vec::new();
    for phase in &list.phases {
        if list.phases.len() > 1 || phase.name.as_str() != DEFAULT_PHASE {
            out.push(format!("## {}", phase.name));
        }
        for item in &phase.items {
            out.push(format!("- {}", row_text(item)));
            for child in &item.children {
                out.push(format!("  - {}", row_text(child)));
            }
        }
    }
    out
}

fn moves(item: &Todo) -> Option<String> {
    let state = format!(
        "{}({})",
        yi_types::graph::TODO_STATE,
        TodoStateName::of(&item.state)
    );
    let facts = crate::affordance::Facts {
        holds: &[state.as_str()],
        name: &name(item),
        cap: 1,
    };
    crate::affordance::render(crate::affordance::shipped(), super::tool::NAME, &facts).pop()
}

pub fn next_lines(list: &TodoList) -> Vec<String> {
    if super::mirror::plan_of(list).is_some() {
        return Vec::new();
    }
    let mut out: Vec<String> = list.running().and_then(moves).into_iter().collect();
    let pending = list
        .items()
        .filter(|item| matches!(item.state, TodoState::Pending))
        .take(
            crate::levers::get()
                .graph_next_lines
                .saturating_sub(out.len()),
        );
    out.extend(pending.filter_map(moves));
    if out.is_empty() {
        let blocked = list
            .items()
            .find(|item| matches!(item.state, TodoState::Blocked { .. }));
        out.extend(blocked.and_then(moves));
    }
    out
}

pub fn render(list: &TodoList) -> String {
    if list.items().next().is_none() {
        return "Todos: empty".to_owned();
    }
    let mut lines = vec![header(list)];
    lines.extend(checklist(list));
    lines.extend(next_lines(list));
    lines.join("\n")
}

/// Incident: the whole list on every op was 61.5% of row 0028's tool-result characters (D184).
pub fn render_change(before: &TodoList, after: &TodoList) -> String {
    if after.items().next().is_none() {
        return render(after);
    }
    let seen = checklist(before);
    let mut lines = vec![header(after)];
    lines.extend(
        checklist(after)
            .into_iter()
            .filter(|line| !seen.contains(line)),
    );
    lines.extend(next_lines(after));
    lines.join("\n")
}
