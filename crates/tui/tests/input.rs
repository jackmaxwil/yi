//! The message box while a turn runs: the steering row, and what Ctrl+C does to a draft.
use crate::common;

use std::error::Error;

use crate::scratch::Scratch;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use yi_tui::Command;
use yi_tui::app::{App, TuiOptions};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_tui::port::Reply;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, UserContent};

type TestResult = Result<(), Box<dyn Error>>;

fn app() -> App {
    app_in("/tmp")
}

fn app_in(cwd: &str) -> App {
    let options = TuiOptions {
        session_name: "input".to_owned(),
        cwd: cwd.to_owned(),
        ..common::options()
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

/// Dies with `wait-idle` walking on before a turn started (A10): a busy prompt was counted off the
/// moment the runtime called it a steer, so one that missed the run's last boundary and opened a
/// turn of its own left every later prompt looking already begun.
#[test]
fn a_busy_prompt_is_awaited_until_a_run_reads_it() -> TestResult {
    for opens_its_own_turn in [false, true] {
        let mut app = app();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        for line in ["first", "second"] {
            type_text(&mut app, &tx, line);
            press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
        }
        app.apply(Reply::Notice("a turn was already running".to_owned()));
        app.reduce_agent(AgentEvent::AgentStart);
        app.reduce_agent(injected("first"));
        if opens_its_own_turn {
            app.reduce_agent(AgentEvent::AgentEnd {
                messages: Vec::new(),
                reason: None,
            });
            assert!(!app.turn_idle(), "the steer's own turn is about to start");
            app.reduce_agent(AgentEvent::AgentStart);
        }
        app.reduce_agent(injected("second"));
        app.reduce_agent(AgentEvent::AgentEnd {
            messages: Vec::new(),
            reason: None,
        });
        assert!(app.turn_idle(), "own turn {opens_its_own_turn}: both read");
        type_text(&mut app, &tx, "third");
        press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
        assert!(
            app.awaiting_turn(),
            "own turn {opens_its_own_turn}: a later prompt reads as already begun"
        );
    }
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
    assert!(!app.bottom_open(), "the slash list stays after ctrl-c");
    assert_eq!(app.composer_text(), "/");
    ctrl_c(&mut app, &tx);
    assert_eq!(app.composer_text(), "");

    let mut files = app_in("/tmp");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut files, &tx, "mail bob@");
    assert!(files.bottom_open());
    ctrl_c(&mut files, &tx);
    assert!(!files.bottom_open(), "the file list stays after ctrl-c");
    assert_eq!(
        files.composer_text(),
        "mail bob@",
        "closing keeps the draft"
    );
    ctrl_c(&mut files, &tx);
    assert_eq!(files.composer_text(), "");
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
        reason: None,
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

/// Dies with a paste into an open `/` list missing the bordered box: the text belongs in the
/// composer, which stays on screen above the matches.
#[test]
fn a_paste_with_a_command_list_open_fills_the_composer() -> TestResult {
    for (pasted, text, tail) in [("mod\nel", "/mod\nel", 0), ("mod\r\nel\n", "/mod\nel\n", 1)] {
        let mut app = app();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        type_text(&mut app, &tx, "/");
        paste(&mut app, &tx, pasted);
        assert_eq!(app.composer_text(), text, "{pasted:?}");
        let rows = common::live_rows(&mut app)?;
        let first = rows
            .iter()
            .position(|row| *row == format!(" │{:<76}│", "/mod"))
            .ok_or_else(|| format!("{pasted:?}: no /mod row: {rows:#?}"))?;
        let want = [format!(" │{:<76}│", "el"), format!(" │{:<76}│", "")];
        assert_eq!(rows.get(first + 1), want.first(), "{pasted:?}: {rows:#?}");
        if tail == 1 {
            assert_eq!(rows.get(first + 2), want.get(1), "{pasted:?}: {rows:#?}");
        }
    }
    Ok(())
}

fn submitted(app: &mut App, tx: &UnboundedSender<Command>, line: &str) {
    paste(app, tx, line);
    press(app, tx, KeyCode::Enter, KeyModifiers::NONE);
}

/// The transcript's rows as the person reads them, one string.
fn transcript(app: &App) -> String {
    app.reflowed(200)
        .iter()
        .map(|row| row.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Dies with `/plna` reaching the model as a prompt: a typo'd command cost a turn and was
/// answered as prose, where a typo should be told so for nothing.
#[test]
fn a_slash_line_naming_no_command_answers_locally_and_names_the_escape() -> TestResult {
    let mut app = app();
    app.set_width(120);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    submitted(&mut app, &tx, "/plna now");
    assert_eq!(sent(&mut rx), Vec::<String>::new(), "a model turn started");
    let rows = transcript(&app);
    assert!(
        rows.contains("unknown command: /plna · start the line with // to send it as a prompt"),
        "{rows}"
    );
    Ok(())
}

/// The popup is where a typo is typed: Enter on a name nothing matches threw the line away
/// without a word.
#[test]
fn a_typo_entered_in_the_slash_popup_gets_the_same_notice() -> TestResult {
    let mut app = app();
    app.set_width(120);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/plna");
    press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!app.bottom_open());
    assert_eq!(sent(&mut rx), Vec::<String>::new());
    assert!(transcript(&app).contains("unknown command: /plna"));
    Ok(())
}

/// `//word` is the escape, and anything that is no word (a path, a code comment) is a prompt as
/// typed: neither may be taken for a command.
#[test]
fn a_double_slash_and_a_slash_path_are_prompts() -> TestResult {
    let mut app = app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for line in [
        "//plna is a word",
        "/usr/local/bin is on my PATH",
        "// a comment",
        "/* a block comment */",
    ] {
        submitted(&mut app, &tx, line);
    }
    assert_eq!(
        sent(&mut rx),
        [
            "prompt /plna is a word",
            "prompt /usr/local/bin is on my PATH",
            "prompt // a comment",
            "prompt /* a block comment */",
        ]
    );
    Ok(())
}

/// Dies with `/learn` panicking off the runtime thread or starting a turn that never opens its
/// skill: it is sent as a prompt, carrying the user's focus.
#[test]
fn learn_is_sent_as_a_prompt_that_opens_its_skill() -> TestResult {
    let mut app = app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    submitted(&mut app, &tx, "/learn the stale tag");
    let sent = sent(&mut rx);
    let [line] = sent.as_slice() else {
        return Err(format!("{sent:?}").into());
    };
    assert!(
        line.starts_with("prompt /learn: read(\"yi://skills/learn\")"),
        "{line}"
    );
    assert!(line.ends_with("the stale tag"), "{line}");
    Ok(())
}

/// The notice says to type `//`, and a typed `/` opens the popup: Enter there ran `//model` and
/// `//undo` as commands and answered `//plna` with the notice that had just named the escape.
#[test]
fn a_double_slash_typed_in_the_popup_is_a_prompt() -> TestResult {
    let mut app = app();
    app.set_width(120);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for line in [
        "//plna is a word",
        "//model",
        "//undo",
        "/usr/local/bin is on my PATH",
    ] {
        type_text(&mut app, &tx, line);
        press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
        assert!(!app.bottom_open(), "{line}: a view stayed open");
    }
    assert_eq!(
        sent(&mut rx),
        [
            "prompt /plna is a word",
            "prompt /model",
            "prompt /undo",
            "prompt /usr/local/bin is on my PATH",
        ]
    );
    let rows = transcript(&app);
    assert!(!rows.contains("unknown command"), "{rows}");
    Ok(())
}

fn enter(app: &mut App, tx: &UnboundedSender<Command>) {
    press(app, tx, KeyCode::Enter, KeyModifiers::NONE);
}

/// Dies with Enter in the `/` list submitting the typed `/mo` as a command of that name, which
/// answers "unknown command"; the highlighted match is what runs.
#[test]
fn enter_in_the_command_list_runs_the_highlighted_match() -> TestResult {
    let mut app = app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/mo");
    enter(&mut app, &tx);
    assert!(app.bottom_open(), "/model opens the model picker");
    assert_eq!(app.composer_text(), "");
    assert!(sent(&mut rx).is_empty());
    assert!(!transcript(&app).contains("unknown command"));
    Ok(())
}

/// Dies with Esc leaving the `/` the list opened with in the box, so the next word was a command.
#[test]
fn escape_closes_the_command_list_and_empties_the_box() -> TestResult {
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/pl");
    press(&mut app, &tx, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.bottom_open());
    assert_eq!(app.composer_text(), "");
    Ok(())
}

/// Dies with Tab dropping the list's highlight: it fills the highlighted name, not the typed prefix.
#[test]
fn tab_in_the_command_list_fills_the_highlighted_name_for_arguments() -> TestResult {
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/p");
    press(&mut app, &tx, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, &tx, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.composer_text(), "/plantree ");
    assert!(!app.bottom_open(), "arguments are typed in the box");
    Ok(())
}

/// Dies with Tab on a pasted `/name` draft, where no list is open, doing nothing: one match fills
/// in place, several open the list and keep the draft.
#[test]
fn tab_on_a_slash_draft_fills_one_match_and_lists_several() -> TestResult {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut one = app();
    paste(&mut one, &tx, "/mod");
    press(&mut one, &tx, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(one.composer_text(), "/model ");
    assert!(!one.bottom_open());
    let mut several = app();
    paste(&mut several, &tx, "/p");
    press(&mut several, &tx, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(several.composer_text(), "/p");
    assert!(several.bottom_open(), "several matches open the list");
    Ok(())
}

/// Dies with the list replacing the box: the matches sit above a box that holds the typed text,
/// painted at a pane that does not start at the screen's origin.
#[test]
fn the_command_list_sits_above_the_box_at_a_nonzero_offset() -> TestResult {
    use ratatui::layout::Rect;
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/mo");
    let backend = common::VT100Backend::with_scrollback(80, 40, 200);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 3)?;
    terminal.set_viewport_area(Rect::new(0, 0, 80, 40));
    let area = Rect::new(7, 5, 60, 30);
    let mut scroll = 0;
    terminal.draw(|frame| {
        let _ = yi_tui::render::paint_pane(&mut app, None, frame.buffer_mut(), area, &mut scroll);
    })?;
    let rows: Vec<String> = (0..40)
        .map(|row| terminal.backend().row_text(row))
        .collect();
    let at = |needle: &str| rows.iter().position(|row| row.contains(needle));
    let list = at("model").ok_or_else(|| format!("no match row: {rows:#?}"))?;
    let draft = at("│/mo").ok_or_else(|| format!("no draft row: {rows:#?}"))?;
    assert!(list < draft, "matches above the box: {rows:#?}");
    assert!(rows.get(list).is_some_and(|row| row.starts_with("       ")));
    Ok(())
}

/// Dies with Esc and Ctrl-C throwing away a draft the list was opened over: Tab on a pasted `/p`
/// opens the list, and closing it must leave the draft.
#[test]
fn closing_a_tab_opened_list_keeps_the_draft() -> TestResult {
    for close in [
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        let mut app = app();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        paste(&mut app, &tx, "/p");
        press(&mut app, &tx, KeyCode::Tab, KeyModifiers::NONE);
        assert!(app.bottom_open());
        press(&mut app, &tx, close.0, close.1);
        assert!(!app.bottom_open());
        assert_eq!(app.composer_text(), "/p", "{close:?}");
    }
    Ok(())
}

/// Dies with Shift-Tab filling the highlighted command like a plain Tab.
#[test]
fn a_modified_tab_in_the_command_list_does_not_fill() -> TestResult {
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/p");
    press(&mut app, &tx, KeyCode::Tab, KeyModifiers::SHIFT);
    assert_eq!(app.composer_text(), "/p");
    assert!(app.bottom_open());
    Ok(())
}

/// Dies with the list's `/` text left in the box when an approval card replaces the list.
#[test]
fn an_approval_replacing_the_command_list_leaves_no_stray_text() -> TestResult {
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/mo");
    let (reply, _answer) = std::sync::mpsc::channel();
    app.open_approval(yi_tui::AskRequest {
        title: "run it".to_owned(),
        description: String::new(),
        grants: Vec::new(),
        tool_call_id: None,
        reply,
    });
    assert_eq!(app.composer_text(), "");
    Ok(())
}

/// Dies with the list taller than the screen: at 60x9 the box that holds the typed text was
/// pushed off, so the text was invisible.
#[test]
fn the_command_list_leaves_the_box_on_a_nine_row_screen() -> TestResult {
    let mut app = app();
    app.set_width(60);
    app.set_rows(9);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "/");
    let rows = common::rows_sized(&mut app, 60, 9)?;
    assert!(
        rows.iter().any(|row| row.starts_with(" │/")),
        "the box with the typed `/` is on screen: {rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("[… ") && row.contains("screen cap")),
        "the cut is named: {rows:#?}"
    );
    Ok(())
}
