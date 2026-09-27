use std::error::Error;

use ratatui::style::Style;
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;
use yi_tui::cell::{Cell, TranscriptMode, thought_lines};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::markdown::render;

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn flat(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn rows(source: &str, width: usize) -> Vec<String> {
    render(source, width, &theme()).iter().map(flat).collect()
}

fn widest(rows: &[String]) -> usize {
    rows.iter().map(|row| row.width()).max().unwrap_or(0)
}

/// Inline HTML is text a model wrote (`Vec<String>` unfenced); dropping it changed the sentence.
#[test]
fn html_renders_literally_and_a_br_breaks_the_line() -> TestResult {
    assert_eq!(
        rows("It returns Vec<String> or Option<T> here.", 60),
        ["It returns Vec<String> or Option<T> here."]
    );
    assert_eq!(
        rows("one<br>two and a <kbd>Ctrl</kbd>+C", 60),
        ["one", "two and a <kbd>Ctrl</kbd>+C"]
    );
    assert_eq!(
        rows("<details>\n<summary>More</summary>\n\nBody\n</details>", 60),
        [
            "<details>",
            "<summary>More</summary>",
            "",
            "Body",
            "",
            "</details>"
        ]
    );
    let table = rows("| a |\n|---|\n| x<br>y |", 60);
    assert!(
        table.iter().any(|row| row.starts_with("│ x"))
            && table.iter().any(|row| row.starts_with("│ y"))
            && !table.iter().any(|row| row.contains("<br>")),
        "{table:?}"
    );
    Ok(())
}

#[test]
fn every_line_in_a_list_item_hangs_under_its_text() -> TestResult {
    let loose = rows(
        "- first para of item one that wraps around\n\n  second para of item one\n- item two",
        30,
    );
    assert!(
        loose.iter().any(|row| row == "  second para of item one"),
        "{loose:?}"
    );
    assert_eq!(
        rows("- line one  \n  line two", 30),
        ["‣ line one", "  line two"]
    );
    let after = rows("- a\n  - b\n\n  more of a", 30);
    assert!(after.iter().any(|row| row == "  more of a"), "{after:?}");
    let code = rows("1. step\n\n   ```sh\n   cargo build\n   ```\n2. next", 30);
    assert!(
        code.iter().any(|row| row == "   │ sh")
            && code.iter().any(|row| row == "   │ cargo build")
            && code.iter().any(|row| row == "2. next"),
        "{code:?}"
    );
    let table = rows("- item\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |", 30);
    let boxed: Vec<&String> = table
        .iter()
        .filter(|row| row.contains(['┌', '│', '├', '└']))
        .collect();
    assert!(
        boxed.len() == 5
            && boxed
                .iter()
                .all(|row| row.starts_with("  ") && !row.starts_with("   ")),
        "{table:?}"
    );
    Ok(())
}

/// The ` (dest)` suffix went to the line under construction, not the cell, so it printed
/// as a stray row under the table.
#[test]
fn a_link_in_a_table_keeps_its_url_in_its_cell() -> TestResult {
    let table = rows(
        "| a | b |\n|---|---|\n| [x](https://e.com) | y |\n\nAfter.",
        60,
    );
    assert!(
        table.iter().any(|row| row.starts_with('│')
            && row.contains("x (https://e.com)")
            && row.contains('y')),
        "{table:?}"
    );
    assert_eq!(
        table
            .iter()
            .filter(|row| row.contains("https://e.com"))
            .count(),
        1,
        "{table:?}"
    );
    assert_eq!(table.last().map(String::as_str), Some("After."));
    Ok(())
}

/// Ratatui drops control characters, so a tab vanished with the indentation it carried. Code
/// keeps four-column stops; prose, whose text can start anywhere on a row once a stream cuts
/// it, takes one fixed width.
#[test]
fn tabs_expand_to_stops_in_code_and_a_fixed_width_in_prose() -> TestResult {
    let code = rows("```go\nfunc f() {\n\treturn 1\n}\n```", 60);
    assert!(code.iter().any(|row| row == "│     return 1"), "{code:?}");
    assert_eq!(rows("a\tb", 60), ["a    b"]);
    assert_eq!(rows("abc\td", 60), ["abc    d"]);
    Ok(())
}

#[test]
fn code_lines_wrap_at_the_width_under_their_rail() -> TestResult {
    let line = "let x = some_function_with_a_long_name(argument_one, argument_two);";
    let code = rows(&format!("```\n{line}\n```"), 30);
    assert!(widest(&code) <= 30, "{code:?}");
    let body: String = code
        .iter()
        .skip(1)
        .map(|row| row.strip_prefix("│ ").ok_or(format!("no rail: {row:?}")))
        .collect::<Result<Vec<_>, _>>()?
        .concat();
    assert_eq!(body, line, "hard-wrapped by columns, nothing dropped");
    let quoted = rows(&format!("> ```\n> {line}\n> ```"), 20);
    assert!(widest(&quoted) <= 20, "{quoted:?}");
    assert!(
        quoted.iter().skip(1).all(|row| row.starts_with("▌ │ ")),
        "{quoted:?}"
    );
    // The split keeps each character's highlight: the wide paint, cut into rows, is the narrow one.
    let theme = theme();
    let cells = |lines: Vec<Line<'static>>| -> Vec<(char, Style)> {
        lines
            .iter()
            .skip(1)
            .flat_map(|line| {
                line.spans
                    .iter()
                    .flat_map(|s| s.content.chars().map(move |ch| (ch, s.style)))
                    .skip("│ ".chars().count())
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let source = format!("```rust\n{line}\n```");
    let wide = cells(render(&source, 200, &theme));
    let narrow = cells(render(&source, 30, &theme));
    assert_eq!(wide, narrow);
    Ok(())
}

/// The live tail of a fence has no newline yet; it word-wrapped, then snapped to one long
/// row when the newline arrived.
#[test]
fn a_streaming_code_line_renders_as_it_will_once_complete() -> TestResult {
    let partial = "```\nlet value = compute_something_really_long(arg);";
    assert_eq!(rows(partial, 30), rows(&format!("{partial}\n"), 30));
    assert_eq!(rows(partial, 30), rows(&format!("{partial}\n```\n"), 30));
    Ok(())
}

/// pulldown-cmark emits CRLF code as `a`, `\nb`, `\n`: a rail per text event doubled it.
#[test]
fn crlf_code_draws_one_rail_per_row() -> TestResult {
    assert_eq!(rows("```\na\r\nb\r\n```\r\n", 40), ["│", "│ a", "│ b"]);
    Ok(())
}

#[test]
fn a_bare_link_prints_its_url_once() -> TestResult {
    assert_eq!(
        rows("See <https://example.com/a> now.", 60),
        ["See https://example.com/a now."]
    );
    assert_eq!(rows("Mail <foo@bar.com>.", 60), ["Mail foo@bar.com."]);
    assert_eq!(
        rows("[foo@bar.com](mailto:foo@bar.com)", 60),
        ["foo@bar.com"]
    );
    assert_eq!(
        rows("[docs](https://docs.rs)", 60),
        ["docs (https://docs.rs)"],
        "a label that is not its URL still shows where it goes"
    );
    Ok(())
}

#[test]
fn an_indented_h1_rule_fits_the_width() -> TestResult {
    let quoted = rows("> # Title", 20);
    assert!(widest(&quoted) <= 20, "{quoted:?}");
    assert!(
        quoted
            .iter()
            .any(|row| *row == format!("▌ {}", "━".repeat(18))),
        "{quoted:?}"
    );
    Ok(())
}

/// Ratatui clips a row at the buffer width, so an unsplit URL lost its tail anyway, and in
/// a cell it pushed the right border out of line.
#[test]
fn a_long_token_never_exceeds_the_width() -> TestResult {
    let table = rows(
        "| k | url |\n|---|---|\n| a | https://example.com/a/very/long/path/to/thing |",
        30,
    );
    assert!(
        table.iter().all(|row| row.width() == 30),
        "every table row is as wide as its border: {table:?}"
    );
    let url = "https://example.com/aaaaaaaaaaaaaaaaaaaaaaaa/bbbbbbbbb";
    let prose = rows(&format!("Visit {url} now"), 20);
    assert!(widest(&prose) <= 20, "{prose:?}");
    assert!(prose.concat().contains(url), "{prose:?}");
    Ok(())
}

#[test]
fn widths_count_columns_not_bytes() -> TestResult {
    let theme = theme();
    let prose = Cell::Assistant {
        markdown: "abcdefghij abcdefghij abcdef next".to_owned(),
    }
    .lines(30, &theme, TranscriptMode::Thinking, 0);
    let prose: Vec<String> = prose.iter().map(flat).collect();
    assert!(
        prose
            .iter()
            .any(|row| row == "• abcdefghij abcdefghij abcdef"),
        "the two-column gutter costs two columns: {prose:?}"
    );
    let table = rows("> | aaaaa | bbbbbb |\n> |---|---|\n> | x | y |", 20);
    assert!(
        table.iter().any(|row| row == "▌ │ aaaaa │ bbbbbb │"),
        "the quote rail costs two columns: {table:?}"
    );
    Ok(())
}

/// Every reasoning row was trimmed flush, a leftover from a renderer that indented all
/// output by two columns; nesting and hanging indents went with it.
#[test]
fn a_thought_keeps_list_nesting_and_hanging_indents() -> TestResult {
    let lines = thought_lines(
        "- a\n  - b\n    - c deep item that wraps around the width",
        30,
        &theme(),
        TranscriptMode::Thinking,
        false,
    );
    let text: Vec<String> = lines.iter().map(flat).collect();
    let deep = text
        .iter()
        .position(|row| row.contains("▪ c"))
        .ok_or(format!("{text:?}"))?;
    assert_eq!(text.first().map(String::as_str), Some("  ‣ a"));
    assert_eq!(text.get(1).map(String::as_str), Some("    ◦ b"));
    assert!(
        text.get(deep)
            .is_some_and(|row| row.starts_with("      ▪ c")),
        "{text:?}"
    );
    assert!(
        text.get(deep + 1)
            .is_some_and(|row| row.starts_with("        ") && !row.starts_with("         ")),
        "{text:?}"
    );
    Ok(())
}

/// Reasoning restyles every span dim italic, so the syntax colour it paid for each frame
/// (47 ms for a 300-line fence) was thrown away: it renders plain and reads the same.
#[test]
fn a_thought_renders_code_plain_and_reads_the_same() -> TestResult {
    let theme = theme();
    let source = "Plan:\n\n```rust\nfn main() {\n    let x = 1;\n}\n```\n";
    let coloured = |lines: &[Line<'_>]| {
        lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.fg == Some(theme.magenta))
    };
    assert!(
        coloured(&render(source, 58, &theme)),
        "the prose render highlights"
    );
    let plain = yi_tui::markdown::render_plain(source, 58, &theme);
    assert!(!coloured(&plain), "{plain:?}");
    let expected: Vec<String> = render(source, 58, &theme)
        .iter()
        .map(|line| format!("  {}", flat(line)).trim_end().to_owned())
        .collect();
    let thought: Vec<String> = thought_lines(source, 60, &theme, TranscriptMode::Thinking, false)
        .iter()
        .map(flat)
        .collect();
    assert_eq!(thought, expected);
    Ok(())
}
