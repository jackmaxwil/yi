use std::error::Error;

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
        extra: serde_json::Map::new(),
    })
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
    let (title, rows) = todo_rows(Some(&open)).ok_or("open work must render")?;
    assert_eq!(title, "Todos 1/3 · running: write · 1 blocked");
    let rows: Vec<String> = rows.iter().map(text).collect();
    assert_eq!(
        rows,
        vec![
            "  - [x] read",
            "  - [>] write",
            "  - [!] land it (blocked on user: which branch)",
        ]
    );
    let finished = list(vec![item("read", TodoStateName::Done)?])?;
    assert!(
        todo_rows(Some(&finished)).is_none(),
        "a finished list leaves the HUD alone"
    );
    assert!(todo_rows(None).is_none());
    Ok(())
}

#[test]
fn a_long_list_folds_past_the_row_cap() -> TestResult {
    let items = (0..TODO_ROWS + 3)
        .map(|index| item(&format!("item {index}"), TodoStateName::Pending))
        .collect::<Result<Vec<_>, _>>()?;
    let (_, rows) = todo_rows(Some(&list(items)?)).ok_or("open work")?;
    assert_eq!(rows.len(), TODO_ROWS + 1);
    assert_eq!(text(&rows[TODO_ROWS]), "  +3 more");
    Ok(())
}
