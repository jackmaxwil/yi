//! What the surface reaches past plain lines: the gutter, the right button, long lines, long
//! files, names to follow and wide characters.

use rune::{Mods, PhysicalInput};
use yd_core::row::Tone;

use super::{Check, at_glyph, element, glyphs, heard, host, line, press, run, strokes, underline};
use crate::code::doc::{Cols, Lang};
use crate::code::marks::Marks;
use crate::code::pointer::{CodeEvent, Token};
use crate::code::view::sample;

#[test]
fn a_press_in_the_gutter_and_a_right_click_say_which_line() -> Check {
    let mut rt = host(sample::RATE_LIMIT, Lang::RUST, Marks::new());
    let row = element(&rt, "code.L14")?.rect;
    press(&mut rt, (10.0, row[1] + 8.0));
    assert_eq!(heard(&rt), [CodeEvent::GutterPress(line(14))]);
    rt.physical_input(PhysicalInput::Modifiers(Mods {
        cmd: true,
        ..Mods::default()
    }));
    press(&mut rt, (10.0, row[1] + 8.0));
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    assert_eq!(
        heard(&rt).len(),
        2,
        "the gutter is the gutter with Command held too"
    );

    // Line 13 is `    pub fn take(...`: glyph 6 is the `a` of `take`.
    let on_take = at_glyph(&rt, 13, 6)?;
    rt.secondary_down(on_take.0, on_take.1);
    rt.secondary_up(on_take.0, on_take.1);
    run(&mut rt, 0.05);
    let take = Token {
        line: line(13),
        cols: Cols::new(11, 15),
        follows: false,
    };
    assert_eq!(
        heard(&rt).last(),
        Some(&CodeEvent::SecondaryPress {
            line: line(13),
            token: Some(take),
            at: [on_take.0, on_take.1],
        })
    );
    let empty = element(&rt, "code.L11")?.rect;
    rt.secondary_down(300.0, empty[1] + 8.0);
    rt.secondary_up(300.0, empty[1] + 8.0);
    run(&mut rt, 0.05);
    assert_eq!(
        heard(&rt).last(),
        Some(&CodeEvent::SecondaryPress {
            line: line(11),
            token: None,
            at: [300.0, empty[1] + 8.0],
        }),
        "a line with nothing under the pointer is still a line"
    );
    Ok(())
}

#[test]
fn the_number_column_widens_only_past_9999_lines() -> Check {
    let code_x = |lines: usize| -> Result<f32, String> {
        let rt = host("x\n".repeat(lines), Lang::PLAIN, Marks::new());
        Ok(element(&rt, "code.row.L1")?.rect[0])
    };
    let (short, four, five) = (code_x(999)?, code_x(9_999)?, code_x(10_000)?);
    assert_eq!(short, four, "four digits fit the 28-point column");
    assert!(
        five >= four + 4.0,
        "five digits need a wider column: {five} against {four}"
    );
    // The last number of a 10,000-line file is drawn whole, left of the code.
    let mut rt = host("x\n".repeat(10_000), Lang::PLAIN, Marks::new());
    rt.update(|_, tx| super::reveal(tx, "code", line(9_990)));
    run(&mut rt, 0.5);
    let row = element(&rt, "code.L10000")?;
    let number = rt
        .elements()
        .into_iter()
        .find(|e| e.text.as_deref() == Some("10000"))
        .ok_or("the number 10000 is not drawn")?;
    assert!(
        number.rect[0] >= row.rect[0] && number.rect[0] + number.rect[2] <= five,
        "the number sits between the row's edge and the code: {:?}",
        number.rect
    );
    Ok(())
}

#[test]
fn a_long_line_scrolls_sideways() -> Check {
    let long = format!(
        "fn short() {{}}\nlet wide = \"{}\";\n",
        "abcdefgh".repeat(40)
    );
    let mut rt = host(long, Lang::RUST, Marks::new());
    let before = element(&rt, "code.row.L2")?.rect;
    assert!(
        before[2] > 760.0,
        "the line is laid out at its own width, {} points, not cut at the view's",
        before[2]
    );
    rt.wheel_xy(400.0, 24.0, -300.0, 0.0);
    run(&mut rt, 0.5);
    let after = element(&rt, "code.row.L2")?.rect;
    assert!(
        after[0] <= before[0] - 250.0,
        "the wheel moved the text left: {} to {}",
        before[0],
        after[0]
    );
    assert_eq!(
        element(&rt, "code.row.L1")?.rect[0],
        after[0],
        "every line moves with it"
    );
    for _ in 0..40 {
        rt.wheel_xy(400.0, 24.0, -300.0, 0.0);
        rt.frame(1.0 / 60.0);
    }
    run(&mut rt, 0.5);
    let end = element(&rt, "code.row.L2")?.rect;
    let last = glyphs(&rt, 2)?.last().copied().ok_or("nothing painted")?;
    assert!(
        end[0] + end[2] <= 761.0 && last[0] + last[2] > 700.0,
        "scrolled to its end the line's last character shows: {end:?} {last:?}"
    );
    Ok(())
}

#[test]
fn a_name_to_follow_is_underlined_with_dots_and_with_a_line_once_open() -> Check {
    // Line 13 is `    pub fn take(...`: `take` is bytes 11 to 15 and glyphs 5 to 8.
    let dotted = host(
        sample::RATE_LIMIT,
        Lang::RUST,
        Marks::new().follow(line(13), Cols::new(11, 15), false),
    );
    let drawn = glyphs(&dotted, 13)?;
    let (t, e) = (drawn[5], drawn[8]);
    let dots = strokes(&dotted, 13)?;
    assert!(
        dots.len() >= 10,
        "a dot every two points under four characters: {}",
        dots.len()
    );
    // A painted stroke is its line and a point of edge each side: a one-point dot is three wide.
    assert!(
        dots.iter().all(|dot| dot[2] < 3.5) && dots[1][0] - dots[0][0] == 2.0,
        "each dot is a point long and a point from the next: {:?}",
        &dots[..2]
    );
    let (left, right) = underline(&dotted, 13)?;
    assert!(
        left >= t[0] - 1.5 && right <= e[0] + e[2] + 1.5,
        "the dots stay under `take`: {left} to {right} against {t:?} and {e:?}"
    );

    let open = host(
        sample::RATE_LIMIT,
        Lang::RUST,
        Marks::new().follow(line(13), Cols::new(11, 15), true),
    );
    assert_eq!(
        strokes(&open, 13)?.len(),
        1,
        "the name whose references are open has one solid line"
    );
    let plain = host(sample::RATE_LIMIT, Lang::RUST, Marks::new());
    assert_eq!(strokes(&plain, 13)?.len(), 0);
    Ok(())
}

#[test]
fn a_wavy_line_stays_under_its_characters_far_along_a_line() -> Check {
    // 104 characters with no space among them, so glyph 96 is byte 96.
    let marks = Marks::new().squiggle(line(1), Cols::new(96, 102), Tone::Need);
    let rt = host("abcdefgh".repeat(13), Lang::PLAIN, marks);
    let drawn = glyphs(&rt, 1)?;
    assert_eq!(drawn.len(), 104);
    let (left, right) = underline(&rt, 1)?;
    let (first, last) = (drawn[96], drawn[101]);
    assert!(
        left > drawn[95][0] + drawn[95][2] / 2.0 && left <= first[0] + 1.0,
        "starts at character 96: {left} against {first:?}"
    );
    assert!(
        right >= last[0] + last[2] - 1.0 && right < drawn[102][0] + drawn[102][2] / 2.0,
        "ends with character 101: {right} against {last:?}"
    );
    Ok(())
}

#[test]
fn a_wavy_line_sits_under_wide_characters_too() -> Check {
    let marks = Marks::new().squiggle(line(1), Cols::new(4, 10), Tone::Bad);
    let rt = host("let 名前 = \"日本語\";\n", Lang::RUST, marks);
    let drawn = glyphs(&rt, 1)?;
    // Painted: `let`, then the two wide characters, then `=`.
    let (left, right) = underline(&rt, 1)?;
    assert!(
        left > drawn[2][0] + drawn[2][2] / 2.0 && left <= drawn[3][0] + 1.0,
        "starts at the first wide character: {left} against {:?}",
        drawn[3]
    );
    assert!(
        right >= drawn[4][0] + drawn[4][2] - 1.0 && right < drawn[5][0] + drawn[5][2] / 2.0,
        "ends with the second: {right} against {:?}",
        drawn[4]
    );
    Ok(())
}
