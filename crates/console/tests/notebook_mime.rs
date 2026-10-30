use ratatui::text::Line;
use serde_json::json;
use yi_console::model::NbCell;
use yi_console::panes::notebook_lines;
use yi_tui::colors::{ColorTier, Theme};
use yi_types::message::{AgentMessage, Content};

mod session {
    include!("../../tui/tests/common/session.rs");
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn rows(cells: &[NbCell], width: usize, full: bool) -> Vec<String> {
    let theme = Theme::new(ColorTier::TrueColor, true);
    notebook_lines(cells, width, full, &theme, "⌘W")
        .iter()
        .map(|line: &Line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

/// The recorded reply that carries every markdown block a notebook must draw: a heading, a
/// numbered list, bold, a table and a fence.
const MARKDOWN_SESSION: &str = include_str!("../../tui/tests/fixtures/sessions/01a0d6e4.jsonl");

fn recorded_markdown() -> Result<String, Box<dyn std::error::Error>> {
    let mut best = String::new();
    for message in session::session_entries(MARKDOWN_SESSION)? {
        if let AgentMessage::Assistant { content, .. } = &message {
            for block in content {
                if let Content::Text { text, .. } = block
                    && text.len() > best.len()
                {
                    best.clone_from(text);
                }
            }
        }
    }
    Ok(best)
}

/// A recorded kernel result's details, which is the JSON object a notebook renders as a
/// tree: a real payload, in the shape the tool wrote it.
fn recorded_json() -> Result<String, serde_json::Error> {
    let details = session::session_entries(recorded_session())?
        .into_iter()
        .find_map(|message| match message {
            AgentMessage::ToolResult {
                tool_name, details, ..
            } if tool_name == "ipython" => details,
            _ => None,
        })
        .unwrap_or_default();
    serde_json::to_string(&details)
}

fn recorded_session() -> &'static str {
    include_str!("../../tui/tests/fixtures/sessions/01a13357.jsonl")
}

fn kind_cell(mime: Option<&str>, result: &str) -> NbCell {
    NbCell {
        number: 1,
        result: Some(result.to_owned()),
        result_mime: mime.map(str::to_owned),
        ..NbCell::default()
    }
}

/// Dies with a markdown result drawn raw: the heading keeps its `#`, the table its pipes,
/// and the chat's own renderer never ran.
#[test]
fn a_markdown_result_draws_its_blocks_as_the_chat_does() -> TestResult {
    let source = recorded_markdown()?;
    assert!(source.len() > 4_000, "the recorded reply is life-sized");
    let drawn = rows(&[kind_cell(Some("text/markdown"), &source)], 72, true);
    for (raw, drawn_as) in [
        ("# Yi codebase assessment", "Yi codebase assessment"),
        ("## What it is", "What it is"),
    ] {
        assert!(source.contains(raw), "{raw:?} left the recording");
        assert!(
            drawn.iter().any(|row| row.contains(drawn_as)),
            "{drawn_as:?}: {drawn:#?}"
        );
        let raw_row = drawn.iter().find(|row| row.contains(raw));
        assert_eq!(raw_row, None, "{raw:?} drawn raw");
    }
    assert!(
        drawn.iter().any(|row| row.contains("│ dimension")),
        "no table row: {drawn:#?}"
    );
    assert!(
        !drawn.iter().any(|row| row.contains("|---|")),
        "the table's rule drawn raw"
    );
    assert!(
        drawn
            .iter()
            .any(|row| row.contains("1. Crate line budgets")),
        "no list row"
    );
    Ok(())
}

/// Dies with a JSON result drawn as its one compact line, or with the cap row silent.
#[test]
fn a_json_result_draws_pretty_and_names_the_cap_it_collapsed_past() -> TestResult {
    let source = recorded_json()?;
    let drawn = rows(&[kind_cell(Some("application/json"), &source)], 72, true);
    assert!(
        drawn.iter().any(|row| row.contains("  \"stdout\":")),
        "not pretty-printed: {drawn:#?}"
    );
    assert!(
        !drawn.iter().any(|row| row.contains("{\"status\":")),
        "the compact one-line source was drawn"
    );
    let long = json!({"crates": (1..=40).map(|n| format!("crate-{n}")).collect::<Vec<_>>()});
    let cut = rows(
        &[kind_cell(Some("application/json"), &long.to_string())],
        72,
        false,
    );
    let notice = cut
        .iter()
        .find(|row| row.contains("[…] result first 20 of"))
        .ok_or("cap row silent")?;
    assert!(notice.contains("notebook cap 20"), "{notice:?}");
    assert!(notice.contains("v shows every row"), "{notice:?}");
    let whole = rows(
        &[kind_cell(Some("application/json"), &long.to_string())],
        72,
        true,
    );
    assert!(
        whole.iter().any(|row| row.contains("crate-40")),
        "{whole:#?}"
    );
    Ok(())
}

/// Dies with an unknown mime re-routed: a type no renderer names draws the value as the
/// text it already was, byte for byte beside a cell that named no mime at all.
#[test]
fn a_mime_no_renderer_names_draws_the_value_as_text() -> TestResult {
    let source = "# not a heading\n| a | b |\n{\"one\": 1}";
    let plain = rows(&[kind_cell(None, source)], 72, false);
    for mime in ["text/html", "text/plain", "image/svg+xml"] {
        assert_eq!(
            rows(&[kind_cell(Some(mime), source)], 72, false),
            plain,
            "{mime} was re-routed"
        );
    }
    Ok(())
}

/// Dies with a python result drawn flat: its keyword keeps the theme's text colour.
#[test]
fn a_code_result_is_highlighted_by_the_language_its_mime_names() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let keyword = |mime: Option<&str>| -> Option<Option<ratatui::style::Color>> {
        notebook_lines(
            &[kind_cell(mime, "def tide(sea):\n    return sea")],
            72,
            false,
            &theme,
            "⌘W",
        )
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "def")
        .map(|span| span.style.fg)
    };
    let coloured = keyword(Some("text/x-python")).ok_or("no def span")?;
    assert_ne!(coloured, Some(theme.text), "the keyword was drawn flat");
    assert_eq!(keyword(None), None, "an unnamed mime was split into tokens");
    Ok(())
}

/// Dies with a declared JSON value that is not JSON drawn pretty anyway, or with no row
/// saying the pane fell back to text.
#[test]
fn a_json_result_that_does_not_parse_says_it_drew_as_text() -> TestResult {
    let drawn = rows(
        &[kind_cell(Some("application/json"), "{\"cut\": ")],
        72,
        false,
    );
    assert!(
        drawn.iter().any(
            |row| row.contains("application/json result did not parse as JSON · shown as text")
        ),
        "{drawn:#?}"
    );
    assert!(
        drawn.iter().any(|row| row.contains("{\"cut\":")),
        "the value itself is gone"
    );
    Ok(())
}

/// Dies with any kind panicking on an empty value, or drawing a tab or an escape raw.
#[test]
fn every_result_kind_survives_an_empty_and_a_control_ridden_value() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let mimes = [
        None,
        Some("text/markdown"),
        Some("application/json"),
        Some("text/x-python"),
    ];
    for mime in mimes {
        let empty = notebook_lines(&[kind_cell(mime, "")], 72, false, &theme, "⌘W");
        assert!(
            !empty
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.contains('…')),
            "{mime:?} drew a cut row for an empty value"
        );
        let cell = kind_cell(mime, "a\tb\x1b[2J\rc € {\"k\": ");
        let lines = notebook_lines(&[cell], 72, false, &theme, "⌘W");
        for span in lines.iter().flat_map(|line| line.spans.iter()) {
            assert_eq!(
                span.content.chars().find(|c: &char| c.is_control()),
                None,
                "{mime:?}: {:?}",
                span.content
            );
        }
    }
    Ok(())
}
