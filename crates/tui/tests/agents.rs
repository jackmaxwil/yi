use std::error::Error;

use ratatui::text::Line;
use yi_tui::agents::{AgentRow, AgentState, AgentsPopup};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::{KeyCodeValue, SingleKey};
use yi_tui::popup::{BottomView, PopupResult};

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn flat(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn key(code: KeyCodeValue) -> SingleKey {
    SingleKey {
        code,
        ctrl: false,
        alt: false,
        shift: false,
    }
}

fn row(id: &str, state: AgentState, tokens: u64, spawn: Option<&str>) -> AgentRow {
    AgentRow {
        id: id.to_owned(),
        name: id.to_owned(),
        state,
        tokens,
        toolcalls: 3,
        spawn: spawn.map(str::to_owned),
    }
}

/// Own-usage columns, so the total is the sum of what is on screen.
#[test]
fn the_popup_lists_each_agents_own_tokens_and_sums_them() -> TestResult {
    let popup = AgentsPopup::new(
        vec![
            row("scout", AgentState::Running, 84_000, None),
            row("builder", AgentState::Done, 230_000, None),
        ],
        200_000,
    );
    let rendered = flat(&popup.lines(100, &theme()));
    let joined = rendered.join("\n");
    assert!(joined.contains("84k"), "{joined}");
    assert!(joined.contains("230k"), "{joined}");
    assert!(joined.contains("total 314k over 2 agents"), "{joined}");
    // The context bar is the root's share of its window, in ten cells.
    assert!(joined.contains("▓▓▓▓░░░░░░ 42%"), "{joined}");
    Ok(())
}

/// The uniquely-Yi row: a child comes from a kernel cell, and knowing which
/// cell is the difference between a list of names and a story.
#[test]
fn children_of_one_kernel_cell_are_shown_under_it_once() -> TestResult {
    let popup = AgentsPopup::new(
        vec![
            row("a", AgentState::Running, 1, Some("rlm.run(task=one)")),
            row("b", AgentState::Running, 1, Some("rlm.run(task=one)")),
            row("c", AgentState::Running, 1, Some("rlm.run(task=two)")),
        ],
        200_000,
    );
    let rendered = flat(&popup.lines(100, &theme()));
    assert_eq!(
        rendered
            .iter()
            .filter(|row| row.contains("⊙ rlm.run(task=one)"))
            .count(),
        1,
        "the spawning cell is named once for the family it made: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|row| row.contains("⊙ rlm.run(task=two)")),
        "{rendered:?}"
    );
    Ok(())
}

/// Stopping an agent is not undoable, so it takes two presses — and the window
/// lapses on its own, so an armed row left alone disarms.
#[test]
fn stopping_an_agent_needs_a_second_press() -> TestResult {
    let mut popup = AgentsPopup::new(
        vec![
            row("scout", AgentState::Running, 1, None),
            row("builder", AgentState::Running, 1, None),
        ],
        200_000,
    );
    assert!(matches!(
        popup.handle_key(&key(KeyCodeValue::Char('x'))),
        PopupResult::Open
    ));
    assert!(popup.stop.is_none(), "one press stops nothing");
    let armed = flat(&popup.lines(100, &theme())).join("\n");
    assert!(armed.contains("scout x again to stop"), "{armed}");

    assert!(matches!(
        popup.handle_key(&key(KeyCodeValue::Char('x'))),
        PopupResult::Close
    ));
    assert_eq!(popup.stop.as_deref(), Some("scout"));

    // Moving the selection disarms: the confirm belongs to the row, not the key.
    let mut popup = AgentsPopup::new(vec![row("scout", AgentState::Running, 1, None)], 200_000);
    popup.handle_key(&key(KeyCodeValue::Char('x')));
    popup.handle_key(&key(KeyCodeValue::Down));
    let disarmed = flat(&popup.lines(100, &theme())).join("\n");
    assert!(!disarmed.contains("again to stop"), "{disarmed}");
    Ok(())
}
