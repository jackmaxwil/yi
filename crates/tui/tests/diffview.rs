use std::error::Error;

use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use serde_json::json;
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::diffview::{DiffBudget, render};

type TestResult = Result<(), Box<dyn Error>>;

const REPLACEMENT: &str = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n one\n-let total = a + b;\n+let total = a - b;\n three\n";

fn text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn styles(lines: &[Line<'static>], row: usize) -> Vec<Style> {
    lines
        .get(row)
        .map(|line| line.spans.iter().map(|span| span.style).collect())
        .unwrap_or_default()
}

/// A consumer reads the columns: the sign tells polarity and the number tells
/// where to look. Losing either turns a diff into two anonymous lists.
#[test]
fn a_row_carries_its_sign_and_the_line_it_belongs_to() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let rendered = text(&render(REPLACEMENT, 80, &theme, DiffBudget::FULL));
    assert_eq!(
        rendered,
        vec![
            "       1 │ one",
            "    -  2 │ let total = a + b;",
            "    +    │ let total = a - b;",
            "       3 │ three",
        ]
    );
    Ok(())
}

/// Incident: derived from the widest number, the gutter would widen at line 100
/// and re-pad rows already written to native scrollback, which cannot be
/// rewritten. Three digits is the floor for every diff.
#[test]
fn the_gutter_does_not_widen_when_a_diff_crosses_line_100() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let early = "--- a/x\n+++ b/x\n@@ -1,1 +1,1 @@\n-a\n+b\n";
    let late = "--- a/x\n+++ b/x\n@@ -99,3 +99,3 @@\n ctx\n-a\n+b\n";
    let prefix = |patch: &str| -> Option<usize> {
        let rendered = text(&render(patch, 80, &theme, DiffBudget::FULL));
        rendered.first().and_then(|row| row.find('│'))
    };
    assert_eq!(prefix(early), prefix(late));
    Ok(())
}

/// A hunk boundary is a jump in the file, not a blank line; without a marker a
/// reader stitches two unrelated regions into one.
#[test]
fn hunks_are_separated_and_a_repeated_number_is_blanked() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let patch = "--- a/x\n+++ b/x\n@@ -1,1 +1,1 @@\n-a\n+b\n@@ -9,1 +9,1 @@\n-c\n+d\n";
    let rendered = text(&render(patch, 80, &theme, DiffBudget::FULL));
    assert!(rendered.iter().any(|row| row.contains('⋮')), "{rendered:?}");
    // The `+` row of a one-for-one replacement repeats the `-` row's number.
    assert_eq!(rendered.get(1).map(String::as_str), Some("    +    │ b"));
    Ok(())
}

/// Word highlight: on a one-for-one replacement the eye should land on the
/// token that moved, not re-read the whole line.
#[test]
fn a_one_for_one_replacement_marks_only_the_changed_tokens() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let lines = render(REPLACEMENT, 80, &theme, DiffBudget::FULL);
    let added = styles(&lines, 2);
    let marked: Vec<usize> = added
        .iter()
        .enumerate()
        .filter(|(_, style)| style.add_modifier.contains(Modifier::REVERSED))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(marked.len(), 1, "exactly one span is emphasised: {added:?}");

    let content: Vec<String> = lines
        .get(2)
        .map(|line| {
            line.spans
                .iter()
                .filter(|span| span.style.add_modifier.contains(Modifier::REVERSED))
                .map(|span| span.content.as_ref().to_owned())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(content, vec!["- ".to_owned()]);

    // A rewrite is not a replacement: two-for-two marks nothing.
    let rewrite = "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-a\n-b\n+c\n+d\n";
    let plain = render(rewrite, 80, &theme, DiffBudget::FULL);
    assert!(
        !plain
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| { span.style.add_modifier.contains(Modifier::REVERSED) }),
        "a multi-line rewrite marks nothing"
    );
    Ok(())
}

/// The indentation is the one thing a re-indent-free edit did not change;
/// marking it every time makes the highlight meaningless.
#[test]
fn indentation_is_never_emphasised() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let patch = "--- a/x\n+++ b/x\n@@ -1,1 +1,1 @@\n-        let a = 1;\n+        let a = 2;\n";
    let lines = render(patch, 80, &theme, DiffBudget::FULL);
    let first = lines
        .get(1)
        .and_then(|line| {
            line.spans
                .iter()
                .find(|span| span.style.add_modifier.contains(Modifier::REVERSED))
        })
        .ok_or("no emphasis on a one-for-one replacement")?;
    assert!(!first.content.starts_with(' '), "{:?}", first.content);
    Ok(())
}

/// A refactor touching thirty files must not bury the answer that follows it.
#[test]
fn the_normal_budget_drops_hunks_and_says_how_many() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let mut patch = String::from("--- a/x\n+++ b/x\n");
    for n in 0..20 {
        patch.push_str(&format!(
            "@@ -{0},3 +{0},3 @@\n ctx\n-a{1}\n+b{1}\n ctx\n",
            n * 10 + 1,
            n
        ));
    }
    let full = render(&patch, 80, &theme, DiffBudget::FULL);
    let cut = render(&patch, 80, &theme, DiffBudget::NORMAL);
    assert!(cut.len() < full.len());

    let rendered = text(&cut);
    let footer = rendered.last().ok_or("no rows rendered")?;
    assert!(footer.starts_with("    … 12 more hunks, "), "{footer}");
    // Change rows outrank context: every kept hunk still shows both of its.
    let changes = rendered
        .iter()
        .filter(|row| {
            let row = row.trim_start();
            row.starts_with('-') || row.starts_with('+')
        })
        .count();
    assert_eq!(changes, 16);
    Ok(())
}

/// The tint is the whole polarity signal at truecolor and cannot be there at 16
/// colours, where the background belongs to the terminal.
#[test]
fn the_palette_forks_on_the_colour_tier() -> TestResult {
    let added_bg = |tier, dark| {
        render(REPLACEMENT, 80, &Theme::new(tier, dark), DiffBudget::FULL)
            .get(2)
            .and_then(|line| line.spans.last().map(|span| span.style.bg))
            .flatten()
    };
    assert!(added_bg(ColorTier::TrueColor, true).is_some());
    assert!(added_bg(ColorTier::Ansi256, true).is_some());
    assert_ne!(
        added_bg(ColorTier::TrueColor, true),
        added_bg(ColorTier::TrueColor, false)
    );
    assert_eq!(added_bg(ColorTier::Ansi16, true), None);
    Ok(())
}

/// The headline of the phase: an edit shows its diff without the reader having
/// to know a mode key exists.
#[test]
fn an_edit_cell_renders_its_diff_in_normal_mode() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let cell = ToolCell {
        name: "edit".to_owned(),
        call_id: String::new(),
        intent: None,
        status: ToolStatus::Done,
        summary: ToolCell::summary_of("edit", "src/lib.rs"),
        digest: Some("updated; first change at line 2".to_owned()),
        preview: Vec::new(),
        elapsed_ms: 0,
        calls: 1,
        details: json!({ "patch": REPLACEMENT, "added": 1, "removed": 1 }),
    };
    let rendered = text(&Cell::Tool(cell).lines(80, &theme, TranscriptMode::Normal, 0));
    assert!(
        rendered
            .iter()
            .any(|row| row.contains("let total = a - b;")),
        "{rendered:?}"
    );
    let digest = rendered
        .iter()
        .find(|row| row.contains("first change at line 2"))
        .ok_or("no digest row")?;
    assert!(digest.ends_with("+1 -1"), "{digest}");
    Ok(())
}
