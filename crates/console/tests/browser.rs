use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use yi_console::app::MouseKind;
use yi_console::browser::{Clicks, css_point, css_size, key_inputs, last_local_url, wheel};
use yi_types::cdp::{KeyKind, MouseKind as CdpMouse};

const CELL: (usize, usize) = (10, 20);

#[test]
fn a_cell_maps_to_its_centre_in_css_pixels_and_off_the_page_to_nothing() {
    let page = Rect::new(30, 5, 40, 10);
    let cases = [
        ((30, 5), Some((5, 10))),
        ((31, 6), Some((15, 30))),
        ((69, 14), Some((395, 190))),
        ((70, 14), None),
        ((29, 5), None),
        ((30, 4), None),
    ];
    for ((x, y), want) in cases {
        assert_eq!(css_point(page, CELL, x, y), want, "cell ({x}, {y})");
    }
    assert_eq!(css_size(page, CELL), (400, 200));
}

#[test]
fn presses_count_on_the_same_cell_inside_the_window_and_stop_at_three() {
    let start = Instant::now();
    let mut clicks = Clicks::default();
    let at = |ms| start + Duration::from_millis(ms);
    let presses = [
        ((1, 1), at(0), 1),
        ((1, 1), at(200), 2),
        ((1, 1), at(400), 3),
        ((1, 1), at(600), 3),
        ((1, 1), at(1100), 1),
        ((2, 1), at(1200), 1),
    ];
    for (cell, now, want) in presses {
        assert_eq!(clicks.press(cell, now), want, "{cell:?} at {now:?}");
    }
}

#[test]
fn a_wheel_notch_scrolls_three_cell_rows_of_pixels() {
    let up = wheel(MouseKind::ScrollUp, (5, 10), CELL);
    let down = wheel(MouseKind::ScrollDown, (5, 10), CELL);
    assert_eq!(
        (up.kind, up.delta_y, up.x, up.y),
        (CdpMouse::MouseWheel, -60, 5, 10)
    );
    assert_eq!(down.delta_y, 60);
}

#[test]
fn a_typed_key_types_and_a_named_key_does_not() {
    let typed = key_inputs(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    let shown: Vec<_> = typed
        .iter()
        .map(|k| {
            (
                k.kind,
                k.key.as_str(),
                k.text.as_deref(),
                k.windows_virtual_key_code,
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            (KeyKind::KeyDown, "a", Some("a"), 65),
            (KeyKind::KeyUp, "a", None, 65)
        ]
    );
    let held = key_inputs(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(
        held.first().map(|k| (k.text.clone(), k.modifiers)),
        Some((None, 2))
    );
    let left = key_inputs(&KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(
        left.first()
            .map(|k| (k.key.as_str(), k.windows_virtual_key_code, k.modifiers)),
        Some(("ArrowLeft", 37, 8))
    );
}

#[test]
fn the_last_local_url_in_the_transcript_opens() {
    let sources = [
        "Serving at http://localhost:3000/ now",
        "see https://example.com and then http://127.0.0.1:8000/docs.",
        "nothing here",
    ];
    assert_eq!(
        last_local_url(sources.into_iter()),
        Some("http://127.0.0.1:8000/docs".to_owned())
    );
    assert_eq!(last_local_url(["http://localhostx"].into_iter()), None);
}
