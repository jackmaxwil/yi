//! The message box while a turn runs: the steering row, and what Ctrl+C does to a draft.
use crate::common;

use std::error::Error;

use crate::scratch::Scratch;
use common::test_model;
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
    app_in("/tmp")
}

fn app_in(cwd: &str) -> App {
    let options = TuiOptions {
        model: test_model("faux-1"),
        session_name: "input".to_owned(),
        cwd: cwd.to_owned(),
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
            Command::Prompt(text) => format!("prompt {text}"),
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
    let rows = common::live_rows(&mut app)?;
    assert!(has_row(&rows, "Steering · 2"), "{rows:#?}");

    app.reduce_agent(injected("fast forward main"));
    let rows = common::live_rows(&mut app)?;
    assert!(has_row(&rows, "Steering · 1"), "{rows:#?}");
    assert!(has_row(&rows, "then run the tests"), "{rows:#?}");

    app.reduce_agent(injected("then run the tests"));
    let rows = common::live_rows(&mut app)?;
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

/// Dies with one injected message removing every queued row of the same text: the row count
/// must fall by one, since each queued steer is its own message in the loop.
#[test]
fn an_injected_message_leaves_one_of_two_identical_queued_rows() -> TestResult {
    let mut app = app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.reduce_agent(AgentEvent::AgentStart);
    for _ in 0..2 {
        type_text(&mut app, &tx, "go on");
        press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
    }
    assert_eq!(sent(&mut rx), ["steer go on", "steer go on"]);
    app.reduce_agent(injected("go on"));
    let rows = common::live_rows(&mut app)?;
    assert!(has_row(&rows, "Steering · 1"), "{rows:#?}");
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let rows = common::live_rows(&mut app)?;
    assert!(!has_row(&rows, "Steering"), "{rows:#?}");
    Ok(())
}

/// Opens an `@` picker over `cwd` and types `typed` into it.
fn picker_over(cwd: &str, typed: &str) -> App {
    let mut app = app_in(cwd);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, &format!("@{typed}"));
    app
}

/// The `[…]` row an `@` picker draws over `cwd`, if it draws one.
fn picker_note(cwd: &str) -> Result<Option<String>, Box<dyn Error>> {
    let mut app = picker_over(cwd, "");
    Ok(common::live_rows(&mut app)?
        .into_iter()
        .find(|row| row.contains("[…]"))
        .map(|row| row.trim().to_owned()))
}

/// Picks what `typed` leaves selected in an `@` picker over `cwd`: what the composer then holds.
fn picked(cwd: &str, typed: &str) -> String {
    let mut app = picker_over(cwd, typed);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
    app.composer_text()
}

fn files_in(dir: &Scratch, numbers: std::ops::Range<usize>) -> TestResult {
    for n in numbers {
        std::fs::File::create(dir.join(format!("f{n:04}.txt")))?;
    }
    Ok(())
}

/// Dies with a picker whose row said "type a path for the rest" while typing a path found
/// nothing: only the first hundred names were searchable, a root `justfile` sorted behind a busy
/// directory fell out of them, and Enter on no match threw the typed text away.
#[test]
fn a_name_typed_into_the_picker_reaches_a_file_past_its_first_hundred() -> TestResult {
    let dir = Scratch::new("yi-picker")?;
    std::fs::File::create(dir.join("justfile"))?;
    std::fs::create_dir(dir.join("alpha"))?;
    for n in 0..150 {
        std::fs::File::create(dir.join(format!("alpha/a{n:03}.txt")))?;
    }
    let at = dir.display().to_string();
    assert_eq!(picked(&at, "justfile"), "justfile ");
    assert_eq!(picked(&at, "a149"), "alpha/a149.txt ");
    assert_eq!(picker_note(&at)?, None, "151 files are all of them");
    Ok(())
}

/// The walk is bounded, and says so: 5,000 names are all of them, 5,001 are a floor.
#[test]
fn a_picker_names_its_cap_one_file_past_five_thousand() -> TestResult {
    let dir = Scratch::new("yi-picker-big")?;
    files_in(&dir, 0..5_000)?;
    let at = dir.display().to_string();
    assert_eq!(picker_note(&at)?, None, "5000 files are all of them");
    files_in(&dir, 5_000..5_001)?;
    assert_eq!(
        picker_note(&at)?.as_deref(),
        Some("[…] 5000 of 5000+ files · picker cap 5000 · Esc, then type the path")
    );
    Ok(())
}

/// The crates tree of this checkout is the life-sized one: hundreds of tracked files, ignored
/// ones skipped. (The checkout root holds the test HOME, which the walk refuses to enter.)
#[test]
fn the_picker_over_this_checkouts_crates_reaches_a_file_deep_in_the_tree() -> TestResult {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()?;
    let at = crates.display().to_string();
    assert_eq!(picked(&at, "tui/src/input.rs"), "tui/src/input.rs ");
    Ok(())
}

fn paste(app: &mut App, tx: &UnboundedSender<Command>, text: &str) {
    yi_tui::input::handle_terminal_event(app, tx, Event::Paste(text.to_owned()));
}

/// Dies with a pasted Makefile recipe reaching the model with its tabs turned into three spaces
/// (and a terminal capture with its escapes dropped): `make` then refuses the recipe it was shown.
#[test]
fn a_pasted_makefile_recipe_reaches_the_prompt_with_its_tabs() -> TestResult {
    let mut app = app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    paste(
        &mut app,
        &tx,
        "build:\r\n\tcc -o app main.c\r\n\t./app --check\x1b[0m",
    );
    press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        sent(&mut rx),
        ["prompt build:\n\tcc -o app main.c\n\t./app --check\x1b[0m"]
    );
    Ok(())
}

/// The box draws what the model will read: a tab as four cells and an escape as its picture, so
/// no control byte reaches the terminal and the row still ends where the text does.
#[test]
fn a_pasted_tab_and_escape_are_drawn_as_cells_not_sent_to_the_terminal() -> TestResult {
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    paste(&mut app, &tx, "a\tb\x1b[31mc");
    let rows = common::live_rows(&mut app)?;
    assert!(has_row(&rows, "a    b\u{241b}[31mc"), "{rows:#?}");
    Ok(())
}

/// Dies with a paste into an open `/` popup landing in the composer hidden behind it: the popup
/// kept an empty query and the text sent with the next prompt.
#[test]
fn a_paste_with_a_popup_open_fills_the_popup_not_the_composer() -> TestResult {
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/");
    paste(&mut app, &tx, "mod\nel");
    assert_eq!(app.composer_text(), "");
    let rows = common::live_rows(&mut app)?;
    assert!(has_row(&rows, " /mod el"), "{rows:#?}");
    Ok(())
}
