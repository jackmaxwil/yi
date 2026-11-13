use std::error::Error;

use ratatui::style::{Color, Modifier, Style};
use serde_json::{Value, json};
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Over, Theme};
use yi_tui::highlight::Token;
use yi_tui::result_paint::{sniff, text_spans};
use yi_tui::transcript::show_controls;

type TestResult = Result<(), Box<dyn Error>>;
type Spans = Vec<(String, Style)>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn card(name: &str, argument: &str, preview: &[&str], status: ToolStatus, details: Value) -> Spans {
    let cell = ToolCell {
        name: name.to_owned(),
        call_id: String::new(),
        status,
        summary: ToolCell::summary_of(name, argument),
        digest: None,
        preview: preview.iter().map(|row| show_controls(row)).collect(),
        elapsed_ms: 0,
        calls: 1,
        details,
    };
    Cell::Tool(cell)
        .lines(100, &theme(), TranscriptMode::Verbose, 0)
        .iter()
        .flat_map(|line| line.spans.clone())
        .map(|span| (span.content.into_owned(), span.style))
        .collect()
}

fn done(name: &str, argument: &str, preview: &[&str]) -> Spans {
    card(name, argument, preview, ToolStatus::Done, json!({}))
}

fn carries(spans: &[(String, Style)], text: &str, token: Token) -> bool {
    let want = theme().syntax_style(token);
    spans
        .iter()
        .any(|(content, style)| content.trim() == text && style.fg == want.fg)
}

fn pathed(spans: &[(String, Style)], text: &str) -> bool {
    let want = theme().pick(Over::StatusPath, theme().accent);
    spans
        .iter()
        .any(|(content, style)| content == text && style.fg == Some(want))
}

fn shows_escape(spans: &[(String, Style)]) -> bool {
    spans
        .iter()
        .any(|(content, _)| content.contains('\u{241b}') || content.contains('\u{1b}'))
}

#[test]
fn painter_marks_a_path_and_its_line_in_prose() {
    let spans: Spans = text_spans(
        "  --> crates/tui/src/cell.rs:551:9 failed.",
        &theme(),
        Style::default(),
    )
    .into_iter()
    .map(|span| (span.content.into_owned(), span.style))
    .collect();
    assert!(pathed(&spans, "crates/tui/src/cell.rs:551:9"), "{spans:?}");
}

#[test]
fn painter_leaves_urls_versions_and_slashed_words_unmarked() {
    let row = "see https://example.com/search?q=docs/guide.rs&n=2, api.example.com:443, v1.2.3, and/or 3/4 or e.g. this";
    let spans = text_spans(row, &theme(), Style::default());
    assert_eq!(spans.len(), 1, "{spans:?}");
}

#[test]
fn painter_types_untyped_output_by_its_content() {
    let rows = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
    assert!(sniff(&rows("{\"a\": 1}")).is_some());
    assert!(sniff(&rows("diff --git a/x b/x\n@@ -1 +1 @@")).is_some());
    assert!(sniff(&rows("#!/usr/bin/env python3\nprint(1)")).is_some());
    assert!(sniff(&rows("[1/3] Compiling yi-tui")).is_none());
    assert!(sniff(&rows("total 8\ndrwxr-xr-x  2 me  staff")).is_none());
    assert!(sniff(&rows("[[package]]\nname = \"yi\"")).is_none());
}

#[test]
fn malformed_back_to_back_escapes_and_junk_params_keep_the_colour() {
    let spans = done(
        "bash",
        "x",
        &["\u{1b}\u{1b}[31mred\u{1b}[0m \u{1b}(Bplain \u{1b}[31;999mstill"],
    );
    assert!(!shows_escape(&spans), "{spans:?}");
    let red = |text: &str| {
        spans
            .iter()
            .any(|(content, style)| content == text && style.fg == Some(Color::Indexed(1)))
    };
    assert!(red("red"), "{spans:?}");
    assert!(red("still"), "{spans:?}");
    assert!(
        !spans.iter().any(|(content, _)| content.contains("[31m")),
        "{spans:?}"
    );
}

#[test]
fn search_notice_rows_are_not_parsed_as_the_file() {
    let spans = done(
        "grep",
        "fn",
        &[
            "[src/lib.rs#AB12]",
            "1:fn main() {}",
            "[cap reached: 1 of 9 hits; let the next page run]",
        ],
    );
    assert!(!carries(&spans, "let", Token::Keyword), "{spans:?}");
}

#[test]
fn read_from_the_middle_of_a_file_is_coloured() {
    let spans = done(
        "read",
        "src/lib.rs",
        &["40:    let total = palette.count();"],
    );
    assert!(carries(&spans, "let", Token::Keyword), "{spans:?}");
}

#[test]
fn read_with_no_extension_is_typed_by_its_shebang() {
    let spans = done(
        "read",
        "scripts/run",
        &["1:#!/usr/bin/env python3", "2:import sys"],
    );
    assert!(carries(&spans, "import", Token::Keyword), "{spans:?}");
}

#[test]
fn read_streaming_write_is_coloured_by_its_path() {
    let spans = card(
        "write",
        "src/main.rs",
        &["fn main() {"],
        ToolStatus::Running,
        Value::Null,
    );
    assert!(carries(&spans, "fn", Token::Keyword), "{spans:?}");
}

#[test]
fn read_finished_write_report_is_not_parsed_as_its_file() {
    let spans = done("write", "src/main.rs", &["Wrote 12 bytes to src/main.rs"]);
    assert!(!carries(&spans, "12", Token::Number), "{spans:?}");
    assert!(pathed(&spans, "src/main.rs"), "{spans:?}");
}

#[test]
fn search_hits_and_context_are_coloured_under_their_file() {
    let spans = done(
        "grep",
        "fn ",
        &[
            "[src/paint.rs#AB12]",
            "19-impl Palette {",
            "20:    pub fn new() -> Self {",
            "--",
            "44:    fn fmt(&self)",
        ],
    );
    assert!(pathed(&spans, "src/paint.rs"), "{spans:?}");
    assert!(carries(&spans, "impl", Token::Keyword), "{spans:?}");
    assert!(carries(&spans, "pub", Token::Keyword), "{spans:?}");
    assert!(carries(&spans, "fn", Token::Keyword), "{spans:?}");
    assert!(
        !spans.iter().any(|(content, _)| content.contains("AB12")),
        "{spans:?}"
    );
}

#[test]
fn shell_colour_codes_render_as_colour() {
    let spans = done(
        "bash",
        "cargo build",
        &[
            "\u{1b}[1;31merror\u{1b}[0m: unused variable \u{1b}[1m`total`\u{1b}[0m",
            "  \u{1b}[34m-->\u{1b}[0m src/main.rs:55:9",
            "\u{1b}]8;;https://x\u{7}link\u{1b}]8;;\u{7} \u{1b}[38;2;1;2;3mrgb\u{1b}[K",
        ],
    );
    assert!(!shows_escape(&spans), "{spans:?}");
    let error = spans.iter().find(|(content, _)| content == "error");
    assert!(
        error.is_some_and(|(_, style)| style.fg == Some(Color::Indexed(1))
            && style.add_modifier.contains(Modifier::BOLD)),
        "{spans:?}"
    );
    assert!(pathed(&spans, "src/main.rs:55:9"), "{spans:?}");
    assert!(
        spans
            .iter()
            .any(|(content, style)| content == "rgb" && style.fg == Some(Color::Rgb(1, 2, 3))),
        "{spans:?}"
    );
}

#[test]
fn shell_json_output_is_coloured() {
    let spans = done("bash", "curl", &["{\"name\": \"yi\", \"count\": 3}"]);
    assert!(carries(&spans, "3", Token::Number), "{spans:?}");
}

#[test]
fn shell_digest_drops_colour_codes() {
    let digest = ToolCell::digest_of("bash", "\u{1b}[32mok\u{1b}[0m 3 passed", &json!({}), false);
    assert_eq!(digest.as_deref(), Some("ok 3 passed"));
}

#[test]
fn malformed_json_cut_mid_string_still_colours_what_parsed() {
    let spans = done(
        "bash",
        "curl",
        &["{\"name\": \"yi\", \"items\": [1, 2.5, \"thr"],
    );
    assert!(carries(&spans, "2.5", Token::Number), "{spans:?}");
}

#[test]
fn malformed_escapes_cut_or_stray_are_dropped() {
    let spans = done(
        "bash",
        "x",
        &["ok \u{1b}", "half \u{1b}[1;3", "bell \u{7} kept"],
    );
    assert!(!shows_escape(&spans), "{spans:?}");
    assert!(
        spans.iter().any(|(content, _)| content == "ok"),
        "{spans:?}"
    );
    assert!(
        spans
            .iter()
            .any(|(content, _)| content.contains('\u{2407}')),
        "{spans:?}"
    );
}

#[test]
fn malformed_non_ascii_path_is_marked_without_a_panic() {
    let spans = done("mcp__docs__search", "q", &["voir café/ñandú.rs:3 ✓ — fini"]);
    assert!(pathed(&spans, "café/ñandú.rs:3"), "{spans:?}");
}

#[test]
fn malformed_parse_error_restarts_on_the_next_row() -> TestResult {
    let wide = format!("1:let s = \"{}\u{2026}", "a".repeat(5_000));
    let spans = done("read", "src/lib.rs", &[&wide, "2:let total = 1;"]);
    assert!(carries(&spans, "let", Token::Keyword), "{spans:?}");
    Ok(())
}

#[test]
fn notebook_output_colours_codes_and_json() {
    let details = json!({
        "code": "print(r)",
        "stdout": "\u{1b}[31mFAILED\u{1b}[0m tests/test_a.py:12\n",
        "result": "{\"rows\": 4}",
    });
    let spans = card("ipython", "print(r)", &[], ToolStatus::Done, details);
    assert!(!shows_escape(&spans), "{spans:?}");
    assert!(
        spans
            .iter()
            .any(|(content, style)| content == "FAILED" && style.fg == Some(Color::Indexed(1))),
        "{spans:?}"
    );
    assert!(pathed(&spans, "tests/test_a.py:12"), "{spans:?}");
    assert!(carries(&spans, "4", Token::Number), "{spans:?}");
}
