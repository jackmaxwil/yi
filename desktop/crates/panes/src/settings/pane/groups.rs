//! The left column: the search box, the groups with how many of their settings differ from
//! the defaults, and the footer.

use rune::{Cursor, El, KeyEvent, Tx, col, el, row, spacer, text, text_input};
use yd_core::ids::PaneId;
use yd_core::settings::schema::Setting;
use yd_core::settings::store::browse;
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::kit::bars::footer;
use yd_ui::kit::icons::{SEARCH, glyph};
use yd_ui::kit::key_hint::key_hint;
use yd_ui::theme::look::{BODY, LABEL, META, NONE, colour};
use yd_ui::theme::scale::LINE_PICKED;
use yd_ui::win::Win;

use super::act;
use super::state::{Part, State, Tab};

pub const WIDTH: f32 = 190.0;

pub struct Drawn<'a> {
    pub pane: PaneId,
    pub key: &'a str,
    pub tokens: &'a Tokens,
    pub rows: &'a [Setting],
    pub state: &'a State,
    /// The search field to draw: [`super::fields::drawn`].
    pub search: &'a rune::TextEdit,
    /// The key that shows Settings, as the map has it.
    pub hint: String,
}

fn search_box(d: &Drawn<'_>) -> El {
    let (tokens, pane) = (d.tokens, d.pane);
    let hint = format!("Search {} settings", d.rows.len());
    let field = text_input(d.search, LABEL)
        .single_line()
        .key(format!("{}.search.input", d.key))
        .grow(1.0)
        .min_w(0.0)
        .color(colour(tokens.fg))
        .placeholder(&hint, colour(tokens.faint))
        .label(&hint)
        .caret(colour(tokens.accent), a11y::selection(tokens))
        .on_edit(move |win: &mut Win| act::edit(win, pane, Part::Search))
        .on_input_change(move |win: &mut Win, _: &mut Tx| act::searched(win, pane))
        .on_input_key(move |win: &mut Win, event: &KeyEvent, _: &mut Tx| {
            act::search_key(win, pane, event)
        });
    row()
        .key(format!("{}.search", d.key))
        .w_full()
        .px(3.0)
        .py(3.0)
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .h(22.0)
                .px(7.0)
                .gap(6.0)
                .center_y()
                .radius(6.0)
                .fill(colour(tokens.raised))
                .child(glyph(&SEARCH, 11.0, tokens.faint))
                .child(field)
                .child(key_hint(tokens, &d.hint)),
        )
}

fn group_row(d: &Drawn<'_>, name: &str, changed: usize, on: bool) -> El {
    let (tokens, pane) = (d.tokens, d.pane);
    let ink = if on { tokens.fg } else { tokens.dim };
    let named = name.to_owned();
    let count = (changed > 0).then(|| {
        text(changed.to_string())
            .font(META)
            .color(colour(tokens.faint))
    });
    row()
        .key(format!("{}.group.{name}", d.key))
        .w_full()
        .h(tokens.row)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .fill(colour(if on { tokens.selected } else { NONE }))
        .cursor(Cursor::Pointer)
        .children(on.then(|| {
            el().absolute(0.0, 0.0)
                .size(2.0, tokens.row)
                .fill(colour(LINE_PICKED.of(tokens)))
        }))
        .child(text(name).font(BODY).color(colour(ink)))
        .child(spacer())
        .children(count)
        .on_press(move |win: &mut Win| act::group(win, pane, &named))
}

pub fn view(d: &Drawn<'_>) -> El {
    let tokens = d.tokens;
    let front = (d.state.tab == Tab::Group && d.state.search.trim().is_empty())
        .then(|| d.state.group_in(d.rows))
        .flatten();
    let groups = browse::groups(d.rows);
    let list = col()
        .key(format!("{}.groups", d.key))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .px(3.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .children(groups.iter().map(|group| {
            let on = front.as_deref() == Some(group.name.as_str());
            group_row(d, &group.name, group.changed, on)
        }));
    let pane = d.pane;
    let changed = browse::changed(d.rows);
    let only = text("Show only changed")
        .font(LABEL)
        .color(colour(tokens.dim));
    let bar = footer(
        tokens,
        vec![
            text(format!("{changed} changed"))
                .font(LABEL)
                .color(colour(tokens.faint)),
        ],
        vec![
            row()
                .key(format!("{}.only-changed", d.key))
                .cursor(Cursor::Pointer)
                .child(only)
                .on_press(move |win: &mut Win| act::tab(win, pane, Tab::Changed.id())),
        ],
    )
    .key(format!("{}.footer", d.key));
    col()
        .key(format!("{}.groups-column", d.key))
        .w(WIDTH)
        .h_full()
        .min_h(0.0)
        .child(search_box(d))
        .child(list)
        .child(bar)
}
