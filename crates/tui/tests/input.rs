//! The message box while a turn runs: the steering row, and what Ctrl+C does to a draft.
use crate::common;

use std::error::Error;

use common::{VT100Backend, test_model};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use yi_tui::Command;
use yi_tui::app::{App, TuiOptions};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, UserContent};

type TestResult = Result<(), Box<dyn Error>>;

fn app() -> App {
    let options = TuiOptions {
        model: test_model("faux-1"),
        session_name: "input".to_owned(),
        cwd: "/tmp".to_owned(),
        lane: None,
        context_window: 128_000,
        session_dir: String::new(),
        keys: Vec::new(),
        initial_prompt: None,
        pace: 0,
    };
    App::new(
        options,
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        80,
    )
}

fn press(app: &mut App, tx: &UnboundedSender<Command>, code: KeyCode, modifiers: KeyModifiers) {
    let event = Event::Key(KeyEvent::new(code, modifiers));
    yi_tui::input::handle_terminal_event(app, tx, event);
}

fn type_text(app: &mut App, tx: &UnboundedSender<Command>, text: &str) {
    for c in text.chars() {
        press(app, tx, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

fn ctrl_c(app: &mut App, tx: &UnboundedSender<Command>) {
    press(app, tx, KeyCode::Char('c'), KeyModifiers::CONTROL);
}

fn sent(rx: &mut UnboundedReceiver<Command>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(command) = rx.try_recv() {
        out.push(match command {
            Command::Steer(text) => format!("steer {text}"),
            Command::Abort => "abort".to_owned(),
            _ => "other".to_owned(),
        });
    }
    out
}

fn injected(text: &str) -> AgentEvent {
    AgentEvent::MessageStart {
        message: AgentMessage::user_input(UserContent::Text(text.to_owned()), 0),
    }
}

fn live_rows(app: &mut App) -> Result<Vec<String>, Box<dyn Error>> {
    let mut terminal = yi_tui::terminal::Terminal::new(VT100Backend::new(80, 24), 4)?;
    yi_tui::render::draw(app, &mut terminal, None);
    let backend = terminal.backend();
    Ok((0..24).map(|row| backend.row_text(row)).collect())
}

fn has_row(rows: &[String], needle: &str) -> bool {
    rows.iter().any(|row| row.contains(needle))
}

/// Dies with the `Steering` row outliving its message: the loop injected the steer after the
/// tool batch and the transcript shows it, while the HUD kept calling it queued until AgentEnd.
#[test]
fn a_steer_leaves_the_queued_row_when_the_loop_injects_it() -> TestResult {
    let mut app = app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.reduce_agent(AgentEvent::AgentStart);
    for line in ["fast forward main", "then run the tests"] {
        type_text(&mut app, &tx, line);
        press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
    }
    assert_eq!(
        sent(&mut rx),
        ["steer fast forward main", "steer then run the tests"]
    );
    let rows = live_rows(&mut app)?;
    assert!(has_row(&rows, "Steering · 2"), "{rows:#?}");

    app.reduce_agent(injected("fast forward main"));
    let rows = live_rows(&mut app)?;
    assert!(has_row(&rows, "Steering · 1"), "{rows:#?}");
    assert!(has_row(&rows, "then run the tests"), "{rows:#?}");

    app.reduce_agent(injected("then run the tests"));
    let rows = live_rows(&mut app)?;
    assert!(!has_row(&rows, "Steering"), "{rows:#?}");
    Ok(())
}

/// Dies with a popup swallowing Ctrl+C: `/` on an empty box and `@` after a word each opened a
/// view whose key handler ignored the key, so the box looked frozen.
#[test]
fn ctrl_c_closes_an_open_popup_before_it_clears_the_draft() -> TestResult {
    let mut app = app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/");
    assert!(app.bottom_open());
    ctrl_c(&mut app, &tx);
    assert!(!app.bottom_open(), "the slash popup stays after ctrl-c");

    type_text(&mut app, &tx, "mail bob@");
    assert!(app.bottom_open());
    ctrl_c(&mut app, &tx);
    assert!(!app.bottom_open(), "the file popup stays after ctrl-c");
    assert_eq!(app.composer_text(), "mail bob", "closing keeps the draft");
    ctrl_c(&mut app, &tx);
    assert_eq!(app.composer_text(), "");
    assert!(sent(&mut rx).is_empty());
    Ok(())
}

/// Dies with a box of only blank lines counted empty by Ctrl+C: nothing cleared, and the press
/// aborted the running turn and armed the quit window instead.
#[test]
fn ctrl_c_clears_a_whitespace_only_draft_without_aborting() -> TestResult {
    let mut app = app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.reduce_agent(AgentEvent::AgentStart);
    app.set_draft_if_empty("  \n \n");
    assert!(!app.composer_text().is_empty());
    ctrl_c(&mut app, &tx);
    assert_eq!(app.composer_text(), "");
    assert!(sent(&mut rx).is_empty(), "the clear aborted the run");

    ctrl_c(&mut app, &tx);
    assert_eq!(sent(&mut rx), ["abort"], "an empty box still aborts");
    Ok(())
}
