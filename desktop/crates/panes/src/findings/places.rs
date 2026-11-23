//! Findings by place: each place's open findings as a bar; pressing one filters the list.

use rune::{Cursor, El, col, el, row, text};
use yd_core::address::uri::Address;
use yd_core::findings::view::Place;
use yd_ui::kit::bars::footer;
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{META, colour};
use yd_ui::win::Win;

use crate::findings::hub::title;
use crate::findings::pane::View;
use crate::findings::pane::fold::place_label;

/// How much of the track one finding fills (record 6.6.4: the count times 30 %).
const SHARE: f32 = 0.3;

/// Picks `place`, or drops the pick when it is the one picked.
fn pick(win: &mut Win, pane: yd_core::ids::PaneId, place: &Address) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(view) = hub.panes.get_mut::<View>(Some(pane))
    {
        let again = view.place.as_ref() == Some(place);
        view.place = (!again).then(|| place.clone());
    }
}

pub fn view(pane: &PaneCx<'_>, key: &str, places: &[Place], picked: Option<&Address>) -> El {
    let tokens = pane.tokens();
    let at = pane.pane;
    let bar = |place: &Place| {
        let name = place_label(&place.place);
        let fill = (SHARE * place.count as f32).min(1.0);
        let ink = if place.high { tokens.bad } else { tokens.dim };
        let lit = picked == Some(&place.place);
        let address = place.place.clone();
        row()
            .key(format!("{key}.place.{name}"))
            .w_full()
            .h(20.0)
            .px(8.0)
            .gap(8.0)
            .center_y()
            .fill(colour(if lit {
                tokens.selected
            } else {
                tokens.selected.alpha(0.0)
            }))
            .cursor(Cursor::Pointer)
            .on_press(move |win: &mut Win| pick(win, at, &address))
            .child(
                text(name)
                    .font(META.mono())
                    .color(colour(tokens.dim))
                    .truncate()
                    .w(112.0),
            )
            .child(
                row()
                    .grow(1.0)
                    .min_w(0.0)
                    .child(el().h(6.0).radius(3.0).fill(colour(ink)).grow(fill))
                    .child(el().grow(1.0 - fill)),
            )
            .child(
                text(place.count.to_string())
                    .font(META)
                    .color(colour(tokens.dim))
                    .w(12.0),
            )
    };
    let meta = match places.len() {
        1 => "1 place".to_owned(),
        n => format!("{n} places"),
    };
    col()
        .key(format!("{key}.places"))
        .w_full()
        .child(title(
            tokens,
            &format!("{key}.places-title"),
            "By place",
            &meta,
        ))
        .child(col().w_full().py(2.0).children(places.iter().map(bar)))
        .child(
            footer(
                tokens,
                vec![
                    text("Click a place to show only its findings")
                        .font(META)
                        .color(colour(tokens.faint)),
                ],
                Vec::new(),
            )
            .key(format!("{key}.places.footer")),
        )
}
