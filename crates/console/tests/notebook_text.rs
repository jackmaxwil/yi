//! The console notebook draws a cell's plain output through the transcript's painter, so a
//! colour code, a JSON value and a file path look the same in both apps.

use ratatui::style::Color;
use yi_console::model::NbCell;
use yi_console::panes::notebook_lines;
use yi_tui::colors::{ColorTier, Over, Theme};
use yi_tui::highlight::Token;

#[test]
fn notebook_text_colours_codes_json_and_paths() {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let cell = NbCell {
        number: 1,
        code: "run()".to_owned(),
        stdout: "\u{1b}[31mFAILED\u{1b}[0m tests/test_a.py:12\n".to_owned(),
        result: Some("{\"rows\": 4}".to_owned()),
        result_mime: Some("text/plain".to_owned()),
        ..NbCell::default()
    };
    let spans: Vec<_> = notebook_lines(&[cell], 100, true, &theme)
        .into_iter()
        .flat_map(|line| line.spans)
        .map(|span| (span.content.into_owned(), span.style))
        .collect();
    assert!(
        !spans.iter().any(|(text, _)| text.contains('\u{241b}')),
        "{spans:?}"
    );
    assert!(
        spans
            .iter()
            .any(|(text, style)| text == "FAILED" && style.fg == Some(Color::Indexed(1))),
        "{spans:?}"
    );
    let path = theme.pick(Over::StatusPath, theme.accent);
    assert!(
        spans
            .iter()
            .any(|(text, style)| text == "tests/test_a.py:12" && style.fg == Some(path)),
        "{spans:?}"
    );
    let number = theme.syntax_style(Token::Number).fg;
    assert!(
        spans
            .iter()
            .any(|(text, style)| text == "4" && style.fg == number),
        "{spans:?}"
    );
}
