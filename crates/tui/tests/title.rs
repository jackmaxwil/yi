//! The title field: a function of the free rows and one elapsed time, drawn under the chat of
//! an empty, idle pane and nowhere else.

use std::error::Error;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::Cell;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::title::{self, Scene};

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

fn scene(which: Scene, width: u16, height: u16, elapsed: Duration) -> Vec<Vec<char>> {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    title::paint(&mut buffer, area, which, elapsed, &theme());
    rows(&buffer, area)
}

fn field(width: u16, height: u16, elapsed: Duration) -> Vec<Vec<char>> {
    scene(Scene::Echo, width, height, elapsed)
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
        title::paint(
            &mut buffer,
            area,
            Scene::Echo,
            Duration::from_secs(3),
            &theme(),
        );
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
fn an_empty_idle_pane_is_filled_by_the_field_only_when_enabled() -> TestResult {
    let mut off = app();
    let plain = pane(&mut off);
    assert_eq!(
        drawn_below(&plain, 4),
        0,
        "off by default: the rows below the chat are blank"
    );

    let mut on = app();
    on.set_title(Some((Scene::Kali, Duration::from_secs(4))));
    assert!(on.title_visible());
    let shown = pane(&mut on);
    assert!(
        drawn_below(&shown, 4) > 500,
        "the field fills the rows below the chat"
    );
    assert_eq!(
        plain.get(..4),
        shown.get(..4),
        "the composer, its inside and the status row stay as the chat drew them"
    );
    Ok(())
}

#[test]
fn the_first_message_ends_the_field() {
    let mut app = app();
    app.set_title(Some((Scene::Echo, Duration::ZERO)));
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
    app.set_title(Some((Scene::Echo, Duration::ZERO)));
    let wake = app.title_wake();
    assert!(
        wake.is_some_and(|wake| wake > Duration::ZERO && wake <= title::FRAME),
        "{wake:?}"
    );
}

fn lit(glyphs: &[Vec<char>]) -> usize {
    glyphs
        .iter()
        .flatten()
        .filter(|glyph| **glyph != ' ')
        .count()
}

/// At the first instant the fold is faded out and the name has not risen: an empty screen.
/// Past the opening the name is solid on the Y stem and the fold fills the rest of the rows.
#[test]
fn the_fold_opens_from_nothing_and_the_name_settles_in() {
    let at = |secs: f64| scene(Scene::Kali, 80, 24, Duration::from_secs_f64(secs));
    assert_eq!(lit(&at(0.0)), 0, "nothing at the first instant");
    let settled = at(4.0);
    assert_eq!(settled[14][35], '@', "the Y stem is solid");
    assert!(
        lit(&settled) > 600,
        "the fold fills the rows: {}",
        lit(&settled)
    );
}

#[test]
fn the_draw_follows_the_weights_and_the_seed() {
    let weights = [(Scene::Kali, 8), (Scene::Echo, 2)];
    let kali = (0..20_000)
        .filter(|seed| title::pick(&weights, *seed) == Some(Scene::Kali))
        .count();
    assert!(
        (15_600..=16_400).contains(&kali),
        "kali drew {kali} of 20000"
    );
    assert_eq!(
        title::pick(&weights, 7),
        title::pick(&weights, 7),
        "one seed, one scene"
    );
    assert_eq!(title::pick(&[], 1), None);
    assert_eq!(title::pick(&[(Scene::Kali, 0), (Scene::Echo, 0)], 1), None);
    let only = [(Scene::Kali, 0), (Scene::Echo, 3)];
    assert!((0..200).all(|seed| title::pick(&only, seed) == Some(Scene::Echo)));
}

#[test]
fn a_scene_is_found_by_its_name_and_by_no_other() {
    assert_eq!(Scene::named("kali"), Some(Scene::Kali));
    assert_eq!(Scene::named("echo"), Some(Scene::Echo));
    assert_eq!(Scene::named("Kali"), None);
    assert_eq!(Scene::named(""), None);
}

/// The small mark's cells: 10 by 5 at column 2, row 1. In `MARK_STROKES` the Y's stem is the line
/// x = -0.28, which the orb draws at (32 - 0.28 * 1.0256 * 24.96) / 64 = 0.388 of the image's
/// width, so column 2 + 3.88 = 5.88 and cell 5; its foot y = -0.62 is at 0.748 of the height,
/// row 1 + 3.74 = 4.74 and cell 4.
#[test]
fn the_name_lands_on_the_small_mark_and_the_field_is_gone() {
    let area = Rect::new(0, 0, 80, 24);
    let landing = Rect::new(2, 1, 10, 5);
    let at = |secs: f64| {
        let mut buffer = Buffer::empty(area);
        title::paint_with(
            &mut buffer,
            area,
            (Scene::Kali, Duration::from_secs_f64(secs)),
            (Some(landing), None),
            &theme(),
        );
        rows(&buffer, area)
    };
    let early = at(4.0);
    assert_eq!(
        early[14][35], '@',
        "held in the middle until the landing starts"
    );
    let landed = at(5.39);
    assert_eq!(
        landed[4][5], '@',
        "the stem's foot is on the small mark's stem"
    );
    let outside = landed
        .iter()
        .enumerate()
        .flat_map(|(y, line)| line.iter().enumerate().map(move |(x, g)| (x, y, *g)))
        .filter(|(x, y, glyph)| *glyph != ' ' && !(1..=12).contains(x) | !(0..=6).contains(y))
        .count();
    assert_eq!(
        outside, 0,
        "nothing is left but the name, on the small mark"
    );
    assert_eq!(lit(&at(5.4)), 0, "and then it is over");
}

/// Incident: the small mark showed beside the big one, two logos at once. The orb's image stays
/// unplaced while the title plays and appears when it is over.
#[test]
fn the_small_mark_waits_for_the_big_one_to_land() {
    let placed = |elapsed: Duration| {
        let mut app = app();
        app.set_kitty(true);
        app.set_title(Some((Scene::Echo, elapsed)));
        pane(&mut app);
        let mut out = Vec::new();
        yi_tui::orb::tick(&mut app, &mut out, &mut yi_tui::orb::Tick::default());
        String::from_utf8_lossy(&out).contains("a=p")
    };
    assert!(
        !placed(Duration::from_secs(1)),
        "no second logo while the name plays"
    );
    assert!(placed(title::OVER), "the small mark is there once it ends");
}

/// With the small mark's rows above the composer, the field reaches them too: the pane is full.
#[test]
fn the_field_reaches_the_rows_the_small_mark_would_hold() {
    let mut app = app();
    app.set_kitty(true);
    app.set_title(Some((Scene::Kali, Duration::from_secs(4))));
    let grid = pane(&mut app);
    let top: usize = grid.iter().take(5).flatten().filter(|g| **g != ' ').count();
    assert!(
        top > 100,
        "only {top} cells drawn in the five rows above the composer"
    );
}
