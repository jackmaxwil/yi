use std::error::Error;
use std::time::{Duration, Instant};

use yi_tui::colors::{ColorTier, Theme};
use yi_tui::hud::{TODO_FULL_FOR, TodoClock, todo_rows};
use yi_types::plan::doc::{TodoLabel, TodoStateName};
use yi_types::todo::{BlockedOn, PhaseName, TodoItem, TodoList, TodoPhase};

type TestResult = Result<(), Box<dyn Error>>;

fn item(label: &str, state: TodoStateName) -> Result<TodoItem, Box<dyn Error>> {
    let mut item = TodoItem::pending(TodoLabel::new(label)?);
    item.state = state;
    Ok(item)
}

fn phase(name: &str, items: Vec<TodoItem>) -> Result<TodoPhase, Box<dyn Error>> {
    Ok(TodoPhase {
        name: PhaseName::new(name)?,
        items,
        extra: serde_json::Map::new(),
    })
}

fn list(items: Vec<TodoItem>) -> Result<TodoList, Box<dyn Error>> {
    Ok(TodoList {
        phases: vec![phase("Tasks", items)?],
        ..TodoList::default()
    })
}

/// Ten items, the first `done` finished and the next one running.
fn ten(done: usize) -> Result<TodoList, Box<dyn Error>> {
    let items = (0..10)
        .map(|index| {
            let state = match index.cmp(&done) {
                std::cmp::Ordering::Less => TodoStateName::Done,
                std::cmp::Ordering::Equal => TodoStateName::Running,
                std::cmp::Ordering::Greater => TodoStateName::Pending,
            };
            item(&format!("item {}", index + 1), state)
        })
        .collect::<Result<Vec<_>, _>>()?;
    list(items)
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

fn rows(list: &TodoList, full: bool) -> Result<(String, Vec<String>), Box<dyn Error>> {
    let (title, rows) = todo_rows(Some(list), full, &theme()).ok_or("open work must render")?;
    Ok((title, rows.iter().map(text).collect()))
}

/// The HUD is chrome, not the model's checklist: no ids to address, no `running:` echo
/// of the row already marked with ▶, just a numbered list.
#[test]
fn the_block_numbers_open_work_and_hides_a_finished_list() -> TestResult {
    let mut blocked = item("land it", TodoStateName::Blocked)?;
    blocked.on = Some(BlockedOn::User);
    blocked.note = Some("which branch".to_owned());
    let mut open = list(vec![
        item("read", TodoStateName::Done)?,
        item("write", TodoStateName::Running)?,
        blocked,
    ])?;
    open.for_each_mut(|row| row.id = yi_types::todo::TodoId::parse("t9"));
    let (title, rows) = rows(&open, true)?;
    assert_eq!(title, "Todos 1/3 · 1 blocked");
    assert_eq!(
        rows,
        vec![
            "  1. ✓ read",
            "  2. ▶ write",
            "  3. ! land it (blocked on user: which branch)",
        ]
    );
    let finished = list(vec![item("read", TodoStateName::Done)?])?;
    assert!(
        todo_rows(Some(&finished), true, &theme()).is_none(),
        "a finished list leaves the HUD alone"
    );
    assert!(todo_rows(None, true, &theme()).is_none());
    Ok(())
}

/// Phases head their items in the full view; children number on in flat order.
#[test]
fn the_full_view_shows_every_phase_and_child() -> TestResult {
    let mut parent = item("write the fix", TodoStateName::Pending)?;
    parent
        .children
        .push(item("parser", TodoStateName::Pending)?);
    let two = TodoList {
        phases: vec![
            phase(
                "Ground",
                vec![item("read the code", TodoStateName::Running)?],
            )?,
            phase("Fix", vec![parent])?,
        ],
        ..TodoList::default()
    };
    let (_, full) = rows(&two, true)?;
    assert_eq!(
        full,
        vec![
            "  Ground",
            "  1. ▶ read the code",
            "  Fix",
            "  2. ○ write the fix",
            "    3. ○ parser",
        ]
    );
    let (_, compact) = rows(&two, false)?;
    assert_eq!(
        compact,
        vec![
            "  1. ▶ read the code",
            "  2. ○ write the fix",
            "    3. ○ parser"
        ],
        "the compact view drops the phase headings"
    );
    Ok(())
}

/// Once seen, the list folds to the step behind, the step in flight, and two ahead.
#[test]
fn the_compact_view_is_last_current_and_next_two() -> TestResult {
    let (_, compact) = rows(&ten(5)?, false)?;
    assert_eq!(
        compact,
        vec![
            "  5. ✓ item 5",
            "  6. ▶ item 6",
            "  7. ○ item 7",
            "  8. ○ item 8",
        ]
    );
    let (_, full) = rows(&ten(5)?, true)?;
    assert_eq!(full.len(), 10, "the full view folds nothing: {full:?}");
    Ok(())
}

/// Nothing done means nothing behind: the current step and the three after it.
#[test]
fn a_fresh_list_folds_to_current_and_next_three() -> TestResult {
    let (_, compact) = rows(&ten(0)?, false)?;
    assert_eq!(
        compact,
        vec![
            "  1. ▶ item 1",
            "  2. ○ item 2",
            "  3. ○ item 3",
            "  4. ○ item 4",
        ]
    );
    Ok(())
}

/// The window never runs past the list: the last step shows with the one behind it.
#[test]
fn the_compact_view_stops_at_the_end_of_the_list() -> TestResult {
    let (_, compact) = rows(&ten(9)?, false)?;
    assert_eq!(compact, vec!["  9. ✓ item 9", "  10. ▶ item 10"]);
    Ok(())
}

/// With nothing running, the first open item is the current one.
#[test]
fn the_first_open_item_is_current_when_none_runs() -> TestResult {
    let open = list(vec![
        item("read", TodoStateName::Done)?,
        item("write", TodoStateName::Pending)?,
        item("ship", TodoStateName::Pending)?,
    ])?;
    let (_, compact) = rows(&open, false)?;
    assert_eq!(compact, vec!["  1. ✓ read", "  2. ○ write", "  3. ○ ship"]);
    Ok(())
}

/// The full view lasts ten seconds from when a list first shows; stepping an item keeps
/// the clock, a changed set of labels restarts it.
#[test]
fn the_clock_restarts_only_for_a_new_list() -> TestResult {
    let start = Instant::now();
    let mut clock = TodoClock::default();
    clock.observe(Some(&ten(0)?), start);
    assert!(clock.full(start));
    let late = start + TODO_FULL_FOR;
    assert!(!clock.full(late));
    assert_eq!(clock.wake(start), TODO_FULL_FOR);
    assert_eq!(clock.wake(late), Duration::MAX);

    clock.observe(Some(&ten(3)?), late);
    assert!(!clock.full(late), "stepping items is the same list");

    let mut grown = ten(3)?;
    let extra = item("item 11", TodoStateName::Pending)?;
    if let Some(tasks) = grown.phases.first_mut() {
        tasks.items.push(extra);
    }
    clock.observe(Some(&grown), late);
    assert!(clock.full(late), "an appended item is a new list");

    clock.observe(None, late);
    assert_eq!(clock.wake(late), Duration::MAX);
    clock.observe(Some(&grown), late + TODO_FULL_FOR);
    assert!(
        clock.full(late + TODO_FULL_FOR),
        "a list that left and came back shows in full again"
    );
    Ok(())
}
