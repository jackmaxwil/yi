//! The editor pane's lexer is primed from the top of the file, so a construct that opens
//! above the viewport still colours the rows inside it.

use std::error::Error;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use yi_console::app::MouseKind;
use yi_console::model::Editor;
use yi_tui::highlight::Token;

type TestResult = Result<(), Box<dyn Error>>;

fn commented_file(rows: usize) -> Editor {
    let mut lines = vec!["/* the header comment this file opens with".to_owned()];
    for n in 2..=rows {
        lines.push(format!("   still inside the comment, line {n}"));
    }
    lines.push("*/".to_owned());
    Editor {
        path: "demo.rs".to_owned(),
        text: Box::new(tui_textarea::TextArea::from(lines)),
        dirty: false,
        mtime: None,
        stale: false,
        scroll_top: 0,
        primed: None,
        last_cursor: (0, 0),
        lang: Some("rs".to_owned()),
        crlf: false,
        final_newline: true,
        workdir: "/repo".to_owned(),
    }
}

fn first_token(editor: &mut Editor, line: &str) -> Option<Token> {
    let mut lang = editor.primed_lang()?;
    yi_tui::highlight::tokens(line, &mut lang)
        .first()
        .map(|(_, _, token)| *token)
}

#[test]
fn a_row_below_the_viewport_top_is_coloured_by_what_opened_above_it() -> TestResult {
    let mut editor = commented_file(60);
    let row = "   still inside the comment, line 41";

    // A lexer that has read nothing sees the row as code: the trailing 41 is a number.
    let mut fresh = yi_tui::highlight::lang_for("rs").ok_or("rust has a grammar")?;
    assert_eq!(
        yi_tui::highlight::tokens(row, &mut fresh)
            .first()
            .map(|(_, _, token)| *token),
        Some(Token::Number),
        "without the rows above it, the row lexes as plain code"
    );

    // Primed at the viewport's top, the same row is inside the comment that opened on
    // line 1 — the whole point of advancing the parse over the rows above the screen.
    editor.scroll_top = 40;
    assert_eq!(
        first_token(&mut editor, row),
        Some(Token::Comment),
        "a comment opened above the viewport still colours the rows inside it"
    );
    Ok(())
}

#[test]
fn the_primed_state_is_cached_and_advances_with_the_viewport() -> TestResult {
    let mut editor = commented_file(60);
    editor.scroll_top = 20;
    editor.primed_lang().ok_or("a rust file has a lexer")?;
    assert_eq!(editor.primed.as_ref().map(|(at, _)| *at), Some(20));

    // Moving down reuses the cached state; moving back above it starts over.
    editor.scroll_top = 30;
    editor.primed_lang().ok_or("still a lexer")?;
    assert_eq!(editor.primed.as_ref().map(|(at, _)| *at), Some(30));
    editor.scroll_top = 5;
    editor.primed_lang().ok_or("still a lexer")?;
    assert_eq!(editor.primed.as_ref().map(|(at, _)| *at), Some(5));

    // A file the highlighter has no grammar for primes nothing rather than failing.
    editor.lang = Some("unknown-extension".to_owned());
    editor.primed = None;
    assert!(editor.primed_lang().is_none());
    Ok(())
}

pub(crate) fn console() -> (yi_console::app::App, yi_console::client::Outbound) {
    let (events, _received) = yi_console::client::events();
    let socket = std::env::temp_dir().join("yi-editor-no-daemon.sock");
    let (outbound, _threads) = yi_console::client::spawn(socket, events);
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
    let mut app = yi_console::app::App::new("/repo".to_owned(), theme);
    app.state.zone = yi_console::model::Zone::Panes;
    (app, outbound)
}

pub(crate) fn key(
    app: &mut yi_console::app::App,
    outbound: &yi_console::client::Outbound,
    key: KeyEvent,
) {
    app.handle_event(outbound, ratatui::crossterm::event::Event::Key(key));
}

/// The palette's `query`, typed and entered: `e <path>` opens an editor, `/<text>` finds.
pub(crate) fn palette(
    app: &mut yi_console::app::App,
    outbound: &yi_console::client::Outbound,
    query: &str,
) {
    key(
        app,
        outbound,
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::ALT),
    );
    for c in query.chars() {
        key(
            app,
            outbound,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    key(
        app,
        outbound,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
}

fn editor(app: &yi_console::app::App) -> Option<&Editor> {
    match &app.state.focused_pane()?.content {
        yi_console::model::PaneContent::Editor(editor) => Some(editor),
        _ => None,
    }
}

fn draw(app: &mut yi_console::app::App, terminal: &mut Terminal<TestBackend>) -> TestResult {
    let theme = app.theme;
    terminal.draw(|frame| {
        let mut view = yi_console::render::compute_view(app, frame.area(), &theme);
        yi_console::render::render(app, frame, &mut view, &theme);
    })?;
    Ok(())
}

fn row_text(terminal: &Terminal<TestBackend>, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
        .collect()
}

/// The screen row holding `needle` and the column where it starts, in cells.
fn find_row(terminal: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16, String)> {
    (0..terminal.backend().buffer().area.height).find_map(|y| {
        let row = row_text(terminal, y);
        let at = row.find(needle)?;
        let x = u16::try_from(row.get(..at)?.chars().count()).ok()?;
        Some((y, x, row))
    })
}

/// Dies with a Makefile's recipes drawn flush-left: the buffer drops a tab like any control
/// character, and a click then landed one character left of the one drawn under it.
#[test]
fn a_tab_draws_to_its_stop_and_a_click_lands_on_the_character_drawn_there() -> TestResult {
    let (mut app, outbound) = console();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/makefile.txt");
    palette(&mut app, &outbound, &format!("e {path}"));
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    draw(&mut app, &mut terminal)?;
    let (_, target, _) = find_row(&terminal, " 1 build:").ok_or("no build: row")?;
    let (y, at, row) = find_row(&terminal, "@cargo build").ok_or("no recipe row")?;
    assert!(row.contains(" 2     @cargo build "), "{row:?}");
    assert_eq!(
        at,
        target + 3 + 4,
        "the recipe sits one tab stop in: {row:?}"
    );
    let c = at.saturating_add(1);
    app.handle_mouse(&outbound, MouseKind::Down, c, y);
    app.handle_mouse(&outbound, MouseKind::Up, c, y);
    assert_eq!(editor(&app).map(|e| e.text.cursor()), Some((1, 2)));
    let theme = app.theme;
    let view = yi_console::render::compute_view(&mut app, terminal.get_frame().area(), &theme);
    assert_eq!(
        view.editor_cursor,
        Some((c, y)),
        "the cursor cell is the clicked one"
    );
    outbound.shutdown();
    Ok(())
}

/// Dies with the cursor drawn on the character after an emoji with VS16 rather than past it:
/// its chars sum to one cell where the buffer draws two.
#[test]
fn the_cursor_sits_past_an_emoji_drawn_two_cells_wide() -> TestResult {
    let dir = crate::scratch::Scratch::new("yi-editor-emoji")?;
    let path = dir.join("emoji.txt");
    std::fs::write(&path, "a\u{2764}\u{fe0f}Q\n")?;
    let (mut app, outbound) = console();
    palette(&mut app, &outbound, &format!("e {}", path.display()));
    key(
        &mut app,
        &outbound,
        KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    draw(&mut app, &mut terminal)?;
    let buffer = terminal.backend().buffer();
    let (x, y) = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .find(|&cell| buffer.cell(cell).is_some_and(|cell| cell.symbol() == "Q"))
        .ok_or("no Q drawn")?;
    let theme = app.theme;
    let view = yi_console::render::compute_view(&mut app, terminal.get_frame().area(), &theme);
    assert_eq!(view.editor_cursor, Some((x + 1, y)));
    outbound.shutdown();
    Ok(())
}

/// This repository's own source, `lines` long, ending on a line nothing else in it says.
fn long_file(dir: &std::path::Path, lines: usize) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let mut sources: Vec<std::path::PathBuf> = std::fs::read_dir(root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("src/lib.rs"))
        .filter(|path| path.is_file())
        .collect();
    sources.sort();
    let mut text = Vec::new();
    for source in sources.iter().cycle().take(10_000) {
        text.extend(std::fs::read_to_string(source)?.lines().map(str::to_owned));
        if text.len() >= lines {
            break;
        }
    }
    text.truncate(lines.saturating_sub(1));
    text.push("// the line the find is for".to_owned());
    let path = dir.join(format!("long-{lines}.rs"));
    std::fs::write(&path, text.join("\n") + "\n")?;
    Ok(path)
}

/// Dies with a file past the cursor's u16 row index opening anyway, where every jump below row
/// 65,535 lands on it, or with the last addressable line refused; at the limit it is reached.
#[test]
fn the_editor_opens_65536_lines_and_refuses_one_more_by_name() -> TestResult {
    let dir = crate::scratch::Scratch::new("yi-editor-limit")?;
    let (mut app, outbound) = console();
    palette(
        &mut app,
        &outbound,
        &format!("e {}", long_file(&dir, 65_536)?.display()),
    );
    palette(&mut app, &outbound, "/the line the find is for");
    assert_eq!(editor(&app).map(|e| e.text.cursor()), Some((65_535, 3)));
    let (mut app, outbound) = console();
    palette(
        &mut app,
        &outbound,
        &format!("e {}", long_file(&dir, 65_537)?.display()),
    );
    assert!(editor(&app).is_none(), "the longer file is refused");
    let banner = app.state.banner.clone().unwrap_or_default();
    assert!(
        banner.contains("65537 lines is over the 65,536-line editor limit"),
        "{banner}"
    );
    outbound.shutdown();
    Ok(())
}

/// Dies with `no match` for a match earlier on the cursor's own line: the wrap stopped short of
/// the row it started on.
#[test]
fn find_wraps_to_a_match_earlier_on_the_cursors_line() -> TestResult {
    let dir = crate::scratch::Scratch::new("yi-editor-find")?;
    let path = dir.join("one.rs");
    std::fs::write(&path, "let a = 1; let b = a;\n")?;
    let (mut app, outbound) = console();
    palette(&mut app, &outbound, &format!("e {}", path.display()));
    if let Some(yi_console::model::PaneContent::Editor(editor)) =
        app.state.focused_pane_mut().map(|pane| &mut pane.content)
    {
        editor
            .text
            .move_cursor(tui_textarea::CursorMove::Jump(0, 15));
    }
    palette(&mut app, &outbound, "/a =");
    assert_eq!(editor(&app).map(|e| e.text.cursor()), Some((0, 4)));
    assert_eq!(app.state.banner, None);
    outbound.shutdown();
    Ok(())
}
