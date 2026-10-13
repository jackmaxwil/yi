//! The title animation: a function of a screen's size, a scene and the seconds since the console
//! started, drawn over every cell of the screen and over in 3 s.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::Cell;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::title::{self, Scene};

use crate::common;

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

/// A screen the TUI has already drawn, every cell an `X`, then the cover over it.
fn screen(
    which: Scene,
    (width, height): (u16, u16),
    secs: f64,
    landing: Option<Rect>,
) -> Vec<Vec<char>> {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::filled(area, ratatui::buffer::Cell::new("X"));
    let elapsed = Duration::from_secs_f64(secs);
    let cache = &mut title::Cache::default();
    title::cover(
        &mut buffer,
        area,
        (which, elapsed),
        landing,
        &theme(),
        cache,
    );
    rows(&buffer, area)
}

fn count(glyphs: &[Vec<char>], want: impl Fn(char) -> bool) -> usize {
    glyphs.iter().flatten().filter(|g| want(**g)).count()
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

/// Whatever the TUI drew under it, the cover replaces: not one `X` survives while it holds.
/// Row 14 of 24 and column 35 of 80 hold the point (-0.1875, -0.208), 0.0025 from the Y stem's
/// line; row 29 of 50 and column 90 of 200 hold (-0.19, -0.18), on it.
#[test]
fn the_cover_replaces_every_cell_while_it_holds() {
    for (size, stem) in [((80, 24), (35, 14)), ((200, 50), (90, 29))] {
        let held = screen(Scene::Kali, size, 2.1, None);
        assert_eq!(
            count(&held, |g| g == 'X'),
            0,
            "{size:?}: the TUI shows through"
        );
        assert_eq!(held[stem.1][stem.0], '@', "{size:?}: the Y stem is solid");
    }
}

#[test]
fn the_echo_stem_is_solid_at_both_sizes() {
    for (width, height, col, row) in [(80, 24, 35, 14), (200, 50, 90, 29)] {
        let glyphs = screen(Scene::Echo, (width, height), 0.0, None);
        assert_eq!(
            glyphs[row][col], '@',
            "{width}x{height} row {row} col {col}"
        );
    }
}

/// The echo's value is `ring² · e^(-0.9 d)`, at most 0.26 past `d = 1.5`, which rounds to step 2:
/// a cell that far from the name never draws past `:`.
#[test]
fn far_cells_stay_faint_at_every_time() {
    for secs in [0.0, 0.4, 0.9, 1.4] {
        let glyphs = screen(Scene::Echo, (120, 40), secs, None);
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
    let at = |secs: f64| screen(Scene::Echo, (80, 24), secs, None);
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
    let moved = changed(&at(start), &at(start + ring / 4.0));
    assert!(moved > 100, "a quarter period moved only {moved} cells");
}

/// At the first instant the fold is faded out and the name has not risen: an empty cover.
#[test]
fn the_fold_opens_from_nothing_and_fills_the_screen() {
    let first = screen(Scene::Kali, (80, 24), 0.0, None);
    assert_eq!(
        count(&first, |g| g != ' '),
        0,
        "nothing drawn, and nothing of the TUI left"
    );
    let open = screen(Scene::Kali, (80, 24), 1.5, None);
    assert!(
        count(&open, |g| g != ' ') > 600,
        "the fold fills the screen"
    );
}

#[test]
fn a_screen_with_no_room_draws_nothing_and_does_not_panic() {
    for (width, height) in [(0, 0), (0, 10), (10, 0), (1, 1), (1, 40), (300, 1)] {
        screen(Scene::Echo, (width, height), 2.0, None);
        screen(
            Scene::Kali,
            (width, height),
            2.6,
            Some(Rect::new(0, 0, 10, 5)),
        );
    }
}

/// The small mark's cells: 10 by 5 at column 2, row 1. In `MARK_STROKES` the Y's stem is the line
/// x = -0.28, which the orb draws at (32 - 0.28 * 1.0256 * 24.96) / 64 = 0.388 of the image's
/// width, so column 2 + 3.88 = 5.88 and cell 5; its foot y = -0.62 is at 0.748 of the height,
/// row 1 + 3.74 = 4.74 and cell 4.
#[test]
fn the_name_lands_on_the_small_mark_and_the_screen_comes_back() {
    let landing = Rect::new(2, 1, 10, 5);
    for which in [
        Scene::Kali,
        Scene::Solid,
        Scene::Mobius,
        Scene::Clouds,
        Scene::Growth,
        Scene::Spirals,
        Scene::Attractor,
    ] {
        let at = |secs: f64| screen(which, (80, 24), secs, Some(landing));
        let held = at(2.1);
        assert_eq!(
            count(&held, |g| g == 'X'),
            0,
            "{which:?}: covered whole until the landing"
        );
        assert!(
            !" .:".contains(held[14][35]),
            "{which:?}: the name is in the middle"
        );

        let landed = at(2.99);
        assert!(
            !" .:X".contains(landed[4][5]),
            "{which:?}: the stem's foot is on the small mark"
        );
        let strays = landed
            .iter()
            .enumerate()
            .flat_map(|(y, line)| line.iter().enumerate().map(move |(x, g)| (x, y, *g)))
            .filter(|(x, y, g)| *g != 'X' && !((1..=12).contains(x) && (0..=6).contains(y)))
            .count();
        assert_eq!(
            strays, 0,
            "{which:?}: the TUI is back but for the name, on the small mark"
        );

        let over = at(3.0);
        assert_eq!(
            count(&over, |g| g != 'X'),
            0,
            "{which:?}: and then it is over"
        );
    }
}

/// The solid name is sphere-traced: it is absent before it has any thickness, lit once it has,
/// and a camera that swings once moves it between frames.
#[test]
fn the_solid_name_rises_swings_and_shows_a_lit_side() {
    let at = |secs: f64| screen(Scene::Solid, (80, 24), secs, None);
    let flat = at(0.1);
    assert_eq!(
        count(&flat, |g| g == '@' || g == '%'),
        0,
        "no solid yet at the first instant"
    );
    let held = at(1.8);
    assert_eq!(
        count(&held, |g| g == 'X'),
        0,
        "it covers the screen like the others"
    );
    let lit = held
        .iter()
        .flatten()
        .filter(|g| matches!(**g, '*' | '#' | '%' | '@'))
        .count();
    assert!(lit > 150, "only {lit} cells are on its lit side");
    let moved = at(1.0)
        .iter()
        .flatten()
        .zip(at(1.6).iter().flatten())
        .filter(|(a, b)| a != b)
        .count();
    assert!(moved > 60, "the swing moved only {moved} cells");
}

#[test]
fn the_screen_comes_back_a_few_cells_at_a_time() {
    let area = 80.0 * 24.0;
    let back = |secs: f64| {
        let glyphs = screen(Scene::Kali, (80, 24), secs, Some(Rect::new(2, 1, 10, 5)));
        count(&glyphs, |g| g == 'X') as f64 / area
    };
    let (early, mid, late) = (back(2.4), back(2.6), back(2.8));
    assert!(early < mid && mid < late, "{early:.2} {mid:.2} {late:.2}");
    assert!(early < 0.35 && late > 0.65, "{early:.2} {late:.2}");
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
    assert_eq!(Scene::named("solid"), Some(Scene::Solid));
    assert_eq!(Scene::named("mobius"), Some(Scene::Mobius));
    assert_eq!(Scene::named("clouds"), Some(Scene::Clouds));
    assert_eq!(Scene::named("growth"), Some(Scene::Growth));
    assert_eq!(Scene::named("spirals"), Some(Scene::Spirals));
    assert_eq!(Scene::named("attractor"), Some(Scene::Attractor));
    assert_eq!(Scene::named("Kali"), None);
    assert_eq!(Scene::named(""), None);
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

/// Incident: the small mark showed beside the big one, two logos at once. While the title covers
/// the screen the orb's image stays unplaced, and its cells are still known so the name can land.
#[test]
fn the_small_mark_waits_for_the_big_one_to_land() {
    let placed = |cover: bool| {
        let mut app = app();
        app.set_kitty(true);
        app.set_title_cover(cover);
        let area = Rect::new(0, 0, 80, 30);
        let mut buffer = Buffer::empty(area);
        let mut scroll = 0;
        yi_tui::render::paint_pane(&mut app, None, &mut buffer, area, &mut scroll);
        let mut out = Vec::new();
        yi_tui::orb::tick(&mut app, &mut out, &mut yi_tui::orb::Tick::default());
        (
            String::from_utf8_lossy(&out).contains("a=p"),
            app.orb_rect(),
        )
    };
    let (covered, slot) = placed(true);
    assert!(!covered, "no second logo while the name plays");
    assert_eq!(
        slot,
        Some(Rect::new(0, 0, 10, 5)),
        "but the landing cells are known"
    );
    assert!(placed(false).0, "the small mark is there once it ends");
}

#[test]
fn the_first_message_makes_the_session_not_idle() {
    let mut app = app();
    assert!(app.title_idle());
    app.commit_cell(&Cell::User {
        text: "hello".to_owned(),
    });
    assert!(!app.title_idle());
}

/// The vortex is the name sampled through a flow that is gone by 2 s: the first frame is not the
/// held frame, and the held frame has the name where every other scene has it.
#[test]
fn the_mobius_name_comes_out_of_a_vortex_and_settles() {
    let at = |secs: f64| screen(Scene::Mobius, (80, 24), secs, None);
    let (first, held) = (at(0.1), at(2.1));
    assert_eq!(
        count(&first, |g| g == 'X'),
        0,
        "the vortex covers the screen too"
    );
    let moved = first
        .iter()
        .flatten()
        .zip(held.iter().flatten())
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        moved > 300,
        "the vortex moved only {moved} cells by the hold"
    );
    assert_eq!(held[14][35], '@', "the Y stem is where the others put it");
}

/// The weather fades up from nothing, drifts, and the name rises through it as in the fold.
#[test]
fn the_clouds_fade_up_drift_and_let_the_name_through() {
    let at = |secs: f64| screen(Scene::Clouds, (80, 24), secs, None);
    assert_eq!(
        count(&at(0.0), |g| g != ' '),
        0,
        "nothing at the first instant"
    );
    let (a, b) = (at(1.0), at(1.5));
    assert!(
        count(&a, |g| g != ' ') > 500,
        "the weather fills the screen"
    );
    let moved = a
        .iter()
        .flatten()
        .zip(b.iter().flatten())
        .filter(|(x, y)| x != y)
        .count();
    assert!(
        moved > 200,
        "the drift moved only {moved} cells in half a second"
    );
    assert_eq!(at(2.1)[14][35], '@', "and the Y stem is solid through it");
}

/// The coral is a simulation the cache advances to the second asked for: it grows out of the
/// strokes, a later frame continues the same run, and an earlier second starts it over.
#[test]
fn the_growth_spreads_from_the_strokes_and_the_cache_keeps_its_place() {
    let area = Rect::new(0, 0, 80, 24);
    let draw = |cache: &mut title::Cache, secs: f64| {
        let mut buffer = Buffer::filled(area, ratatui::buffer::Cell::new("X"));
        let at = (Scene::Growth, Duration::from_secs_f64(secs));
        title::cover(&mut buffer, area, at, None, &theme(), cache);
        rows(&buffer, area)
    };
    let mut cache = title::Cache::default();
    let early = count(&draw(&mut cache, 0.4), |g| g != ' ');
    let late = count(&draw(&mut cache, 1.6), |g| g != ' ');
    assert!(
        late > early + 200,
        "the coral did not spread: {early} then {late}"
    );
    let again = draw(&mut cache, 1.6);
    assert_eq!(
        again,
        draw(&mut title::Cache::default(), 1.6),
        "a kept run equals a fresh one"
    );
    assert_eq!(
        draw(&mut cache, 0.4),
        draw(&mut title::Cache::default(), 0.4),
        "back in time starts over"
    );
}

/// The cyclic automaton starts as noise and the cache steps it at 55 a second: the screen
/// changes while it runs, a kept run equals a fresh one, and a second that runs back starts over.
#[test]
fn the_spirals_turn_and_the_cache_keeps_its_place() {
    let area = Rect::new(0, 0, 80, 24);
    let draw = |cache: &mut title::Cache, secs: f64| {
        let mut buffer = Buffer::filled(area, ratatui::buffer::Cell::new("X"));
        let at = (Scene::Spirals, Duration::from_secs_f64(secs));
        title::cover(&mut buffer, area, at, None, &theme(), cache);
        rows(&buffer, area)
    };
    let mut cache = title::Cache::default();
    let early = draw(&mut cache, 0.4);
    let late = draw(&mut cache, 1.0);
    let moved = early
        .iter()
        .flatten()
        .zip(late.iter().flatten())
        .filter(|(a, b)| a != b)
        .count();
    assert!(moved > 500, "the spirals moved only {moved} cells in 0.6 s");
    assert_eq!(
        late,
        draw(&mut title::Cache::default(), 1.0),
        "a kept run equals a fresh one"
    );
    assert_eq!(draw(&mut cache, 0.4), early, "back in time starts over");
}

/// The Clifford orbit with `c = 1`, `d = 0.7` cannot leave |x| < 2, |y| < 1.7, and the screen maps
/// that box to its middle 95%: the outer 2.5% (row 0, row 39, columns 0 to 2) never holds a point
/// at any time, however `a` and `b` have drifted, and the shape itself moves.
#[test]
fn the_attractor_stays_inside_its_box_and_drifts() {
    let (width, height) = (120_u16, 40_u16);
    let frame = |secs: f64| screen(Scene::Attractor, (width, height), secs, None);
    for secs in [0.1, 0.9, 1.1] {
        let glyphs = frame(secs);
        assert!(
            count(&glyphs, |g| g != ' ') > 300,
            "t {secs}: nothing drawn"
        );
        let edge = |x: usize, y: usize| {
            y == 0 || y == usize::from(height) - 1 || x < 3 || x >= usize::from(width) - 3
        };
        let strays = glyphs
            .iter()
            .enumerate()
            .flat_map(|(y, line)| line.iter().enumerate().map(move |(x, g)| (x, y, *g)))
            .filter(|(x, y, g)| *g != ' ' && edge(*x, *y))
            .count();
        assert_eq!(strays, 0, "t {secs}: the orbit left its box");
    }
    let (first, later) = (frame(0.1), frame(1.1));
    let moved = first
        .iter()
        .flatten()
        .zip(later.iter().flatten())
        .filter(|(a, b)| a != b)
        .count();
    assert!(moved > 200, "a second of drift moved only {moved} cells");
}
