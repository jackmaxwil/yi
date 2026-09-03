//! The editor pane's lexer is primed from the top of the file, so a construct that opens
//! above the viewport still colours the rows inside it.

use std::error::Error;

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
