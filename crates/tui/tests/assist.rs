use std::error::Error;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use yi_tui::app::{App, TuiOptions};
use yi_tui::assist::{self, FileOffer};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::composer::Composer;
use yi_tui::keymap::default_keymap;

use crate::common;

type TestResult = Result<(), Box<dyn Error>>;

fn paint(composer: &mut Composer) -> Buffer {
    let height = composer.layout(40, 20);
    composer.set_frame(Style::default(), Style::default(), Style::default(), None);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 40, height.saturating_add(2)));
    composer.render(Rect::new(0, 0, 40, height), &mut buffer);
    buffer
}

fn underlined(buffer: &Buffer) -> bool {
    buffer.content().iter().any(|cell| {
        cell.modifier.contains(Modifier::UNDERLINED)
            && cell.fg == Color::Reset
            && cell.underline_color == Color::Rgb(255, 95, 95)
    })
}

#[test]
fn a_fake_engine_underlines_a_prose_typo_and_skips_code() -> TestResult {
    let _fake = assist::fake_engine();
    for (text, marked) in [
        ("helllo", false),
        ("helllo ", true),
        ("`helllo`", false),
        ("foo_bar", false),
        ("foo_barrr", false),
        ("/plan", false),
        ("/helllo", false),
        ("hellloWorld", false),
        ("see helllo", false),
        ("see helllo ", true),
        ("see helllo now", true),
    ] {
        let mut composer = Composer::default();
        composer.set_text(text);
        composer.refresh();
        assert_eq!(underlined(&paint(&mut composer)), marked, "{text}");
    }
    Ok(())
}

#[test]
fn tab_accepts_a_ghost_and_one_undo_removes_it() -> TestResult {
    let _words = assist::observe("zzqhello");
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "zzqh");
    press(&mut app, &tx, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.composer_text(), "zzqhello ");
    press(&mut app, &tx, KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(app.composer_text(), "zzqh");
    Ok(())
}

#[test]
fn typing_through_a_ghost_keeps_the_rest() -> TestResult {
    let _words = assist::observe("zzqhello");
    let mut composer = Composer::default();
    composer.set_text("zzqh");
    composer.refresh();
    assert_eq!(assist::ghost_suffix("zzqh", 4), "ello");
    composer.set_text("zzqhe");
    composer.refresh();
    assert_eq!(assist::ghost_suffix("zzqhe", 5), "llo");
    Ok(())
}

#[test]
fn autocorrect_stays_off_until_asked() -> TestResult {
    let _fake = assist::fake_engine();
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "helllo ");
    assert_eq!(app.composer_text(), "helllo ");
    let _auto = assist::autocorrect_on();
    type_text(&mut app, &tx, "helllo ");
    assert!(
        app.composer_text().contains("hello "),
        "{:?}",
        app.composer_text()
    );
    Ok(())
}

#[test]
fn ctrl_dot_lists_a_guess_for_the_word() -> TestResult {
    let _fake = assist::fake_engine();
    let mut app = app();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    type_text(&mut app, &tx, "helllo");
    press(&mut app, &tx, KeyCode::Char('.'), KeyModifiers::CONTROL);
    let rows = common::live_rows(&mut app)?;
    assert!(rows.iter().any(|row| row.contains("hello")), "{rows:#?}");
    press(&mut app, &tx, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.composer_text(), "hello");
    Ok(())
}

#[test]
fn a_closed_shortcode_rewrites_before_a_list() -> TestResult {
    let mut composer = Composer::default();
    composer.set_text(":joy:");
    composer.refresh();
    assert_eq!(composer.text(), "😂");
    Ok(())
}

#[test]
fn file_rows_keep_the_previous_list_while_a_walk_is_still_out() {
    let previous = vec!["src/main.rs".to_owned(), "justfile".to_owned()];
    let FileOffer {
        labels,
        searching,
        note,
    } = assist::file_rows(&previous, None, "just", false, 10);
    assert!(searching);
    assert_eq!(labels, vec!["justfile".to_owned()]);
    assert!(note.is_none());
}

#[test]
fn blend_keeps_the_longer_suffix_and_otherwise_the_first() {
    assert_eq!(
        assist::blend(Some("lo"), Some("lo there"), None).as_deref(),
        Some("lo there")
    );
    assert_eq!(
        assist::blend(Some("xyz"), Some("abc"), None).as_deref(),
        Some("xyz")
    );
}

#[test]
fn a_utf16_range_lands_on_a_character() {
    assert_eq!(assist::char_index("😀x", 2), 1);
}

fn app() -> App {
    App::new(
        TuiOptions {
            session_name: "assist".to_owned(),
            ..common::options()
        },
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        80,
    )
}

fn press(
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<yi_tui::Command>,
    code: KeyCode,
    modifiers: KeyModifiers,
) {
    yi_tui::input::handle_terminal_event(app, tx, Event::Key(KeyEvent::new(code, modifiers)));
}

fn type_text(app: &mut App, tx: &tokio::sync::mpsc::UnboundedSender<yi_tui::Command>, text: &str) {
    for ch in text.chars() {
        press(app, tx, KeyCode::Char(ch), KeyModifiers::NONE);
    }
}

#[test]
fn a_spell_mark_is_a_curl_and_a_link_stays_straight() -> TestResult {
    let curled = yi_tui::term::spell_marks(b"\x1b[4m\x1b[58;2;255;95;95mh");
    assert_eq!(curled, b"\x1b[4m\x1b[4:3m\x1b[58;2;255;95;95mh");
    assert_eq!(yi_tui::term::spell_marks(b"\x1b[4mlink"), b"\x1b[4mlink");
    let mut out = Vec::new();
    {
        let mut writer = yi_tui::term::Squiggle::new(&mut out);
        std::io::Write::write_all(&mut writer, b"\x1b[58;")?;
        std::io::Write::write_all(&mut writer, b"5;9m")?;
        std::io::Write::flush(&mut writer)?;
    }
    assert_eq!(out, b"\x1b[4:3m\x1b[58;5;9m");
    Ok(())
}
