use std::error::Error;

use ratatui::style::Style;
use serde_json::json;
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::highlight::{Token, lang_for, spans, tokens};

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn kinds(line: &str, lang: &str) -> Vec<(String, Token)> {
    let Some(lang) = lang_for(lang) else {
        return Vec::new();
    };
    tokens(line, lang)
        .into_iter()
        .filter_map(|(start, end, token)| line.get(start..end).map(|text| (text.to_owned(), token)))
        .collect()
}

/// The four kinds a reader actually uses to skim code. Getting a keyword and a
/// string confused is worse than no colour at all.
#[test]
fn a_line_of_rust_splits_into_the_kinds_that_matter() -> TestResult {
    let found = kinds("    let total = compute(42, \"tail\");", "rust");
    assert!(
        found.contains(&("let".to_owned(), Token::Keyword)),
        "{found:?}"
    );
    assert!(
        found.contains(&("compute".to_owned(), Token::Function)),
        "{found:?}"
    );
    assert!(
        found.contains(&("42".to_owned(), Token::Number)),
        "{found:?}"
    );
    assert!(
        found.contains(&("\"tail\"".to_owned(), Token::Str)),
        "{found:?}"
    );
    Ok(())
}

/// A `#` inside a string is not a comment, and a comment swallowing the rest of
/// a line it does not own is the most visible way a lexer can be wrong.
#[test]
fn a_comment_and_a_string_do_not_swallow_each_other() -> TestResult {
    let python = kinds("path = \"a#b\"  # trailing note", "python");
    assert!(
        python.contains(&("\"a#b\"".to_owned(), Token::Str)),
        "{python:?}"
    );
    assert!(
        python
            .iter()
            .any(|(text, token)| *token == Token::Comment && text.starts_with("# trailing")),
        "{python:?}"
    );

    // An escaped quote does not end the string.
    let rust = kinds("let s = \"a\\\"b\";", "rust");
    assert!(
        rust.contains(&("\"a\\\"b\"".to_owned(), Token::Str)),
        "{rust:?}"
    );
    Ok(())
}

/// An unknown fence language must render, not vanish or panic.
#[test]
fn an_unknown_language_is_declined_rather_than_guessed() -> TestResult {
    assert!(lang_for("brainfuck").is_none());
    assert!(lang_for("").is_none());
    assert!(lang_for("crates/tui/src/cell.rs").is_some());
    Ok(())
}

/// The caller owns the row: a diff tint is a background, and a highlighter that
/// resets it would punch holes in the tint on every token.
#[test]
fn highlighting_keeps_the_background_the_caller_set() -> TestResult {
    let lang = lang_for("rust").ok_or("rust missing")?;
    let base = Style::default().bg(ratatui::style::Color::Rgb(0x21, 0x3A, 0x2B));
    let rendered = spans("let x = 1;", lang, &theme(), base);
    assert!(rendered.len() > 1, "the line was tokenised");
    assert!(
        rendered.iter().all(|span| span.style.bg == base.bg),
        "{rendered:?}"
    );
    Ok(())
}

/// A generated line is not a line to read, and colouring it costs more than it
/// is worth.
#[test]
fn an_absurdly_long_line_is_left_plain() -> TestResult {
    let lang = lang_for("json").ok_or("json missing")?;
    let long = format!("\"{}\"", "a".repeat(8_000));
    assert!(tokens(&long, lang).is_empty());
    let rendered = spans(&long, lang, &theme(), Style::default());
    assert_eq!(rendered.len(), 1);
    Ok(())
}

/// The patch names its own file, so a diff body needs no language parameter —
/// and without one it would render as plain text inside its own tint.
#[test]
fn a_diff_body_is_highlighted_from_the_path_in_its_header() -> TestResult {
    let patch = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-let a = 1;\n+let a = 2;\n";
    let lines = yi_tui::diffview::render(patch, 80, &theme(), yi_tui::diffview::DiffBudget::FULL);
    let keyword = theme().syntax_style(Token::Keyword);
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.content.as_ref() == "let" && span.style.fg == keyword.fg),
        "the diff body carries syntax colour"
    );
    Ok(())
}

/// A shell call names itself; `$ bash cargo test` says the word "bash" where
/// the command should be, and hides a nonzero exit entirely.
#[test]
fn a_bash_cell_shows_the_command_and_its_exit() -> TestResult {
    let cell = ToolCell {
        name: "bash".to_owned(),
        status: ToolStatus::Done,
        summary: ToolCell::summary_of("bash", "cargo test --workspace"),
        digest: Some("running 12 tests".to_owned()),
        elapsed_ms: 1_200,
        details: json!({ "exitCode": 1 }),
        ..ToolCell::default()
    };
    let rendered: Vec<String> = Cell::Tool(cell)
        .lines(100, &theme(), TranscriptMode::Normal, 0)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect();
    let joined = rendered.join("\n");
    assert!(joined.contains("$ cargo test --workspace"), "{joined}");
    assert!(!joined.contains("bash cargo"), "{joined}");
    assert!(joined.contains("· exit 1"), "{joined}");
    assert!(joined.contains("· 1s"), "{joined}");
    Ok(())
}
