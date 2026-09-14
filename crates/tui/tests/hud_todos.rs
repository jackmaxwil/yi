use std::error::Error;

use yi_tui::colors::{ColorTier, Theme};
use yi_tui::hud::{TODO_ROWS, todo_rows};
use yi_types::plan::doc::{TodoLabel, TodoStateName};
use yi_types::todo::{BlockedOn, PhaseName, TodoItem, TodoList, TodoPhase};

type TestResult = Result<(), Box<dyn Error>>;

fn item(label: &str, state: TodoStateName) -> Result<TodoItem, Box<dyn Error>> {
    let mut item = TodoItem::pending(TodoLabel::new(label)?);
    item.state = state;
    Ok(item)
}

fn list(items: Vec<TodoItem>) -> Result<TodoList, Box<dyn Error>> {
    Ok(TodoList {
        phases: vec![TodoPhase {
            name: PhaseName::new("Tasks")?,
            items,
            extra: serde_json::Map::new(),
        }],
        ..TodoList::default()
    })
}

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.to_string())
        .collect()
}

#[test]
fn the_block_shows_open_work_and_hides_a_finished_list() -> TestResult {
    let mut blocked = item("land it", TodoStateName::Blocked)?;
    blocked.on = Some(BlockedOn::User);
    blocked.note = Some("which branch".to_owned());
    let open = list(vec![
        item("read", TodoStateName::Done)?,
        item("write", TodoStateName::Running)?,
        blocked,
    ])?;
    let theme = theme();
    let (title, rows) = todo_rows(Some(&open), &theme).ok_or("open work must render")?;
    assert_eq!(title, "Todos 1/3 · running: write · 1 blocked");
    let rows: Vec<String> = rows.iter().map(text).collect();
    assert_eq!(
        rows,
        vec![
            "  ✓ read",
            "  ▶ write",
            "  ! land it (blocked on user: which branch)",
        ]
    );
    let finished = list(vec![item("read", TodoStateName::Done)?])?;
    assert!(
        todo_rows(Some(&finished), &theme).is_none(),
        "a finished list leaves the HUD alone"
    );
    assert!(todo_rows(None, &theme).is_none());
    Ok(())
}

#[test]
fn a_long_list_folds_past_the_row_cap() -> TestResult {
    let items = (0..TODO_ROWS + 3)
        .map(|index| item(&format!("item {index}"), TodoStateName::Pending))
        .collect::<Result<Vec<_>, _>>()?;
    let (_, rows) = todo_rows(Some(&list(items)?), &theme()).ok_or("open work")?;
    assert_eq!(rows.len(), TODO_ROWS + 1);
    assert_eq!(text(&rows[TODO_ROWS]), "  +3 more");
    Ok(())
}

/// Eleven items with the running one at the tail: the window slides so the step in
/// flight is on screen, and says how many rows sit above it.
#[test]
fn the_running_item_stays_on_screen_in_a_long_list() -> TestResult {
    let mut items = (0..TODO_ROWS + 2)
        .map(|index| item(&format!("item {index}"), TodoStateName::Done))
        .collect::<Result<Vec<_>, _>>()?;
    items.push(item("item running", TodoStateName::Running)?);
    items.push(item("item last", TodoStateName::Pending)?);
    let (_, rows) = todo_rows(Some(&list(items)?), &theme()).ok_or("open work")?;
    let rows: Vec<String> = rows.iter().map(text).collect();
    assert_eq!(
        rows.first().map(String::as_str),
        Some("  +4 above"),
        "{rows:?}"
    );
    assert!(rows.iter().any(|row| row == "  ▶ item running"), "{rows:?}");
    assert_eq!(
        rows.last().map(String::as_str),
        Some("  ○ item last"),
        "{rows:?}"
    );
    Ok(())
}
