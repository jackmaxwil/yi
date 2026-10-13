//! The title field: a function of the free rows and one elapsed time, drawn under the chat of
//! an empty, idle pane and nowhere else.

use std::error::Error;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::Cell;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::title;

use crate::common;

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn rows(buffer: &Buffer, area: Rect) -> Vec<Vec<char>> {
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| {
                    buffer
                        .cell((x, y))
                        .and_then(|cell| cell.symbol().chars().next())
                        .unwrap_or(' ')
                })
                .collect()
        })
        .collect()
}

fn field(width: u16, height: u16, elapsed: Duration) -> Vec<Vec<char>> {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    title::paint(&mut buffer, area, elapsed, &theme());
    rows(&buffer, area)
}

/// The mark's strokes in `MARK_STROKES` span x from -0.62 to 0.46 and y from -0.62 to 0.62, so
/// the field centres them on (-0.08, 0) and scales by 0.95. The Y's stem is the segment at
/// x = -0.28, which lands at u = (-0.28 + 0.08) * 0.95 = -0.19; the i's stem at x = 0.46 lands
/// at u = 0.513 and its dot at (0.513, 0.5415). The stroke is 0.06 either side of its line.
#[test]
fn distance_is_signed_from_the_stroke_edge() {
    let near = |got: f64, want: f64| (got - want).abs() < 1e-9;
    assert!(
        near(title::distance(-0.19, -0.2), -0.06),
        "on the Y stem's line"
    );
    assert!(
        near(title::distance(-0.13, -0.2), 0.0),
        "on the stem's edge"
    );
    assert!(
        near(title::distance(0.01, -0.2), 0.14),
        "0.2 from the line, 0.06 of it stroke"
    );
    assert!(near(title::distance(0.513, 0.0), -0.06), "on the i's stem");
    assert!(
        near(title::distance(0.513, 0.5415), -0.06),
        "on the i's dot"
    );
    assert!(title::distance(5.0, 5.0) > 6.0, "far away is far");
}

/// Row 14 of 24 and column 35 of 80 hold the point (-0.1875, -0.208), 0.0025 from the Y stem's
/// line; row 29 of 50 and column 90 of 200 hold (-0.19, -0.18), on it.
#[test]
fn the_stem_is_solid_at_both_sizes() {
    for (width, height, col, row) in [(80, 24, 35, 14), (200, 50, 90, 29)] {
        let glyphs = field(width, height, Duration::ZERO);
        let at = glyphs.get(row).and_then(|line| line.get(col)).copied();
        assert_eq!(at, Some('@'), "{width}x{height} row {row} col {col}");
    }
}

/// The field's value is `ring² · e^(-0.9 d)` with `d` the distance, at most 0.26 past `d = 1.5`,
/// which rounds to step 2: a cell that far from the name never draws past `:`.
#[test]
fn far_cells_stay_faint_at_every_time() {
    for secs in [0.0, 0.4, 0.9, 1.4] {
        let glyphs = field(120, 40, Duration::from_secs_f64(secs));
        for (y, line) in glyphs.iter().enumerate() {
            for (x, glyph) in line.iter().enumerate() {
                let h = 40.0_f64;
                let u = (x as f64 + 0.5 - 60.0) / h;
                let v = (h - 2.0 * y as f64 - 1.0) / h;
                if title::distance(u, v) > 1.5 {
                    assert!(" .:".contains(*glyph), "{glyph:?} at {x},{y}, t {secs}");
                }
            }
        }
    }
}

#[test]
fn the_rings_move_and_the_loop_closes() {
    let at = |secs: f64| field(80, 24, Duration::from_secs_f64(secs));
    let start = 0.35;
    assert_eq!(
        at(start),
        at(start + title::RING_SECS),
        "a whole period is the same frame"
    );
    let changed = |a: &[Vec<char>], b: &[Vec<char>]| {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .filter(|(x, y)| x != y)
            .count()
    };
    let ring = title::RING_SECS;
    let step = changed(&at(0.9), &at(0.901));
    let seam = changed(&at(ring - 0.001), &at(ring));
    assert!(
        seam <= step + 3,
        "the wrap moved {seam} cells where a millisecond moves {step}"
    );
    let moved = at(start)
        .iter()
        .flatten()
        .zip(at(start + title::RING_SECS / 4.0).iter().flatten())
        .filter(|(a, b)| a != b)
        .count();
    assert!(moved > 100, "a quarter period moved only {moved} cells");
}

#[test]
fn the_strokes_do_not_pulse() {
    for secs in [0.0, 0.3, 0.7, 1.1, 1.6] {
        let glyphs = field(80, 24, Duration::from_secs_f64(secs));
        assert_eq!(glyphs[14][35], '@', "t {secs}");
    }
}

#[test]
fn an_area_with_no_room_draws_nothing_and_does_not_panic() {
    for (width, height) in [(0, 0), (0, 10), (10, 0), (1, 1), (1, 40), (300, 1)] {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        title::paint(&mut buffer, area, Duration::from_secs(3), &theme());
    }
}

fn app() -> App {
    App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "s".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        theme(),
        yi_tui::keymap::default_keymap(),
        80,
    )
}

const PANE: Rect = Rect::new(0, 0, 80, 30);

fn pane(app: &mut App) -> Vec<Vec<char>> {
    let mut buffer = Buffer::empty(PANE);
    let mut scroll = 0;
    yi_tui::render::paint_pane(app, None, &mut buffer, PANE, &mut scroll);
    rows(&buffer, PANE)
}

fn drawn_below(grid: &[Vec<char>], from: usize) -> usize {
    grid.iter()
        .skip(from)
        .flatten()
        .filter(|glyph| **glyph != ' ')
        .count()
}

#[test]
fn an_empty_idle_pane_shows_the_field_under_the_chat_only_when_enabled() -> TestResult {
    let mut off = app();
    let plain = pane(&mut off);
    assert_eq!(
        drawn_below(&plain, 4),
        0,
        "off by default: the rows below the chat are blank"
    );

    let mut on = app();
    on.set_title(true);
    assert!(on.title_visible());
    let shown = pane(&mut on);
    assert!(drawn_below(&shown, 4) > 50, "the field fills the free rows");
    assert_eq!(
        plain.get(..4),
        shown.get(..4),
        "the composer box and status row, the chat's own four, are never touched"
    );
    Ok(())
}

#[test]
fn the_first_message_ends_the_field() {
    let mut app = app();
    app.set_title(true);
    app.commit_cell(&Cell::User {
        text: "hello".to_owned(),
    });
    assert!(!app.title_visible());
    assert_eq!(drawn_below(&pane(&mut app), 12), 0);
}

#[test]
fn the_wake_is_asked_for_only_while_the_field_shows() {
    let mut app = app();
    assert_eq!(app.title_wake(), None);
    app.set_title(true);
    let wake = app.title_wake();
    assert!(
        wake.is_some_and(|wake| wake > Duration::ZERO && wake <= title::FRAME),
        "{wake:?}"
    );
}
