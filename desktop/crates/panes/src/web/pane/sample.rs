//! The mockup's page as words over a blank screen, for scenes: no recorded picture is kept.

use rune::{El, TextStyle, col, el, row, spacer, text};
use yd_core::tokens::rgb;
use yd_ui::theme::look::colour;

/// The page's own words, in the order the mockup draws them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sample {
    /// The site's name, its sections, the one in use, and who is signed in.
    pub nav: Vec<String>,
    pub alert: String,
    /// The alert's fill and words as `#rrggbb` and its corner radius, where a scene changes
    /// them from the page's own.
    pub look: Option<(String, String, u32)>,
    pub drop: String,
    pub button: String,
}

const DARK: yd_core::tokens::Rgba = rgb(0x1a, 0x1b, 0x1d);
const DIM: yd_core::tokens::Rgba = rgb(0x5a, 0x5d, 0x63);
const SMALL: TextStyle = TextStyle::new(11.5, 400.0).lh(15.0);

/// The page at its own sizes (record 4.5): white, the site's bar, an alert and a drop zone.
pub fn page(sample: &Sample, key: &str) -> El {
    let word = |words: &str, weight: f32, size: f32, ink| {
        text(words)
            .font(TextStyle::new(size, weight).lh(15.0))
            .color(colour(ink))
    };
    let (brand, links) = sample
        .nav
        .split_first()
        .map_or((None, &[][..]), |(b, l)| (Some(b), l));
    let (signed, links) = links
        .split_last()
        .map_or((None, links), |(s, l)| (Some(s), l));
    let last = links.len().saturating_sub(1);
    let nav = row()
        .key(format!("{key}.page.nav"))
        .w_full()
        .h(28.0)
        .px(14.0)
        .gap(14.0)
        .center_y()
        .clip_rounded(0.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(rgb(0xe6, 0xe6, 0xe2)))
        .children(brand.map(|b| word(b, 600.0, 12.5, DARK)))
        .children(links.iter().enumerate().map(|(i, link)| {
            let current = i == last;
            word(
                link,
                if current { 600.0 } else { 400.0 },
                11.5,
                if current { DARK } else { DIM },
            )
        }))
        .child(spacer())
        .children(signed.map(|s| word(s, 400.0, 11.5, DIM)));
    let own = (rgb(0xfd, 0xec, 0xec), rgb(0x9c, 0x24, 0x24), 6.0);
    let (fill, ink, round) = sample.look.as_ref().map_or(own, |(fill, ink, round)| {
        let hue = |hex: &str, or| yd_core::tokens::Rgba::parse(hex).unwrap_or(or);
        (hue(fill, own.0), hue(ink, own.1), *round as f32)
    });
    let alert = col()
        .key(format!("{key}.page.alert"))
        .w_full()
        .px(10.0)
        .py(7.0)
        .radius(round)
        .fill(colour(fill))
        .child(word(&sample.alert, 500.0, 11.5, ink).wrap());
    let button = row()
        .px(9.0)
        .py(3.0)
        .radius(6.0)
        .fill(colour(DARK))
        .child(word(&sample.button, 500.0, 11.5, rgb(0xff, 0xff, 0xff)));
    let drop = row()
        .key(format!("{key}.page.drop"))
        .w_full()
        .h(60.0)
        .gap(8.0)
        .center()
        .radius(8.0)
        .border(1.5, colour(rgb(0xc9, 0xc9, 0xc4)))
        .child(text(sample.drop.as_str()).font(SMALL).color(colour(DIM)))
        .child(button);
    col()
        .key(format!("{key}.page.words"))
        .w_full()
        .h_full()
        .child(nav)
        .child(
            col()
                .w_full()
                .px(16.0)
                .py(12.0)
                .gap(10.0)
                .child(alert)
                .child(drop),
        )
        .child(el().grow(1.0))
}
