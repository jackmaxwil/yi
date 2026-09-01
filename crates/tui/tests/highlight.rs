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
    let Some(mut lang) = lang_for(lang) else {
        return Vec::new();
    };
    tokens(line, &mut lang)
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

/// Every spelling a fence carries in Yi's own transcripts must colour. The
/// bundled grammars answer to a file extension, so the spoken names — and
/// TypeScript, which has no grammar at all — render plain without an alias.
#[test]
fn the_language_names_fences_actually_carry_all_resolve() -> TestResult {
    for name in [
        "rs",
        "rust",
        "py",
        "python",
        "python3",
        "ipython",
        "sh",
        "bash",
        "zsh",
        "shell",
        "json",
        "js",
        "jsx",
        "javascript",
        "ts",
        "tsx",
        "typescript",
    ] {
        assert!(lang_for(name).is_some(), "{name} renders plain");
    }
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
    let mut lang = lang_for("rust").ok_or("rust missing")?;
    let base = Style::default().bg(ratatui::style::Color::Rgb(0x21, 0x3A, 0x2B));
    let rendered = spans("let x = 1;", &mut lang, &theme(), base);
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
    let mut lang = lang_for("json").ok_or("json missing")?;
    let long = format!("\"{}\"", "a".repeat(8_000));
    assert!(tokens(&long, &mut lang).is_empty());
    let rendered = spans(&long, &mut lang, &theme(), Style::default());
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
        call_id: String::new(),
        intent: None,
        status: ToolStatus::Done,
        summary: ToolCell::summary_of("bash", "cargo test --workspace"),
        digest: Some("running 12 tests".to_owned()),
        preview: Vec::new(),
        elapsed_ms: 1_200,
        calls: 1,
        details: json!({ "exitCode": 1 }),
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

/// A lifetime never closes its quote; before the audit `<'a>` opened a
/// "string" that swallowed the rest of the row, which on generic-heavy Rust
/// was most rows. A real char literal still colours.
#[test]
fn a_lifetime_is_not_a_string_but_a_char_literal_is() -> TestResult {
    let generic = kinds("fn f<'a, 'b>(x: &'a str) -> &'b str {", "rust");
    assert!(
        generic.iter().all(|(_, token)| *token != Token::Str),
        "{generic:?}"
    );
    let ch = kinds("let c = 'x'; let nl = '\\n';", "rust");
    assert!(ch.contains(&("'x'".to_owned(), Token::Str)), "{ch:?}");
    assert!(ch.contains(&("'\\n'".to_owned(), Token::Str)), "{ch:?}");
    Ok(())
}

/// A block comment that closes on its own line owns only its span; the code
/// after it used to be swallowed to end of line.
#[test]
fn a_closed_block_comment_releases_the_rest_of_the_line() -> TestResult {
    let found = kinds("let a = 1; /* note */ let b = 2;", "rust");
    assert!(
        found.contains(&("/* note */".to_owned(), Token::Comment)),
        "{found:?}"
    );
    assert_eq!(
        found
            .iter()
            .filter(|(text, token)| text == "let" && *token == Token::Keyword)
            .count(),
        2,
        "{found:?}"
    );
    Ok(())
}

/// In shell `x#y` is a literal word, and a comment that swallowed a URL's
/// fragment ate the rest of the row. Python keeps the anywhere rule. The
/// shell comment that opens off whitespace is the one the grammar answers.
#[test]
fn a_shell_hash_mid_word_is_not_a_comment() -> TestResult {
    let shell = kinds("curl http://host/a#frag # note", "sh");
    assert!(
        shell
            .iter()
            .all(|(text, token)| *token != Token::Comment || text.starts_with("# note")),
        "{shell:?}"
    );
    let punctuated = kinds("echo hi; #note", "sh");
    assert!(
        punctuated
            .iter()
            .any(|(text, token)| *token == Token::Comment && text.starts_with("#note")),
        "{punctuated:?}"
    );
    let python = kinds("x=1#tight comment", "python");
    assert!(
        python
            .iter()
            .any(|(text, token)| *token == Token::Comment && text.starts_with("#tight")),
        "{python:?}"
    );
    Ok(())
}

/// `Type` is claimed off an uppercase initial, which a SCREAMING_CASE constant
/// also has. Colouring `MAX_ROWS` as a type is a claim the scanner cannot make.
#[test]
fn an_all_caps_constant_is_not_a_type() -> TestResult {
    let mut lang = lang_for("rs").ok_or("no rust lang")?;
    let mut kinds = |line: &str| -> Vec<Token> {
        tokens(line, &mut lang)
            .into_iter()
            .map(|(_, _, kind)| kind)
            .collect()
    };
    assert!(
        !kinds("let n = MAX_ROWS;").contains(&Token::Type),
        "a constant is not a type"
    );
    assert!(
        kinds("let s: String = x;").contains(&Token::Type),
        "a mixed-case name still is"
    );
    Ok(())
}

/// The defect the hand-rolled scanner documented as "one mis-coloured row": a
/// block comment's body rendered as live code, and its closing delimiter as
/// nothing at all.
#[test]
fn a_block_comment_keeps_its_colour_past_the_row_that_opened_it() -> TestResult {
    let mut lang = lang_for("rust").ok_or("rust")?;
    let body = [
        "/* a block comment",
        "    let x = \"still comment\";",
        "    done */",
    ];
    for line in body {
        let found: Vec<Token> = tokens(line, &mut lang)
            .into_iter()
            .map(|(_, _, token)| token)
            .collect();
        assert!(
            found.iter().all(|token| *token == Token::Comment),
            "every row of a block comment is comment, got {found:?} for {line:?}"
        );
    }
    Ok(())
}

/// Python's triple quote is the multi-line construct Yi's own kernel cells are
/// written in, and the per-line scanner split it into a bogus empty string.
#[test]
fn a_triple_quoted_string_spans_its_rows() -> TestResult {
    let mut lang = lang_for("python").ok_or("python")?;
    let body = ["s = \"\"\"opening", "def not_a_def(): pass", "\"\"\""];
    let mut kinds = Vec::new();
    for line in body {
        kinds.push(
            tokens(line, &mut lang)
                .into_iter()
                .map(|(_, _, token)| token)
                .collect::<Vec<_>>(),
        );
    }
    let middle = kinds.get(1).ok_or("second row")?;
    assert!(
        middle.iter().all(|token| *token == Token::Str),
        "a docstring body is string, not code: {middle:?}"
    );
    Ok(())
}

/// Resuming a parse must equal parsing the body whole, or a scrolled transcript
/// and a fresh one disagree about the same text.
#[test]
fn resuming_a_parse_equals_parsing_it_whole() -> TestResult {
    let body = ["/* head", "mid", "*/ let a = 1;"];
    let mut split = lang_for("rust").ok_or("rust")?;
    let stepwise: Vec<_> = body.iter().map(|line| tokens(line, &mut split)).collect();
    let mut whole = lang_for("rust").ok_or("rust")?;
    let together: Vec<_> = body.iter().map(|line| tokens(line, &mut whole)).collect();
    assert_eq!(stepwise, together);
    Ok(())
}

fn tool_spans(name: &str, argument: &str, preview: &[&str]) -> Vec<(String, Style)> {
    let cell = ToolCell {
        name: name.to_owned(),
        call_id: String::new(),
        intent: None,
        status: ToolStatus::Done,
        summary: ToolCell::summary_of(name, argument),
        digest: None,
        preview: preview.iter().map(|row| (*row).to_owned()).collect(),
        elapsed_ms: 0,
        calls: 1,
        details: json!({}),
    };
    Cell::Tool(cell)
        .lines(100, &theme(), TranscriptMode::Verbose, 0)
        .iter()
        .flat_map(|line| line.spans.clone())
        .map(|span| (span.content.into_owned(), span.style))
        .collect()
}

fn carries(spans: &[(String, Style)], text: &str, token: Token) -> bool {
    let want = theme().syntax_style(token);
    spans
        .iter()
        .any(|(content, style)| content == text && style.fg == want.fg)
}

/// A read names the file it read, so its rows can carry the same colour the
/// diff of that same file already does.
#[test]
fn a_read_body_is_highlighted_from_its_own_path() -> TestResult {
    let spans = tool_spans("read", "src/lib.rs", &["1:let total = 1;"]);
    assert!(carries(&spans, "let", Token::Keyword), "{spans:?}");
    Ok(())
}

/// A grep hit is one line lifted out of its file, possibly from inside a
/// string or a comment; colouring it from a guess is worse than leaving it dim.
#[test]
fn a_grep_hit_is_left_plain() -> TestResult {
    let spans = tool_spans("grep", "total", &["src/lib.rs:1:let total = 1;"]);
    assert!(!carries(&spans, "let", Token::Keyword), "{spans:?}");
    Ok(())
}

fn plain(spans: &[(String, Style)], text: &str) -> bool {
    let base = Style::default().fg(theme().text);
    spans
        .iter()
        .any(|(content, style)| content == text && *style == base)
}

/// A read with `ranges` prints two windows and says what it skipped between
/// them. Carrying the parse over the gap paints real code as comment, and
/// restarting it guesses at a scope the reader never saw open.
#[test]
fn a_skipped_range_stops_the_highlighting() -> TestResult {
    let spans = tool_spans(
        "read",
        "src/lib.rs",
        &[
            "1:/* opened here",
            "[lines 2-9 not shown]",
            "10:let total = 1;",
        ],
    );
    assert!(plain(&spans, "let total = 1;"), "{spans:?}");
    Ok(())
}

/// A long read reaches the transcript as head and tail with a marker between,
/// so the rows under it are not the rows the parse just walked.
#[test]
fn an_elided_tail_is_left_plain() -> TestResult {
    let spans = tool_spans(
        "read",
        "report.py",
        &[
            "1:import sys",
            "2:HELP = \"\"\"",
            "\u{2026} 7 more lines",
            "12:for x in y",
        ],
    );
    assert!(carries(&spans, "import", Token::Keyword), "{spans:?}");
    assert!(plain(&spans, "for x in y"), "{spans:?}");
    Ok(())
}

/// The read tool clips an over-wide row, so a quote that closed on that line
/// never reached the parse the rows below it would resume from.
#[test]
fn a_clipped_row_stops_the_highlighting() -> TestResult {
    let wide = format!("1:const B: &str = \"{}\u{2026}", "a".repeat(512));
    let spans = tool_spans("read", "src/lib.rs", &[&wide, "2:let total = 1;"]);
    assert!(plain(&spans, "let total = 1;"), "{spans:?}");
    Ok(())
}
