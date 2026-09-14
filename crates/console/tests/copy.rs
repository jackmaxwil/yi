use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use yi_console::select::{Selection, paint, words};

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
    let text = paint(&mut buffer, area, selection, Color::Blue);
    assert_eq!(
        text,
        "$ cargo test\n  running 3 tests\nfix the parser\nThe answer.\n  And the rest.\n  ⚑ reminder note"
    );
    assert_eq!(
        words(&["    let a = 1;".to_owned(), "        let b = 2;".to_owned()]),
        "let a = 1;\n    let b = 2;",
        "a shared indent goes, a relative one stays"
    );
}
