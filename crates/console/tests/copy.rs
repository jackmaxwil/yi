use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use yi_console::select::{Selection, copy, paint, source_span, words};

/// A drag over the drawn frame copied the rails, the bar, the gutter and the padding with
/// the words; the words are what a reader pastes.
#[test]
fn a_drag_copies_the_words_and_none_of_the_chrome() {
    let rows = [
        "│ $ cargo test        ",
        "│   running 3 tests   ",
        "┃   fix the parser    ",
        "• The answer.         ",
        "  And the rest.       ",
        "  ▌ ⚑ reminder note   ",
    ];
    let area = Rect::new(0, 0, 22, 6);
    let mut buffer = Buffer::empty(area);
    for (y, row) in rows.iter().enumerate() {
        buffer.set_string(
            0,
            u16::try_from(y).unwrap_or(0),
            row,
            ratatui::style::Style::default(),
        );
    }
    let selection = Selection {
        anchor: (0, 0),
        head: (21, 5),
    };
    let rows = paint(&mut buffer, area, selection, Color::Blue);
    assert_eq!(
        copy(&rows, |_| (None, None)),
        "$ cargo test\n  running 3 tests\nfix the parser\nThe answer.\n  And the rest.\n  ⚑ reminder note"
    );
    assert_eq!(
        words(&["    let a = 1;".to_owned(), "        let b = 2;".to_owned()]),
        "let a = 1;\n    let b = 2;",
        "a shared indent goes, a relative one stays"
    );
}

const REPLY: &str = "Honest take: the **error messages** are the best part.\n\n\
**Genuinely good**\n\n\
- **Error quality is the standout.** Every failure names the cause and the next move, \
and the refusals spell out the exact call shape they wanted.\n\
- `grid resolve`'s did-you-mean turned every dead end into a pointer.\n\n\
```rust\nlet tag = read(path)?;\n```\n\n\
The pattern: see [the notes](https://example.com/notes).\n";

/// A reply drawn the way the console draws it: beside a sidebar and its border, in a pane
/// four columns in, so every row carries chrome a copy must not.
fn drawn(source: &str) -> (Buffer, Rect) {
    use ratatui::widgets::Widget;
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
    let cell = yi_tui::cell::Cell::Assistant {
        markdown: source.to_owned(),
    };
    let lines = cell.lines(44, &theme, yi_tui::cell::TranscriptMode::Normal, 0);
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 52, height));
    for y in 0..height {
        buffer.set_string(6, y, "│", ratatui::style::Style::default());
    }
    let pane = Rect::new(8, 0, 44, height);
    ratatui::widgets::Paragraph::new(lines).render(pane, &mut buffer);
    (buffer, pane)
}

#[test]
fn a_drag_over_a_whole_reply_copies_its_markdown() {
    let (mut buffer, pane) = drawn(REPLY);
    // From the first word (past the bullet gutter) to the pane's last cell.
    let drag = Selection {
        anchor: (pane.x + 2, 1),
        head: (pane.right() - 1, pane.bottom() - 1),
    };
    let rows = paint(&mut buffer, pane, drag, Color::DarkGray);
    assert_eq!(copy(&rows, |_| (Some(0), Some(REPLY))), REPLY.trim_end());
}

#[test]
fn a_drag_inside_a_reply_copies_the_source_under_it() {
    let (mut buffer, pane) = drawn(REPLY);
    let rows = paint(
        &mut buffer,
        pane,
        Selection::new(pane.x, 0),
        Color::DarkGray,
    );
    assert!(rows.iter().all(|(_, row)| row.is_empty()));
    let shown: Vec<(u16, String)> = (0..pane.height)
        .map(|y| {
            let row: String = (pane.x..pane.right())
                .filter_map(|x| {
                    buffer
                        .cell(Position::new(x, y))
                        .map(|c| c.symbol().to_owned())
                })
                .collect();
            (y, row)
        })
        .collect();
    let line = |needle: &str| {
        shown
            .iter()
            .find(|(_, row)| row.contains(needle))
            .map(|(y, _)| *y)
            .unwrap_or_default()
    };
    // The wrapped bullet, from its first row to its last, snaps to its whole source line.
    let (top, bottom) = (line("Error quality"), line("call shape"));
    let drag = Selection {
        anchor: (pane.x, top),
        head: (pane.right() - 1, bottom),
    };
    let rows = paint(&mut buffer, pane, drag, Color::DarkGray);
    let copied = copy(&rows, |_| (Some(0), Some(REPLY)));
    assert!(copied.starts_with("- **Error quality"), "{copied}");
    assert!(copied.ends_with("they wanted."), "{copied}");
    assert!(
        !copied.contains('\n'),
        "a soft wrap is not a newline: {copied}"
    );

    // Just the code is just the code; the fence comes with it only when the drag does.
    let code = line("let tag");
    let drag = Selection {
        anchor: (pane.x, code),
        head: (pane.right() - 1, code),
    };
    let rows = paint(&mut buffer, pane, drag, Color::DarkGray);
    assert_eq!(
        copy(&rows, |_| (Some(0), Some(REPLY))),
        "let tag = read(path)?;"
    );
    let drag = Selection {
        anchor: (pane.x, code - 1),
        head: (pane.right() - 1, code),
    };
    let rows = paint(&mut buffer, pane, drag, Color::DarkGray);
    assert_eq!(
        copy(&rows, |_| (Some(0), Some(REPLY))),
        "```rust\nlet tag = read(path)?;\n```"
    );
}

#[test]
fn a_span_hugs_its_delimiters_but_not_a_words_insides() {
    assert_eq!(
        source_span("a **bold** word", "bold").as_deref(),
        Some("**bold**")
    );
    assert_eq!(
        source_span("call snake_case_name now", "case").as_deref(),
        Some("case")
    );
    assert_eq!(source_span("text", "nowhere"), None);
}

#[test]
fn a_cell_without_source_copies_its_words_without_chrome() {
    let rows = vec![
        (0, "  │ $ git status".to_owned()),
        (1, "  │".to_owned()),
        (2, "  │   ?? probe/".to_owned()),
    ];
    assert_eq!(
        copy(&rows, |_| (Some(3), None)),
        "$ git status\n\n  ?? probe/"
    );
}
