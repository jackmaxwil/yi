//! The mockup's report as words and bars over a blank screen, for scenes: no picture is kept.

use rune::{El, TextStyle, col, el, row, spacer, text};
use yd_core::tokens::{Rgba, rgb};
use yd_ui::theme::look::colour;

/// The report's own words and its chart's counts, in the order the mockup draws them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sample {
    pub heading: String,
    pub paragraph: String,
    /// Under the chart: where it starts, where the bucket refills, where refusals begin.
    pub axis: [String; 3],
    /// The two legend lines: what got through, what was turned away.
    pub legend: [String; 2],
    pub passed: usize,
    pub refused: usize,
}

const OK: Rgba = rgb(0x2f, 0x9a, 0x57);
const REFUSED: Rgba = rgb(0xd2, 0x45, 0x45);
const DIM: Rgba = rgb(0x5a, 0x5d, 0x63);
/// Uploads sent before the bucket refills (record 4.7): the refused ones end the minute.
const FIRST_MINUTE: usize = 30;

/// The leading count in a legend line: 28 of "200 OK, 28 uploads".
pub fn count(line: &str) -> usize {
    let after = line.split(", ").nth(1).unwrap_or_default();
    after
        .split(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// The page at its 600 point design width, drawn at `scale`.
pub fn page(sample: &Sample, key: &str, scale: f32) -> El {
    let at = |v: f32| v * scale;
    let words = |words: &str, size: f32, weight: f32, ink: Rgba| {
        let style = TextStyle::new(at(size), weight).lh(at(size * 1.35));
        text(words).font(style).color(colour(ink))
    };
    let bar = |high: f32, ink: Rgba| el().w(at(10.0)).h(at(high)).fill(colour(ink));
    let bars = (0..sample.passed.saturating_add(sample.refused)).map(|i| {
        let refusing = FIRST_MINUTE.saturating_sub(sample.refused)..FIRST_MINUTE;
        let passed = !refusing.contains(&i);
        if passed {
            bar(70.0, OK)
        } else {
            bar(26.0, REFUSED)
        }
    });
    let chart = row()
        .key(format!("{key}.page.chart"))
        .gap(at(4.0))
        .items_end()
        .children(bars);
    let axis = row()
        .w_full()
        .child(words(&sample.axis[0], 9.0, 400.0, DIM))
        .child(spacer())
        .child(words(&sample.axis[2], 9.0, 400.0, REFUSED))
        .child(spacer())
        .child(words(&sample.axis[1], 9.0, 400.0, DIM));
    let legend = |line: &str, ink: Rgba| {
        row()
            .gap(at(5.0))
            .center_y()
            .child(el().w(at(8.0)).h(at(8.0)).fill(colour(ink)))
            .child(words(line, 9.0, 400.0, DIM))
    };
    col()
        .key(format!("{key}.page.words"))
        .w(at(600.0))
        .px(at(24.0))
        .py(at(20.0))
        .gap(at(10.0))
        .child(words(&sample.heading, 19.0, 650.0, rgb(0x1a, 0x1b, 0x1d)))
        .child(words(&sample.paragraph, 13.0, 400.0, DIM).wrap())
        .child(chart)
        .child(axis)
        .child(
            row()
                .gap(at(16.0))
                .child(legend(&sample.legend[0], OK))
                .child(legend(&sample.legend[1], REFUSED)),
        )
}
