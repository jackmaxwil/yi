use std::error::Error;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;
use yi_tui::composer::Composer;
use yi_tui::keymap::{Action, EvalContext, KeyCodeValue, KeyInput, SingleKey, default_keymap};

type TestResult = Result<(), Box<dyn Error>>;

/// The owner's own prompt, typed as one paragraph with no newline, plus a run of wide CJK.
const PROMPT: &str = "Make the text input from a single line into a large box area that \
increased in sized to dynamic breakpoints to optimally contain large text areas in TUI in yi \
console and yi solo, same path. 易経は変化の書であり、長い段落を折り返す";

fn press(composer: &mut Composer, code: KeyCode, modifiers: KeyModifiers) {
    composer.input(KeyEvent::new(code, modifiers));
}

fn lines(n: usize) -> String {
    (1..=n)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Paints the box at a nonzero offset, the way the chat pane places it, and returns its rows.
fn paint(composer: &mut Composer, width: u16, pane_rows: u16) -> Vec<String> {
    let height = composer.layout(width, pane_rows);
    composer.set_frame(Style::default(), Style::default(), None);
    let screen = Rect::new(0, 0, width, height.saturating_add(4));
    let mut buffer = Buffer::empty(screen);
    composer.render(
        Rect::new(1, 3, width.saturating_sub(2), height),
        &mut buffer,
    );
    (screen.top()..screen.bottom())
        .map(|y| {
            let mut row = String::new();
            let mut covered = 0;
            for x in screen.left()..screen.right() {
                let symbol = buffer.cell((x, y)).map_or(" ", |cell| cell.symbol());
                if covered > 0 {
                    covered -= 1;
                    continue;
                }
                covered = symbol.width().saturating_sub(1);
                row.push_str(symbol);
            }
            row
        })
        .collect()
}

/// Dies with the paragraph on one row scrolling sideways: the box counted logical lines, so a
/// prompt with no newline never grew past a single row.
#[test]
fn a_typed_paragraph_wraps_inside_the_box_and_keeps_every_word() -> TestResult {
    let mut composer = Composer::default();
    composer.set_text(PROMPT);
    let rows = paint(&mut composer, 60, 60);
    let inner: Vec<&str> = rows
        .iter()
        .filter_map(|row| row.strip_prefix(" │"))
        .filter_map(|row| row.trim_end().strip_suffix('│'))
        .collect();
    assert!(inner.len() >= 3, "the paragraph wraps onto rows: {rows:#?}");
    for row in &rows {
        assert!(row.width() <= 60, "no row runs past the screen: {row:?}");
    }
    let joined = inner
        .iter()
        .map(|row| row.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let words: Vec<&str> = joined.split_whitespace().collect();
    let expected: Vec<&str> = PROMPT.split_whitespace().collect();
    assert_eq!(words, expected, "every word shows, in order: {rows:#?}");
    assert!(
        rows.first().is_some_and(|row| row.trim().is_empty()),
        "nothing paints above the box's rectangle: {rows:#?}"
    );
    Ok(())
}

/// An empty box rests at three text rows on a pane of 24 rows or more, one below that.
#[test]
fn an_empty_box_rests_at_three_rows_on_a_full_size_pane() -> TestResult {
    let mut composer = Composer::default();
    assert_eq!(composer.layout(80, 24), 5);
    assert_eq!(composer.layout(80, 23), 3);
    Ok(())
}

/// The box grows in steps of 1, 2, 3, 5, 8, 13, 21 rows and stops at 38.2% of the pane less
/// its border; each case sits at a step or at the cap and one row past it.
#[test]
fn the_box_grows_by_steps_and_stops_at_the_golden_cap() -> TestResult {
    let mut composer = Composer::default();
    let cases = [
        // (pane rows, text rows, text rows the box shows)
        (60, 4, 5),
        (60, 5, 5),
        (60, 6, 8),
        (60, 13, 13),
        (60, 14, 20),
        (60, 20, 20),
        (60, 21, 20),
        (24, 7, 7),
        (24, 8, 7),
    ];
    for (pane, text, shown) in cases {
        composer.set_text(&lines(text));
        assert_eq!(
            composer.layout(80, pane),
            shown + 2,
            "{text} text rows on a {pane}-row pane"
        );
    }
    Ok(())
}

/// Dies with Up a no-op on a wrapped paragraph: the textarea moved by logical line, and one
/// paragraph is one line however many rows it fills.
#[test]
fn up_and_down_walk_the_wrapped_rows_of_one_paragraph() -> TestResult {
    let mut composer = Composer::default();
    composer.set_text(PROMPT);
    let end = composer.textarea.cursor();
    let _ = composer.layout(40, 60);
    press(&mut composer, KeyCode::Up, KeyModifiers::NONE);
    let up = composer.textarea.cursor();
    assert_eq!(up.0, 0, "still the one logical line");
    assert!(
        up.1 < end.1,
        "Up left the last wrapped row: {up:?} from {end:?}"
    );
    press(&mut composer, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        composer.textarea.cursor(),
        end,
        "Down comes back to where it began"
    );
    Ok(())
}

/// Dies with ctrl+u undoing instead: Ghostty sends Cmd+Delete as ctrl+u, which the textarea
/// bound to undo; kitty-protocol terminals send Cmd+Delete as super+backspace.
#[test]
fn cmd_delete_and_ctrl_u_clear_the_whole_line_the_cursor_is_on() -> TestResult {
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::SUPER] {
        let mut composer = Composer::default();
        composer.set_text("keep this\nclear this whole line");
        for _ in 0..4 {
            press(&mut composer, KeyCode::Left, KeyModifiers::NONE);
        }
        let code = if modifiers == KeyModifiers::CONTROL {
            KeyCode::Char('u')
        } else {
            KeyCode::Backspace
        };
        press(&mut composer, code, modifiers);
        assert_eq!(composer.text(), "keep this\n", "{modifiers:?}");
    }
    Ok(())
}

#[test]
fn option_delete_and_ctrl_backspace_delete_one_word() -> TestResult {
    for modifiers in [KeyModifiers::ALT, KeyModifiers::CONTROL] {
        let mut composer = Composer::default();
        composer.set_text("fix the composer box");
        press(&mut composer, KeyCode::Backspace, modifiers);
        assert_eq!(composer.text(), "fix the composer ", "{modifiers:?}");
    }
    Ok(())
}

/// Dies with ctrl+j deleting to the line's head: Terminal.app and iTerm2 report Shift+Enter as
/// Enter, so ctrl+j and Option+Enter are the newline keys a terminal without kitty flags has.
#[test]
fn ctrl_j_and_option_enter_insert_a_newline() -> TestResult {
    for (code, modifiers) in [
        (KeyCode::Char('j'), KeyModifiers::CONTROL),
        (KeyCode::Enter, KeyModifiers::ALT),
    ] {
        let mut composer = Composer::default();
        composer.set_text("first");
        press(&mut composer, code, modifiers);
        press(&mut composer, KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(composer.text(), "first\nx", "{modifiers:?}+{code:?}");
    }
    Ok(())
}

/// Dies with ctrl+z typed as nothing: undo sat on ctrl+u, and Ghostty keeps Cmd+Z for itself.
#[test]
fn ctrl_z_undoes_and_ctrl_shift_z_redoes() -> TestResult {
    let mut composer = Composer::default();
    for ch in ['a', 'b'] {
        press(&mut composer, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    press(&mut composer, KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(composer.text(), "a");
    press(
        &mut composer,
        KeyCode::Char('Z'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(composer.text(), "ab");
    Ok(())
}

/// Dies with Option+Left jumping to a sibling agent mid-sentence: kitty, WezTerm and iTerm2
/// report Option+Left as alt+left, which the keymap gave to focus whatever the box held.
#[test]
fn option_arrows_move_by_word_over_a_draft_and_walk_agents_from_an_empty_box() -> TestResult {
    let keymap = default_keymap();
    let alt_left = KeyInput::Single(SingleKey {
        code: KeyCodeValue::Left,
        ctrl: false,
        alt: true,
        shift: false,
    });
    let empty = EvalContext { input_empty: true };
    let draft = EvalContext { input_empty: false };
    assert_eq!(
        keymap.resolve(&alt_left, &empty),
        Some(Action::FocusPrevSibling)
    );
    assert_eq!(keymap.resolve(&alt_left, &draft), None);
    let mut composer = Composer::default();
    composer.set_text("fix the box");
    press(&mut composer, KeyCode::Left, KeyModifiers::ALT);
    press(&mut composer, KeyCode::Char('X'), KeyModifiers::NONE);
    assert_eq!(composer.text(), "fix the Xbox");
    Ok(())
}
