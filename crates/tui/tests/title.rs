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
    title::cover(&mut buffer, area, (which, elapsed), landing, &theme());
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
    let at = |secs: f64| screen(Scene::Kali, (80, 24), secs, Some(landing));
    let held = at(2.1);
    assert_eq!(
        count(&held, |g| g == 'X'),
        0,
        "covered whole until the landing starts"
    );
    assert_eq!(held[14][35], '@', "the name is in the middle");

    let landed = at(2.99);
    assert_eq!(
        landed[4][5], '@',
        "the stem's foot is on the small mark's stem"
    );
    let strays = landed
        .iter()
        .enumerate()
        .flat_map(|(y, line)| line.iter().enumerate().map(move |(x, g)| (x, y, *g)))
        .filter(|(x, y, glyph)| *glyph != 'X' && !((1..=12).contains(x) && (0..=6).contains(y)))
        .count();
    assert_eq!(
        strays, 0,
        "the TUI is back everywhere but the name, on the small mark"
    );

    let over = at(3.0);
    assert_eq!(count(&over, |g| g != 'X'), 0, "and then it is over");
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
