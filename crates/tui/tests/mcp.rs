use std::error::Error;
use std::path::PathBuf;

use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::{KeyCodeValue, SingleKey};
use yi_tui::mcp::{McpPage, McpRow};
use yi_tui::popup::{BottomView, PopupResult};

use crate::common::flat;

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn key(code: KeyCodeValue) -> SingleKey {
    SingleKey {
        code,
        ctrl: false,
        alt: false,
        shift: false,
    }
}

fn server(
    name: &str,
    transport: &str,
    state: Option<&str>,
    tools: Option<usize>,
    enabled: bool,
) -> McpRow {
    McpRow {
        name: name.to_owned(),
        source: if transport == "http" {
            ".mcp.json"
        } else {
            "~/.yi/mcp.json"
        }
        .to_owned(),
        path: PathBuf::from(format!("/work/{name}.json")),
        transport: transport.to_owned(),
        target: if transport == "http" {
            "https://mcp.example/api".to_owned()
        } else {
            "uvx server".to_owned()
        },
        enabled,
        session: name.to_owned(),
        state: state.map(str::to_owned),
        tools,
    }
}

fn page() -> McpPage {
    McpPage::from_rows(
        None,
        true,
        vec![
            server("files", "stdio", Some("live"), Some(7), true),
            server("docs", "http", Some("unauthorized"), None, true),
            server("old", "stdio", None, None, false),
        ],
    )
}

fn rows(page: &McpPage, width: usize) -> Vec<String> {
    flat(&page.lines(width, &theme()))
}

#[test]
fn mcp_page_shows_header_servers_with_state_glyphs_detail_and_keys() -> TestResult {
    let rendered = rows(&page(), 110);
    let joined = rendered.join("\n");
    assert!(rendered[0].contains("mcp.enabled on"), "{joined}");
    assert!(rendered[0].contains("3 servers"), "{joined}");
    let files = rendered
        .iter()
        .find(|row| row.contains("files") && row.contains("●"))
        .ok_or("no live row")?;
    assert!(files.contains("stdio") && files.contains('7') && files.contains("~/.yi/mcp.json"));
    let docs = rendered
        .iter()
        .find(|row| row.contains("docs"))
        .ok_or("no docs row")?;
    assert!(docs.contains('!') && docs.contains("login needed") && docs.contains('—'));
    let old = rendered
        .iter()
        .find(|row| row.contains("old"))
        .ok_or("no off row")?;
    assert!(old.contains('✕'), "{old}");
    assert!(joined.contains("/work/files.json"), "{joined}");
    assert!(joined.contains("session   files · live"), "{joined}");
    let footer = rendered.last().ok_or("no footer")?;
    assert!(
        footer.contains("p probe tools") && footer.contains("l login"),
        "{footer}"
    );
    Ok(())
}

#[test]
fn mcp_page_detail_follows_the_selection_and_says_when_tools_are_unprobed() -> TestResult {
    let mut page = page();
    assert!(matches!(
        page.handle_key(&key(KeyCodeValue::Down)),
        PopupResult::Open
    ));
    let joined = rows(&page, 110).join("\n");
    assert!(joined.contains("/work/docs.json"), "{joined}");
    assert!(joined.contains("not probed · p lists tools"), "{joined}");
    assert!(joined.contains("login needed"), "{joined}");
    Ok(())
}

#[test]
fn mcp_page_when_off_explains_how_to_turn_it_on_and_lists_nothing() {
    let page = McpPage::from_rows(
        None,
        false,
        vec![server("files", "stdio", None, None, true)],
    );
    let rendered = rows(&page, 100);
    assert!(rendered[0].contains("mcp.enabled off"), "{rendered:?}");
    assert!(
        rendered.iter().any(|row| row.contains("e turns it on")),
        "{rendered:?}"
    );
}

#[test]
fn mcp_page_login_on_a_stdio_server_names_why_it_did_nothing() {
    let mut page = page();
    page.handle_key(&key(KeyCodeValue::Char('l')));
    assert!(!page.is_busy());
    let joined = rows(&page, 100).join("\n");
    assert!(
        joined.contains("files is a stdio server and has no login"),
        "{joined}"
    );
}

#[test]
fn mcp_page_escape_closes_and_a_long_row_stays_inside_the_width() {
    let mut page = page();
    assert!(matches!(
        page.handle_key(&key(KeyCodeValue::Esc)),
        PopupResult::Close
    ));
    let long = McpPage::from_rows(
        None,
        true,
        vec![server(
            "a-server-with-a-very-long-name-indeed",
            "http",
            Some("live"),
            Some(2),
            true,
        )],
    );
    for row in rows(&long, 60) {
        assert!(row.chars().count() <= 60, "{row}");
    }
}
