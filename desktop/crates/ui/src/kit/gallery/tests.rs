//! The gallery's element tree held to the record's measured numbers (record 1.6), and the theme
//! file loaded without a panic.

use std::error::Error;

use rune::runtime::ElementInfo;
use rune::{Color, Cx, El, Env, Runtime, TextStyle, col};
use yd_core::ids::WindowId;
use yd_core::tokens::{Rgba, ThemeValue, TokenError, Tokens};

use crate::kit::gallery::view;
use crate::theme::look::{colour, white};
use crate::theme::scale::{Fallback, NUMS, YI_DARK, inks, load};
use crate::win::{Hub, Win};

type Check = Result<(), Box<dyn Error>>;

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    view(win, &hub, cx)
}

fn gallery() -> Runtime<Win> {
    let win = Win::new(Hub::new(&[], Vec::new()), WindowId(0));
    let mut runtime = Runtime::new(win, root, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.frame(1.0 / 60.0);
    runtime
}

fn find(all: &[ElementInfo], key: &str) -> Result<ElementInfo, Box<dyn Error>> {
    Ok(all
        .iter()
        .find(|e| e.key.as_deref() == Some(key))
        .ok_or(format!("no element keyed {key}"))?
        .clone())
}

/// The first text drawn inside the element keyed `key`.
fn words(all: &[ElementInfo], key: &str) -> Result<ElementInfo, Box<dyn Error>> {
    let index = all
        .iter()
        .position(|e| e.key.as_deref() == Some(key))
        .ok_or(format!("no element keyed {key}"))?;
    let inside = |mut at: Option<usize>| {
        while let Some(i) = at {
            if i == index {
                return true;
            }
            at = all.get(i).and_then(|e| e.parent);
        }
        false
    };
    Ok(all
        .iter()
        .enumerate()
        .find(|(i, e)| e.text.is_some() && inside(Some(*i)))
        .ok_or(format!("no text in {key}"))?
        .1
        .clone())
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.02
}

fn same(a: Color, b: Rgba) -> bool {
    let b = colour(b);
    [a.r - b.r, a.g - b.g, a.b - b.b, a.a - b.a]
        .iter()
        .all(|d| d.abs() < 0.002)
}

/// Height (and width where the record gives one) of every sized part.
#[test]
fn every_part_has_the_measured_size() -> Check {
    let all = gallery().elements();
    let sizes: &[(&str, Option<f32>, f32)] = &[
        ("kit.button.plain", None, 20.0),
        ("kit.button.primary", None, 20.0),
        ("kit.button.ghost", None, 20.0),
        ("kit.chip.plain", None, 19.0),
        ("kit.chip.menu", None, 19.0),
        ("kit.segmented.first", None, 21.0),
        ("kit.segmented.first.0", None, 17.0),
        ("kit.toggle.off", Some(20.0), 11.0),
        ("kit.toggle.on.knob", Some(8.0), 8.0),
        ("kit.select", None, 20.0),
        ("kit.slider.track", Some(96.0), 4.0),
        ("kit.slider.knob", Some(12.0), 12.0),
        ("kit.hint", None, 14.0),
        ("kit.dot.run", Some(7.0), 7.0),
        ("kit.thread-dot.need", Some(8.0), 8.0),
        ("kit.tag.plain", None, 13.0),
        ("kit.badge.high", None, 13.0),
        ("kit.pill.new", None, 14.0),
        ("kit.meter.low", Some(34.0), 4.0),
        ("kit.bar", Some(30.0), 6.0),
        ("kit.sparkline", None, 9.0),
        ("kit.header", None, 22.0),
        ("kit.section", None, 20.0),
        ("kit.footer", None, 20.0),
        ("kit.hover-card", Some(282.0), 0.0),
    ];
    for (key, width, height) in sizes {
        let [_, _, w, h] = find(&all, key)?.rect;
        assert!(
            *height == 0.0 || near(h, *height),
            "{key} is {h} tall, not {height}"
        );
        if let Some(width) = width {
            assert!(near(w, *width), "{key} is {w} wide, not {width}");
        }
    }
    Ok(())
}

/// Paddings: content 7 in from a button's edge, 6 in a chip, 8 in a header and a section.
#[test]
fn content_sits_at_the_measured_padding() -> Check {
    let all = gallery().elements();
    for (key, pad) in [
        ("kit.button.plain", 7.0),
        ("kit.chip.plain", 6.0),
        ("kit.select", 8.0),
        ("kit.header", 8.0),
        ("kit.section", 8.0),
        ("kit.tag.plain", 4.0),
        ("kit.pill.new", 6.0),
    ] {
        let outer = find(&all, key)?.rect;
        let inner = words(&all, key)?.rect;
        assert!(
            near(inner[0] - outer[0], pad),
            "{key} pads {}, not {pad}",
            inner[0] - outer[0]
        );
    }
    let knob_off =
        find(&all, "kit.toggle.off.knob")?.rect[0] - find(&all, "kit.toggle.off")?.rect[0];
    let knob_on = find(&all, "kit.toggle.on.knob")?.rect[0] - find(&all, "kit.toggle.on")?.rect[0];
    assert!(
        near(knob_off, 1.5) && near(knob_on, 10.5),
        "knob at {knob_off} and {knob_on}"
    );
    Ok(())
}

/// Each part's words are set in the record's size and weight (record 1.3.2, 1.6): their drawn width is the width
/// that style gives, and their line is that style's line.
#[test]
fn words_are_set_in_the_measured_styles() -> Check {
    let all = gallery().elements();
    let styles: &[(&str, TextStyle)] = &[
        ("kit.button.plain", TextStyle::new(10.5, 500.0).lh(14.7)),
        ("kit.chip.plain", TextStyle::new(10.5, 400.0).lh(14.7)),
        (
            "kit.section",
            TextStyle::new(9.5, 500.0).lh(13.3).tracking(0.57),
        ),
        ("kit.tag.plain", TextStyle::new(9.5, 400.0).lh(13.0)),
        (
            "kit.badge.high",
            TextStyle::new(9.5, 600.0).lh(13.0).tracking(0.19),
        ),
        ("kit.pill.new", TextStyle::new(9.5, 400.0).lh(14.0)),
    ];
    for (key, style) in styles {
        let drawn = words(&all, key)?;
        let source = drawn.text.clone().unwrap_or_default();
        let drawn_words = if *key == "kit.section" {
            source.to_uppercase()
        } else {
            source.clone()
        };
        let width = rune::text::advance(&drawn_words, *style);
        assert!(
            (drawn.rect[2] - width).abs() < 0.05,
            "{key}: \"{source}\" is {} wide, {width} in its style",
            drawn.rect[2]
        );
        assert!(
            near(drawn.rect[3], style.line_height),
            "{key}: line {}",
            drawn.rect[3]
        );
    }
    Ok(())
}

/// Fills and inks by role (record 1.1, 1.6), including the decision that done is green.
#[test]
fn parts_are_drawn_in_their_role_colours() -> Check {
    let all = gallery().elements();
    let t = Tokens::default();
    let fills: &[(&str, Rgba)] = &[
        ("kit.button.plain", t.panel),
        ("kit.button.primary", t.fg),
        ("kit.chip.plain", t.raised),
        ("kit.chip.pill", t.accent.alpha(0.18)),
        ("kit.segmented.first", t.raised),
        ("kit.segmented.first.0", white(0.17)),
        ("kit.toggle.off", white(0.16)),
        ("kit.toggle.on", t.ok),
        ("kit.dot.need", t.need),
        ("kit.dot.work", t.accent),
        ("kit.dot.done", t.ok),
        ("kit.dot.idle", t.idle),
        ("kit.dot.stop", t.stopped),
        ("kit.dot.fail", t.bad),
        ("kit.thread-dot.done", t.ok),
        ("kit.badge.high", t.bad.alpha(0.22)),
        ("kit.badge.med", t.warn.alpha(0.16)),
        ("kit.tag.warn", t.need.alpha(0.18)),
        ("kit.pill.proving", t.need.alpha(0.2)),
        ("kit.pill.watching", t.ok.alpha(0.14)),
        ("kit.meter.low.fill", t.meter),
        ("kit.meter.warm.fill", t.need),
        ("kit.meter.hot.fill", t.bad),
        ("kit.bar.heavy", t.bad),
    ];
    for (key, want) in fills {
        let fill = find(&all, key)?.fill;
        assert!(same(fill, *want), "{key} is filled {fill:?}, not {want:?}");
    }
    let inks: &[(&str, Rgba)] = &[
        (
            "kit.button.primary",
            Rgba::parse("#111214").ok_or("bad hex")?,
        ),
        ("kit.button.ghost", t.dim),
        ("kit.chip.pill", t.accent),
        ("kit.section", t.faint),
        ("kit.badge.high", t.bad),
        ("kit.pill.watching", t.ok),
    ];
    for (key, want) in inks {
        let ink = words(&all, key)?.text_color.ok_or("no ink")?;
        assert!(same(ink, *want), "{key} is inked {ink:?}, not {want:?}");
    }
    Ok(())
}

/// A held button presses in and springs back on release.
#[test]
fn a_button_presses_in_and_springs_back() -> Check {
    let mut runtime = gallery();
    let [x, y, w, h] = find(&runtime.elements(), "kit.button.plain")?.rect;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    runtime.pointer_move(cx, cy);
    runtime.pointer_down(cx, cy);
    for _ in 0..30 {
        runtime.frame(1.0 / 60.0);
    }
    let held = find(&runtime.elements(), "kit.button.plain")?.scale;
    assert!(held < 0.97, "held button at scale {held}");
    runtime.pointer_up(cx, cy);
    for _ in 0..90 {
        runtime.frame(1.0 / 60.0);
    }
    let back = find(&runtime.elements(), "kit.button.plain")?.scale;
    assert!(near(back, 1.0), "released button at scale {back}");
    Ok(())
}

/// Yi Dark as a file is exactly the default tokens and every named extra's default.
#[test]
fn the_yi_dark_file_is_the_default_set() -> Check {
    let loaded = load(YI_DARK);
    assert!(
        loaded.fell_back.is_empty(),
        "fell back: {:?}",
        loaded.fell_back
    );
    let mut table = loaded.tokens.clone();
    table.extra.clear();
    assert_eq!(table, Tokens::default());
    for num in NUMS {
        assert_eq!(
            loaded.tokens.extra.get(num.0),
            Some(&ThemeValue::Number(f64::from(num.1)))
        );
    }
    for ink in inks() {
        assert_eq!(
            ink.of(&loaded.tokens),
            ink.1,
            "{} differs from the file",
            ink.0
        );
        assert!(
            loaded.tokens.extra.contains_key(ink.0),
            "{} is not in the file",
            ink.0
        );
    }
    let named = Tokens::NAMES.len() + NUMS.len() + inks().count();
    assert_eq!(
        Tokens::default().values().len() + loaded.tokens.extra.len(),
        named
    );
    Ok(())
}

/// A bad file never panics: each bad value keeps Yi Dark's and is named; unreadable text keeps all.
#[test]
fn a_bad_theme_falls_back_by_name() -> Check {
    let loaded = load(
        r##"{"id": "x", "name": "X", "dark": true,
            "values": {"accent": "blue", "radius.button": "6", "fill.card": 3, "ok": "#00ff00"}}"##,
    );
    let refused: Vec<(&str, TokenError)> = loaded
        .fell_back
        .iter()
        .filter_map(|f| match f {
            Fallback::Refused { name, why } => Some((name.as_str(), *why)),
            Fallback::Unreadable(_) => None,
        })
        .collect();
    assert_eq!(refused.len(), 3, "{refused:?}");
    assert!(refused.contains(&("accent", TokenError::NotAColour)));
    assert!(refused.contains(&("radius.button", TokenError::NotANumber)));
    assert!(refused.contains(&("fill.card", TokenError::NotAColour)));
    assert_eq!(loaded.tokens.accent, Tokens::default().accent);
    assert_eq!(crate::theme::scale::RADIUS_BUTTON.of(&loaded.tokens), 6.0);
    assert_eq!(loaded.tokens.ok, yd_core::tokens::rgb(0, 255, 0));
    let junk = load("not a theme");
    assert!(matches!(
        junk.fell_back.as_slice(),
        [Fallback::Unreadable(_)]
    ));
    assert_eq!(junk.tokens, Tokens::default());
    Ok(())
}
