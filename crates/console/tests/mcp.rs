use std::error::Error;
use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Direction;
use yi_console::app::App;
use yi_console::client;
use yi_console::model::{Link, PaneContent, Zone};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::mcp::{McpPage, McpRow};

use crate::chrome::{draw, row_text};

type TestResult = Result<(), Box<dyn Error>>;

fn row(name: &str, transport: &str, state: Option<&str>, tools: Option<usize>) -> McpRow {
    McpRow {
        name: name.to_owned(),
        source: ".mcp.json".to_owned(),
        path: PathBuf::from(format!("/work/{name}.json")),
        transport: transport.to_owned(),
        target: "https://mcp.example/api".to_owned(),
        enabled: true,
        session: name.to_owned(),
        state: state.map(str::to_owned),
        tools,
    }
}

fn console_with_mcp() -> Result<App, Box<dyn Error>> {
    let mut app = App::new("/repo".to_owned(), Theme::new(ColorTier::TrueColor, true));
    app.state.link = Link::Connected;
    let pane = app
        .state
        .split_focused(Direction::Horizontal)
        .ok_or("no split")?;
    let slot = app.state.panes.get_mut(&pane).ok_or("no pane")?;
    slot.content = PaneContent::Mcp(Box::new(McpPage::from_rows(
        None,
        true,
        vec![
            row("files", "stdio", Some("live"), Some(7)),
            row("docs", "http", Some("unauthorized"), None),
        ],
    )));
    app.state.zone = Zone::Panes;
    Ok(app)
}

fn screen(terminal: &Terminal<TestBackend>) -> Vec<String> {
    (0..terminal.backend().buffer().area.height)
        .map(|y| row_text(terminal, y))
        .collect()
}

fn press(app: &mut App, code: KeyCode) {
    let (events, _received) = client::events();
    let socket = std::env::temp_dir().join("yi-mcp-no-daemon.sock");
    let (outbound, _threads) = client::spawn(socket, events);
    app.handle_event(
        &outbound,
        ratatui::crossterm::event::Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
    );
    outbound.shutdown();
}

#[test]
fn mcp_pane_draws_status_header_server_rows_detail_and_key_footer() -> TestResult {
    let mut app = console_with_mcp()?;
    let mut terminal = Terminal::new(TestBackend::new(150, 30))?;
    draw(&mut app, &mut terminal)?;
    let rows = screen(&terminal);
    let find = |needle: &str| rows.iter().find(|line| line.contains(needle));
    assert!(find("mcp.enabled on").is_some(), "{rows:#?}");
    let files = find("files").ok_or("no files row")?;
    assert!(files.contains('●') && files.contains("stdio"), "{files}");
    let docs = find("login needed").ok_or("no docs row")?;
    assert!(docs.contains('!') && docs.contains("docs"), "{docs}");
    assert!(find("/work/files.json").is_some(), "{rows:#?}");
    assert!(find("p probe").is_some(), "{rows:#?}");
    Ok(())
}

#[test]
fn mcp_pane_arrow_moves_the_detail_and_escape_closes_the_pane() -> TestResult {
    let mut app = console_with_mcp()?;
    let mut terminal = Terminal::new(TestBackend::new(150, 30))?;
    press(&mut app, KeyCode::Down);
    draw(&mut app, &mut terminal)?;
    assert!(
        screen(&terminal)
            .iter()
            .any(|line| line.contains("/work/docs.json")),
        "the detail did not follow the selection"
    );
    press(&mut app, KeyCode::Esc);
    let open = app
        .state
        .panes
        .values()
        .any(|pane| matches!(pane.content, PaneContent::Mcp(_)));
    assert!(!open, "escape left the MCP pane open");
    Ok(())
}
