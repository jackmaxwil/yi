//! The pane's header bar (source, levels, text filter, following), its footer line, and the
//! pill that jumps back to the newest line.

use rune::{Cursor, El, KeyEvent, PopupItem, PopupMenu, Tx, el, row, text, text_input};
use yd_core::ids::PaneId;
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::kit::bars::{footer, header};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::icons::{SEARCH, glyph};
use yd_ui::list::state::Lists;
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::theme::scale::RADIUS_CHIP;
use yd_ui::win::Win;

use super::shown::{Shown, WINDOW};
use super::state::{Filters, Level, change, drawn, edit, sync};

/// Whether the list stands at its newest line with no line open: the plan's default 9.
pub fn following(pane: &PaneCx<'_>) -> bool {
    let list = pane.state::<Lists>().and_then(|lists| lists.list(""));
    list.is_none_or(|list| list.open().is_none() && list.at_tail())
}

fn pick_source(win: &mut Win, tx: &mut Tx, pane: PaneId, anchor: String, names: &[String]) {
    let chosen = win
        .hub
        .try_borrow()
        .ok()
        .and_then(|hub| {
            Some(
                hub.panes
                    .get::<super::state::State>(Some(pane))?
                    .filters
                    .source
                    .clone(),
            )
        })
        .flatten();
    let all = PopupItem::new("*", "All sources").checked(chosen.is_none());
    let each = names.iter().map(|name| {
        PopupItem::new(name.as_str(), name.as_str()).checked(chosen.as_deref() == Some(name))
    });
    let menu = PopupMenu::new().items(std::iter::once(all).chain(each));
    tx.popup_menu(
        menu,
        anchor.as_str(),
        move |win: &mut Win, picked, _tx: &mut Tx| {
            let Some(picked) = picked else { return };
            let source = (picked != "*").then(|| picked.to_owned());
            change(win, pane, |state| {
                state.filters.source = source;
                state.filters.older = 0;
            });
        },
    );
}

fn filter_box(tokens: &Tokens, key: &str, pane: &PaneCx<'_>) -> El {
    let id = pane.pane;
    let blank = rune::TextEdit::new("");
    let field = text_input(drawn(pane).unwrap_or(&blank), LABEL)
        .key(format!("{key}.filter.input"))
        .single_line()
        .w(86.0)
        .color(colour(tokens.fg))
        .placeholder("Filter", colour(tokens.dim))
        .label("Filter")
        .caret(colour(tokens.accent), a11y::selection(tokens))
        .on_edit(move |win: &mut Win| edit(win, id))
        .on_input_change(move |win: &mut Win, _tx: &mut Tx| sync(win, id))
        .on_input_key(move |_: &mut Win, _: &KeyEvent, _: &mut Tx| false);
    row()
        .key(format!("{key}.filter"))
        .h(19.0)
        .px(6.0)
        .gap(5.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .fill(colour(tokens.raised))
        .child(glyph(&SEARCH, 9.0, tokens.dim))
        .child(field)
}

/// The bar: the source menu and the four level toggles at the left; the text filter and
/// whether the list follows at the right.
pub fn bar(pane: &PaneCx<'_>, key: &str, shown: &Shown, filters: &Filters) -> El {
    let tokens = pane.tokens();
    let id = pane.pane;
    let names: Vec<String> = shown.sources.iter().cloned().collect();
    let anchor = format!("{key}.source");
    let label = filters.source.as_deref().unwrap_or("All sources");
    let source = chip(tokens, Chip::Plain, label, None, true)
        .key(anchor.clone())
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            pick_source(win, tx, id, anchor.clone(), &names);
        });
    let level = |level: Level| {
        let look = if filters.on(level) {
            Chip::Pill
        } else {
            Chip::Plain
        };
        chip(tokens, look, level.title(), None, false)
            .key(format!("{key}.level.{}", level.slug()))
            .on_press(move |win: &mut Win| change(win, id, |state| state.filters.flip(level)))
    };
    let around = filters.around.map(|at| {
        let label = format!("Around {}", super::wire::clock_text(at));
        chip(tokens, Chip::Pill, &format!("{label} ✕"), None, false)
            .key(format!("{key}.around"))
            .on_press(move |win: &mut Win| {
                change(win, id, |state| state.filters.around = None);
            })
    });
    let live = following(pane);
    let (dot, words) = if live {
        (tokens.ok, "following")
    } else {
        (tokens.faint, "paused")
    };
    let status = row()
        .key(format!("{key}.following"))
        .gap(5.0)
        .center_y()
        .child(el().size(7.0, 7.0).radius(3.5).fill(colour(dot)))
        .child(text(words).font(LABEL).color(colour(tokens.dim)));
    let left = std::iter::once(source)
        .chain(Level::ALL.into_iter().map(level))
        .chain(around)
        .collect();
    header(tokens, left, vec![filter_box(tokens, key, pane), status])
        .key(format!("{key}.pane-header"))
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The footer: how many sources and lines there are, how many errors and warnings, and the
/// way to list older lines when the window left some out.
pub fn foot(pane: &PaneCx<'_>, key: &str, shown: &Shown) -> El {
    let tokens = pane.tokens();
    let id = pane.pane;
    let said = |words: String| text(words).font(META).color(colour(tokens.faint));
    let older = (shown.older > 0).then(|| {
        text(format!("Load {WINDOW} older"))
            .key(format!("{key}.older"))
            .font(META)
            .color(colour(tokens.accent))
            .cursor(Cursor::Pointer)
            .on_press(move |win: &mut Win| change(win, id, |state| state.filters.older += 1))
    });
    let left = [
        Some(said(format!(
            "{} · {}",
            plural(shown.sources.len(), "source", "sources"),
            plural(shown.total, "line", "lines")
        ))),
        older,
    ];
    let right = said(format!(
        "{} · {}",
        plural(shown.errors, "error", "errors"),
        plural(shown.warnings, "warning", "warnings")
    ));
    footer(tokens, left.into_iter().flatten().collect(), vec![right]).key(format!("{key}.footer"))
}
