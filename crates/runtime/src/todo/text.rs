use yi_types::plan::doc::{TodoLabel, TodoStateName};
use yi_types::todo::{BlockedOn, PhaseName, TodoId, TodoItem, TodoList, TodoPhase};

use super::{DEFAULT_PHASE, TodoError};

pub const NEXT_LINES: usize = 3;

fn marker(state: &TodoStateName) -> &'static str {
    match state {
        TodoStateName::Pending => "[ ]",
        TodoStateName::Running => "[>]",
        TodoStateName::Blocked => "[!]",
        TodoStateName::Done => "[x]",
        TodoStateName::Abandoned | TodoStateName::Failed => "[-]",
        TodoStateName::Other(_) => "[?]",
    }
}

fn state_of(mark: char) -> Option<TodoStateName> {
    match mark {
        ' ' => Some(TodoStateName::Pending),
        '>' => Some(TodoStateName::Running),
        'x' | 'X' => Some(TodoStateName::Done),
        '-' | '~' => Some(TodoStateName::Abandoned),
        '!' => Some(TodoStateName::Blocked),
        _ => None,
    }
}

fn row(raw: &str, line: usize) -> Result<(usize, TodoItem), TodoError> {
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
    let mut item = TodoItem::from_text(text)?;
    item.id = id;
    item.state = state;
    if item.state == TodoStateName::Blocked {
        item.on = Some(BlockedOn::User);
    }
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
    let mut seen: Vec<&TodoLabel> = Vec::new();
    for item in list.items() {
        if seen.contains(&&item.label) {
            return Err(TodoError::Duplicate {
                label: item.label.to_string(),
            });
        }
        seen.push(&item.label);
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

fn carry(old: &TodoList, row: &mut TodoItem) {
    let Some(prior) = old.items().find(|prior| prior.label == row.label) else {
        return;
    };
    if row.id.is_none() {
        row.id = prior.id.clone();
    }
    if prior.state == row.state {
        row.on = prior.on.clone();
        row.note = row.note.take().or_else(|| prior.note.clone());
        row.evidence = prior.evidence.clone();
    }
}

pub fn name(item: &TodoItem) -> String {
    match &item.id {
        Some(id) => id.to_string(),
        None => format!("{:?}", item.label.as_str()),
    }
}

fn row_text(item: &TodoItem) -> String {
    let id = item
        .id
        .as_ref()
        .map(|id| format!("{id} "))
        .unwrap_or_default();
    let cut = if item.is_cut() { "…" } else { "" };
    format!(
        "{} {id}{}{cut}{}",
        marker(&item.state),
        item.label,
        suffix(item)
    )
}

fn suffix(item: &TodoItem) -> String {
    match (&item.state, &item.on, &item.note) {
        (TodoStateName::Blocked, Some(on), Some(note)) => {
            format!(" (blocked on {on}: {note})", on = on.as_str())
        }
        (TodoStateName::Blocked, Some(on), None) => format!(" (blocked on {})", on.as_str()),
        (TodoStateName::Abandoned, _, Some(note)) => format!(" (dropped: {note})"),
        _ => String::new(),
    }
}

pub fn header(list: &TodoList) -> String {
    let progress = list.progress();
    let mut line = format!("Todos {}/{}", progress.done, progress.total);
    if let Some(running) = list.running() {
        line.push_str(&format!(" · running: {}", running.label));
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

fn moves(item: &TodoItem) -> String {
    let name = name(item);
    match item.state {
        TodoStateName::Running => {
            format!(
                "done {name} evidence=`<command>` <output line> · block {name} on user · drop {name} <reason>"
            )
        }
        TodoStateName::Pending => format!("start {name} · drop {name} <reason>"),
        TodoStateName::Blocked => format!("unblock {name} · drop {name} <reason>"),
        _ => String::new(),
    }
}

pub fn next_lines(list: &TodoList) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(running) = list.running() {
        out.push(format!("next: {}", moves(running)));
    }
    for item in list
        .items()
        .filter(|item| item.state == TodoStateName::Pending)
        .take(NEXT_LINES.saturating_sub(out.len()))
    {
        out.push(format!("next: {}", moves(item)));
    }
    if out.is_empty()
        && let Some(blocked) = list
            .items()
            .find(|item| item.state == TodoStateName::Blocked)
    {
        out.push(format!("next: {}", moves(blocked)));
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
