//! The tab bar a mockup frame draws round a pane, for this task's parity scenes only: the
//! real pane chrome is the grid view's, and these scenes take it once it lands.

use rune::{El, col, row, spacer, text};
use yd_core::tokens::Tokens;

use yd_ui::theme::look::{LABEL, META, NONE, colour};
use yd_ui::theme::scale::{LINE_BAR, TAB_BAR, TAB_ON};

/// `body` under a 20 tall bar of `tabs` (title and count), the first on, and `meta`.
pub fn framed(tokens: &Tokens, key: &str, tabs: &[(&str, &str)], meta: &str, body: El) -> El {
    let tab = |(index, (title, count)): (usize, &(&str, &str))| {
        let (fill, ink) = if index == 0 {
            (TAB_ON.of(tokens), tokens.fg)
        } else {
            (NONE, tokens.faint)
        };
        let key = if index == 0 {
            format!("{key}.pane-tab")
        } else {
            format!("{key}.tab.{index}")
        };
        row()
            .key(key)
            .h(19.0)
            .px(5.0)
            .gap(4.0)
            .center_y()
            .radius(4.0)
            .fill(colour(fill))
            .child(text(*title).font(LABEL).color(colour(ink)))
            .children(
                (!count.is_empty()).then(|| text(*count).font(LABEL).color(colour(tokens.faint))),
            )
    };
    let bar = row()
        .key(format!("{key}.pane-tab-bar"))
        .w_full()
        .h(20.0)
        .center_y()
        .fill(colour(TAB_BAR.of(tokens)))
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(LINE_BAR.of(tokens)))
        .children(tabs.iter().enumerate().map(tab))
        .child(text(meta).font(META).color(colour(tokens.faint)).pl(8.0))
        .child(spacer());
    col()
        .w_full()
        .h_full()
        .child(bar)
        .child(col().w_full().grow(1.0).min_h(0.0).child(body))
}
