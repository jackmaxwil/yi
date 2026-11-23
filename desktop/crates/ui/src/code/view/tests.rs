//! The code surface driven in Rune's in-process runtime: layout, input and paint, no window.
//! Where a test needs to know where a character is, it reads the glyphs Rune painted.

use std::rc::Rc;
use std::time::Instant;

use rune::render::{KIND_GLYPH, KIND_SEGMENT};
use rune::{Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, PhysicalInput, Runtime, col, el};
use yd_core::ids::{PaneId, WindowId};
use yd_core::row::Tone;

use super::{CodeView, Gutter, reveal, sample};
use crate::code::colour::{Loaded, Pending};
use crate::code::doc::{Cols, Doc, Lang, LineNo};
use crate::code::marks::{Glyph, GutterMark, Marks, Tint};
use crate::code::pointer::{CodeEvent, Token};
use crate::code::rows::Lines;
use crate::code::state;
use crate::win::{Hub, Win};

#[cfg(test)]
mod reach;

type Check = Result<(), Box<dyn std::error::Error>>;
type Window = Runtime<Win>;

const SCALE: f32 = 2.0;
const PANEL_H: f32 = 100.0;
const LINE_H: f32 = 16.0;
const PANE: PaneId = PaneId(4);

/// What the test's window shows and what its surface told it.
#[derive(Default)]
struct Host {
    loaded: Option<Loaded>,
    marks: Rc<Marks>,
    gutter: Gutter,
    /// The line a 100-point panel sits under.
    panel: Option<LineNo>,
    heard: Vec<CodeEvent>,
}

fn root(win: &Win, _: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let Some(host) = hub.panes.get::<Host>(Some(PANE)) else {
        return col();
    };
    let Some(loaded) = &host.loaded else {
        return col();
    };
    let (doc, colours) = (loaded.doc.clone(), loaded.colours.clone());
    let surface = CodeView::new("code", &hub.model.tokens, doc, colours)
        .marks(host.marks.clone())
        .gutter(host.gutter)
        .on_event(|win, event, _| change(win, |host| host.heard.push(event)));
    let surface = match host.panel {
        Some(after) => surface.slot(after, "panel", |_| {
            el().key("code.panel").w_full().h(PANEL_H)
        }),
        None => surface,
    };
    col().w_full().h_full().child(surface.build())
}

fn change(win: &mut Win, f: impl FnOnce(&mut Host)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(host) = hub.panes.get_mut::<Host>(Some(PANE))
    {
        f(host);
    }
}

fn host_of(text: impl Into<String>, lang: Lang, marks: Marks, gutter: Gutter) -> Window {
    let doc = std::sync::Arc::new(Doc::new(text, lang, 0));
    let colours = Pending::of(&doc, None);
    let mut win = Win::new(Hub::new(&[], Vec::new()), WindowId(0));
    change(&mut win, |host| {
        host.loaded = Some(Loaded { doc, colours });
        host.marks = Rc::new(marks);
        host.gutter = gutter;
    });
    let mut rt = Runtime::new(win, root, (760.0, 400.0), Env::new(true, SCALE));
    rt.poll = Some(|win, _, tx| state::tick(win, tx));
    run(&mut rt, 0.5);
    rt
}

fn host(text: impl Into<String>, lang: Lang, marks: Marks) -> Window {
    host_of(text, lang, marks, Gutter::default())
}

/// Frames for `seconds` of the runtime's clock.
fn run(rt: &mut Window, seconds: f64) {
    let mut left = seconds;
    while left > 0.0 {
        rt.frame(1.0 / 60.0);
        left -= 1.0 / 60.0;
    }
}

fn line(n: u32) -> LineNo {
    LineNo::try_from(n).unwrap_or(LineNo::from_index(0))
}

fn heard(rt: &Window) -> Vec<CodeEvent> {
    let hub = rt.state.hub.try_borrow().ok();
    let host = hub
        .as_ref()
        .and_then(|hub| hub.panes.get::<Host>(Some(PANE)));
    host.map(|host| host.heard.clone()).unwrap_or_default()
}

/// What the surface said of hovers and presses, leaving out the selection.
fn told(rt: &Window) -> Vec<CodeEvent> {
    let pointed = |event: &CodeEvent| !matches!(event, CodeEvent::Selection(_));
    heard(rt).into_iter().filter(pointed).collect()
}

fn element(rt: &Window, key: &str) -> Result<ElementInfo, String> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .ok_or_else(|| format!("no element keyed {key}"))
}

/// The glyphs painted in the text of line `n`, left to right: each one's rectangle in points.
fn glyphs(rt: &Window, n: u32) -> Result<Vec<[f32; 4]>, String> {
    let text = element(rt, &format!("code.row.L{n}"))?.rect;
    let inside = |r: &[f32; 4]| {
        r[1] + r[3] / 2.0 >= text[1]
            && r[1] + r[3] / 2.0 <= text[1] + text[3]
            && r[0] >= text[0] - 1.0
    };
    let painted = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == KIND_GLYPH)
        .map(|i| i.rect.map(|v| v / SCALE));
    let mut out: Vec<[f32; 4]> = painted.filter(inside).collect();
    out.sort_by(|a, b| a[0].total_cmp(&b[0]));
    Ok(out)
}

/// The middle of the glyph of the `nth` character of line `n` that is not a space.
fn at_glyph(rt: &Window, n: u32, nth: usize) -> Result<(f32, f32), String> {
    let g = glyphs(rt, n)?
        .get(nth)
        .copied()
        .ok_or_else(|| format!("line {n} has no glyph {nth}"))?;
    Ok((g[0] + g[2] / 2.0, g[1] + g[3] / 2.0))
}

/// The strokes painted in the row of line `n`, left to right: each one's rectangle in points.
fn strokes(rt: &Window, n: u32) -> Result<Vec<[f32; 4]>, String> {
    let row = element(rt, &format!("code.L{n}"))?.rect;
    let inside = |r: &[f32; 4]| r[1] >= row[1] - 1.0 && r[1] + r[3] <= row[1] + row[3] + 1.0;
    let painted = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == KIND_SEGMENT)
        .map(|i| i.rect.map(|v| v / SCALE));
    let mut out: Vec<[f32; 4]> = painted.filter(inside).collect();
    out.sort_by(|a, b| a[0].total_cmp(&b[0]));
    Ok(out)
}

/// The left and right ends, in points, of the line painted under the code of line `n`.
fn underline(rt: &Window, n: u32) -> Result<(f32, f32), String> {
    let all = strokes(rt, n)?;
    let (first, last) = all
        .first()
        .zip(all.last())
        .ok_or("no line under the code")?;
    Ok((first[0], last[0] + last[2]))
}

fn press(rt: &mut Window, at: (f32, f32)) {
    rt.pointer_move(at.0, at.1);
    rt.pointer_down(at.0, at.1);
    rt.pointer_up(at.0, at.1);
    run(rt, 0.05);
}

fn copy(rt: &mut Window) {
    rt.key(KeyEvent {
        key: Key::Command('c'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    run(rt, 0.05);
}

/// The median and the 95th centile of `times`.
fn spread(mut times: Vec<f64>) -> (f64, f64) {
    times.sort_by(f64::total_cmp);
    let at = |q: usize| {
        let i = times.len().saturating_sub(1).saturating_mul(q) / 100;
        times.get(i).copied().unwrap_or(0.0)
    };
    (at(50), at(95))
}

#[test]
fn a_5000_line_file_builds_only_the_rows_in_view_and_stays_inside_the_frame_budget() -> Check {
    let mut rt = host(sample::rust(5000, 0), Lang::RUST, Marks::new());
    let lines = rt
        .state
        .hub
        .try_borrow()?
        .panes
        .get::<Host>(Some(PANE))
        .and_then(|host| host.loaded.as_ref())
        .map_or(0, |loaded| loaded.doc.lines());
    assert!(lines >= 5000);
    let built = |rt: &Window| {
        let rows = rt.elements().into_iter().filter_map(|e| e.key);
        rows.filter(|k| k.starts_with("code.L")).count()
    };
    assert!(
        (25..=130).contains(&built(&rt)),
        "{} rows built for a 400-point view",
        built(&rt)
    );
    assert_eq!(element(&rt, "code.L1")?.rect[3], LINE_H);
    assert_eq!(
        element(&rt, "code.L2")?.rect[1] - element(&rt, "code.L1")?.rect[1],
        LINE_H
    );

    // Twenty-five lines a frame, the fastest a hand flings: each frame's CPU before the GPU.
    let mut times = Vec::new();
    for _ in 0..240 {
        rt.wheel(380.0, 200.0, -400.0);
        let start = Instant::now();
        rt.frame(1.0 / 120.0);
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let (p50, p95) = spread(times);
    println!(
        "code surface, {lines} lines, 25 lines a frame: p50 {p50:.3} ms, p95 {p95:.3} ms, \
         {} rows built, {} layout nodes",
        built(&rt),
        rt.stats.layout_nodes
    );
    assert!(
        built(&rt) <= 130,
        "{} rows built while scrolling",
        built(&rt)
    );
    // Incident: a debug build is ten to twenty times slower, so the budget holds in release.
    if !cfg!(debug_assertions) {
        assert!(p95 < 8.3, "p95 {p95:.3} ms is over the 8.3 ms frame");
    }

    rt.update(move |_, tx| reveal(tx, "code", LineNo::from_index(lines - 10)));
    run(&mut rt, 0.5);
    let last = element(&rt, &format!("code.L{lines}"))?;
    assert!(
        last.visible[3] > 0.0 && last.rect[1] < 400.0,
        "the last line shows at {:?}",
        last.rect
    );
    assert_eq!(
        glyphs(&rt, u32::try_from(lines - 1)?)?.len(),
        1,
        "the file ends with a lone brace and an empty line"
    );
    Ok(())
}

#[test]
fn tints_and_gutter_marks_draw_on_their_own_lines() -> Check {
    let marks = Marks::new()
        .gutter(line(14), GutterMark::Breakpoint)
        .glyph(line(20), Glyph::Fix)
        .tint(line(18), Tint::Current)
        .tint(line(8), Tint::Added)
        .tint(line(10), Tint::Removed);
    let gutter = Gutter {
        marks: true,
        signs: true,
    };
    let rt = host_of(sample::RATE_LIMIT, Lang::RUST, marks, gutter);
    let (dot, row) = (
        element(&rt, "code.L14.breakpoint")?.rect,
        element(&rt, "code.L14")?.rect,
    );
    assert!(
        dot[0] < 13.0 && dot[1] > row[1] && dot[1] + dot[3] < row[1] + LINE_H,
        "the breakpoint sits in line 14's mark column: {dot:?}"
    );
    let bulb = element(&rt, "code.L20.fix")?.rect;
    assert_eq!(
        (
            bulb[0],
            bulb[1] - element(&rt, "code.L20")?.rect[1],
            bulb[2]
        ),
        (2.0, 3.0, 8.0)
    );
    assert_eq!(element(&rt, "code.L8.sign")?.text.as_deref(), Some("+"));
    assert_eq!(element(&rt, "code.L10.sign")?.text.as_deref(), Some("-"));
    let fill = |n: u32| element(&rt, &format!("code.L{n}")).map(|e| e.fill.a);
    assert!(
        fill(18)? > 0.1 && fill(10)? > 0.1,
        "the current and the removed line are filled"
    );
    assert_eq!(
        (fill(8)?, fill(17)?),
        (0.0, 0.0),
        "an added line has its bar and no fill, a plain line neither"
    );
    for n in [8, 10, 18] {
        let (bar, at) = (
            element(&rt, &format!("code.L{n}.bar"))?,
            element(&rt, &format!("code.L{n}"))?,
        );
        assert_eq!(
            (bar.rect[0], bar.rect[2], bar.rect[3]),
            (at.rect[0], 2.0, at.rect[3]),
            "line {n} has a 2 point bar down its left edge"
        );
        assert!(bar.fill.a > 0.9, "line {n}'s bar is solid");
    }
    for unmarked in [
        "code.L15.breakpoint",
        "code.L19.fix",
        "code.L9.sign",
        "code.L17.bar",
    ] {
        assert!(element(&rt, unmarked).is_err(), "{unmarked} draws nothing");
    }
    // The mark column is 13 points: with it the code starts that much further right.
    let with = element(&rt, "code.row.L1")?.rect[0];
    let without = host(sample::RATE_LIMIT, Lang::RUST, Marks::new());
    assert_eq!(with - element(&without, "code.row.L1")?.rect[0], 13.0);
    assert_eq!(element(&without, "code.row.L1")?.rect[0], 28.0 + 11.0);
    Ok(())
}

#[test]
fn a_slot_opens_between_two_lines_and_closes_again() -> Check {
    let mut rt = host(sample::RATE_LIMIT, Lang::RUST, Marks::new());
    let (above, below) = (
        element(&rt, "code.L13")?.rect[1],
        element(&rt, "code.L14")?.rect[1],
    );
    assert_eq!(below - above, LINE_H);

    rt.update(|win, _| change(win, |host| host.panel = Some(line(13))));
    run(&mut rt, 1.5);
    let panel = element(&rt, "code.slot.panel")?.rect;
    assert_eq!(
        (panel[1], panel[3]),
        (above + LINE_H, PANEL_H),
        "the panel sits right under line 13"
    );
    assert_eq!(panel[2], 760.0, "and is as wide as the surface");
    assert_eq!(
        element(&rt, "code.L13")?.rect[1],
        above,
        "the line above stays"
    );
    assert_eq!(
        element(&rt, "code.L14")?.rect[1],
        below + PANEL_H,
        "the lines below move down by the panel's height"
    );
    assert_eq!(
        element(&rt, "code.L23")?.rect[1],
        above + LINE_H * 10.0 + PANEL_H
    );

    rt.update(|win, _| change(win, |host| host.panel = None));
    run(&mut rt, 1.5);
    assert_eq!(element(&rt, "code.L14")?.rect[1], below);
    assert!(element(&rt, "code.slot.panel").is_err());
    Ok(())
}

/// Incident: the dogfood log named the line's `on_hover_change` as running the whole view for
/// line after line. A native event can land after a hover's clock ran out but before the poll.
#[test]
fn leaving_a_line_after_the_hovers_clock_ran_out_leaves_the_end_to_the_poll() -> Check {
    let mut rt = host(sample::RATE_LIMIT, Lang::RUST, Marks::new());
    let on_take = at_glyph(&rt, 13, 6)?;
    rt.pointer_move(on_take.0, on_take.1);
    run(&mut rt, 0.5);
    assert_eq!(told(&rt).len(), 1, "the hover started");
    let poll = rt.poll.take();
    rt.pointer_move(4.0, on_take.1);
    run(&mut rt, 0.3);
    let hear: state::Hear = Rc::new(|win, event, _| change(win, |host| host.heard.push(event)));
    rt.update(move |win: &mut Win, tx| {
        let at = win
            .hub
            .try_borrow()
            .ok()
            .and_then(|hub| state::pointer(&hub, "code"));
        if let Some(at) = at {
            state::leave(win, "code", at.line, &hear, tx);
        }
    });
    assert_eq!(
        told(&rt).len(),
        1,
        "the line's leave ended the hover itself"
    );
    rt.poll = poll;
    run(&mut rt, 0.1);
    assert_eq!(told(&rt).len(), 2, "the poll ends it: {:?}", told(&rt));
    Ok(())
}

#[test]
fn resting_on_a_token_starts_a_hover_and_presses_say_which_token() -> Check {
    let follow = Marks::new().follow(line(13), Cols::new(11, 15), false);
    let mut rt = host(sample::RATE_LIMIT, Lang::RUST, follow);
    // Line 13 is `    pub fn take(&mut self, ...`: glyph 6 is the `a` of `take`, glyph 12 the `m` of `mut`.
    let take = Token {
        line: line(13),
        cols: Cols::new(11, 15),
        follows: true,
    };
    let mutable = Token {
        line: line(13),
        cols: Cols::new(17, 20),
        follows: false,
    };
    let on_take = at_glyph(&rt, 13, 6)?;

    rt.pointer_move(on_take.0, on_take.1);
    run(&mut rt, 0.2);
    assert_eq!(told(&rt), [], "a hover waits for the pointer to rest");
    run(&mut rt, 0.3);
    assert_eq!(told(&rt), [CodeEvent::HoverStart(take)]);
    assert_eq!(
        state::hovered(&*rt.state.hub.try_borrow()?, "code"),
        Some(take)
    );
    assert_eq!(
        rt.cursor(),
        rune::Cursor::Pointer,
        "a name that can be followed shows a hand"
    );

    press(&mut rt, on_take);
    rt.physical_input(PhysicalInput::Modifiers(Mods {
        cmd: true,
        ..Mods::default()
    }));
    press(&mut rt, on_take);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    assert_eq!(
        told(&rt).get(1..),
        Some(&[CodeEvent::Press(take), CodeEvent::CommandPress(take)][..])
    );

    let on_mut = at_glyph(&rt, 13, 12)?;
    rt.pointer_move(on_mut.0, on_mut.1);
    run(&mut rt, 0.05);
    assert_eq!(told(&rt).len(), 3, "the hover outlasts a short gap");
    assert_eq!(
        rt.cursor(),
        rune::Cursor::Text,
        "and any other word shows the text cursor"
    );
    run(&mut rt, 0.6);
    assert_eq!(
        told(&rt).get(3..),
        Some(&[CodeEvent::HoverEnd(take), CodeEvent::HoverStart(mutable)][..])
    );

    rt.pointer_move(700.0, 380.0);
    run(&mut rt, 0.3);
    assert_eq!(told(&rt).last(), Some(&CodeEvent::HoverEnd(mutable)));
    Ok(())
}

#[test]
fn a_hint_before_a_token_does_not_move_where_the_token_is_hit_or_underlined() -> Check {
    // Line 19 is `            Some(left) => { self.tokens = left; Ok(()) }`; the hint goes in after `left`.
    let marks = Marks::new()
        .hint(line(19), 21, ": u32")
        .squiggle(line(19), Cols::new(28, 39), Tone::Need)
        .value(line(19), "left = 3");
    let mut rt = host(sample::RATE_LIMIT, Lang::RUST, marks);
    let drawn = glyphs(&rt, 19)?;
    // Painted: `Some(left` is 9 glyphs, the hint `:u32` 4, then `)=>{` 4, then `self.tokens` 11.
    let (first, last, before, after) = (drawn[17], drawn[27], drawn[16], drawn[28]);
    assert!(
        drawn[9][2] < drawn[8][2] || drawn[10][3] < drawn[8][3],
        "glyphs 9 to 12 are the hint's smaller ones: {:?} {:?}",
        drawn[8],
        drawn[10]
    );
    assert_eq!(
        element(&rt, "code.L19.value")?.rect[3],
        13.0,
        "the label after the line's end is a 13-point pill"
    );

    press(
        &mut rt,
        (first[0] + first[2] / 2.0, first[1] + first[3] / 2.0),
    );
    assert_eq!(
        heard(&rt),
        [CodeEvent::Press(Token {
            line: line(19),
            cols: Cols::new(28, 32),
            follows: false
        })]
    );
    press(
        &mut rt,
        (
            drawn[10][0] + drawn[10][2] / 2.0,
            drawn[10][1] + drawn[10][3] / 2.0,
        ),
    );
    assert_eq!(
        heard(&rt).len(),
        1,
        "a press on the hint is not a press on a token"
    );
    let value = drawn.last().copied().ok_or("line 19 drew nothing")?;
    press(
        &mut rt,
        (value[0] + value[2] / 2.0, value[1] + value[3] / 2.0),
    );
    assert_eq!(
        heard(&rt).len(),
        1,
        "nor is a press on the value after the line"
    );

    let (left, right) = underline(&rt, 19)?;
    assert!(
        left > before[0] + before[2] / 2.0 && left <= first[0] + 1.0,
        "the wavy line starts at `self`: {left} against {first:?}"
    );
    assert!(
        right >= last[0] + last[2] - 1.0 && right < after[0] + after[2] / 2.0,
        "and ends with `tokens`: {right} against {last:?}"
    );
    Ok(())
}

#[test]
fn a_selection_dragged_across_lines_copies_the_source_and_leaves_hints_out() -> Check {
    // Lines 10 to 13 are `}`, an empty line, `impl Bucket {` with a hint in it, and `pub fn take` behind a tab.
    let marks = Marks::new().hint(line(12), 5, ": T");
    let mut rt = host(
        sample::RATE_LIMIT.replace("    pub fn take", "\tpub fn take"),
        Lang::RUST,
        marks,
    );
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    // The drag starts in the empty part of line 9's row, well right of where its text ends.
    let (from, to) = (element(&rt, "code.L9")?.rect, glyphs(&rt, 13)?[2]);
    rt.pointer_move(600.0, from[1] + from[3] / 2.0);
    rt.pointer_down(600.0, from[1] + from[3] / 2.0);
    run(&mut rt, 0.05);
    rt.pointer_move(to[0] + to[2] + 0.5, to[1] + to[3] / 2.0);
    run(&mut rt, 0.05);
    rt.pointer_up(to[0] + to[2] + 0.5, to[1] + to[3] / 2.0);
    run(&mut rt, 0.05);

    copy(&mut rt);
    let copied = clipboard.text().map(|text| text.to_string());
    // Rune counts a text as selected only once a character of it is: line 9's break is not taken.
    assert_eq!(copied.as_deref(), Some("}\n\nimpl Bucket {\n\tpub"));
    let told = heard(&rt);
    assert_eq!(
        told.last(),
        Some(&CodeEvent::Selection(Some(Lines::new(line(10), line(13))))),
        "the caller is told which lines the selection touches"
    );
    assert!(
        told.iter().all(|e| matches!(e, CodeEvent::Selection(_))),
        "a drag that selects is not a press on a token: {told:?}"
    );

    // The whole file, most of it rows that were never built, copies as the file.
    let mut long = host(sample::rust(5000, 0), Lang::RUST, Marks::new());
    long.use_clipboard(&clipboard);
    long.update(|_, tx| tx.select_all_text("code.code"));
    run(&mut long, 0.05);
    copy(&mut long);
    let source = sample::rust(5000, 0);
    let copied = clipboard.text().map(|text| text.to_string());
    assert_eq!(
        copied.as_deref().map(str::len),
        Some(source.trim_end_matches('\n').len())
    );
    assert_eq!(copied.as_deref(), Some(source.trim_end_matches('\n')));
    Ok(())
}
