//! The icon component and its drawing rule (record 1.6.20); the full table comes with its task.

use rune::{El, Icon, icon};
use yd_core::tokens::Rgba;

use crate::theme::look::colour;

/// An icon in the record's line style: a 24-unit box, stroke 1.7, round caps and joins, drawn
/// in the colour it is given. `line_icon!("down", r#"<path d="M6 9l6 6 6-6"/>"#)`.
#[macro_export]
macro_rules! line_icon {
    ($name:literal, $body:literal) => {
        ::rune::Icon {
            name: $name,
            view: 24.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        }
    };
}

pub static DOWN: Icon = line_icon!("down", r#"<path d="M6 9l6 6 6-6"/>"#);
pub static SEARCH: Icon = line_icon!(
    "search",
    r#"<circle cx="11" cy="11" r="7"/><path d="M16.5 16.5L21 21"/>"#
);

/// One icon at `size` points in `ink`.
pub fn glyph(drawing: &'static Icon, size: f32, ink: Rgba) -> El {
    icon(drawing, size).color(colour(ink))
}
