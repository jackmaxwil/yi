//! `/theme`: a preview repaints the live rows only, and every close but Enter puts the palette back.
use crate::common::{self, VT100Backend};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;
use yi_tui::Command;
use yi_tui::app::App;
use yi_tui::terminal::Terminal;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// dark-nord's accent (`nord8`, #88c0d0) and the built-in truecolor dark accent (#82aaff).
const NORD: &str = "38;2;136;192;208";
const BUILT: &str = "38;2;130;170;255";

fn keys(app: &mut App, tx: &UnboundedSender<Command>, codes: impl IntoIterator<Item = KeyCode>) {
    for code in codes {
        let event = Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        yi_tui::input::handle_terminal_event(app, tx, event);
    }
}

fn typed(text: &str) -> Vec<KeyCode> {
    text.chars().map(KeyCode::Char).collect()
}

fn frame(app: &mut App, terminal: &mut Terminal<VT100Backend>) -> String {
    yi_tui::render::draw(app, terminal, None);
    terminal.backend_mut().take_written()
}

fn listed(
    app: &mut App,
    tx: &UnboundedSender<Command>,
    terminal: &mut Terminal<VT100Backend>,
) -> String {
    keys(app, tx, typed("/theme"));
    keys(app, tx, [KeyCode::Enter]);
    keys(app, tx, typed("dark-nord"));
    frame(app, terminal)
}

#[test]
fn a_preview_repaints_the_live_rows_and_esc_puts_the_palette_back() -> TestResult {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = common::app();
    let mut terminal = Terminal::new(VT100Backend::with_scrollback(80, 24, 200), 4)?;
    app.notice("a row already in scrollback");
    frame(&mut app, &mut terminal);
    let before = terminal.backend().transcript();
    let previewed = listed(&mut app, &tx, &mut terminal);
    assert!(
        previewed.contains(NORD),
        "the selected name wears its own accent: {previewed:?}"
    );
    assert!(
        !previewed.contains("\x1b[3J"),
        "a preview erased scrollback: {previewed:?}"
    );
    assert_eq!(terminal.backend().transcript().first(), before.first());
    keys(&mut app, &tx, [KeyCode::Esc]);
    frame(&mut app, &mut terminal);
    keys(&mut app, &tx, typed("/"));
    let restored = frame(&mut app, &mut terminal);
    assert!(
        restored.contains(BUILT) && !restored.contains(NORD),
        "{restored:?}"
    );
    Ok(())
}

#[test]
fn an_approval_over_an_open_theme_list_puts_the_palette_back() -> TestResult {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = common::app();
    let mut terminal = Terminal::new(VT100Backend::new(80, 24), 4)?;
    assert!(listed(&mut app, &tx, &mut terminal).contains(NORD));
    let (reply, _answered) = std::sync::mpsc::channel();
    app.open_approval(yi_tui::AskRequest {
        title: "bash requires permission".to_owned(),
        description: "make build".to_owned(),
        grants: Vec::new(),
        reply,
        tool_call_id: Some("c1".to_owned()),
    });
    let asked = frame(&mut app, &mut terminal);
    assert!(asked.contains(BUILT) && !asked.contains(NORD), "{asked:?}");
    Ok(())
}

#[test]
fn a_new_palette_leaves_a_scrolled_pane_where_it_was() {
    use ratatui::{buffer::Buffer, layout::Rect};
    let mut app = common::app();
    for row in 0..60 {
        app.notice(format!("row {row}"));
    }
    let area = Rect::new(0, 0, 80, 20);
    let mut buffer = Buffer::empty(area);
    let mut scroll = 12;
    let _ = yi_tui::render::paint_pane(&mut app, None, &mut buffer, area, &mut scroll);
    let held = scroll;
    let light = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, false);
    app.set_theme(light);
    let _ = yi_tui::render::paint_pane(&mut app, None, &mut buffer, area, &mut scroll);
    assert_eq!(
        (held, scroll),
        (12, 12),
        "a palette change jumped the pane to its bottom"
    );
}
