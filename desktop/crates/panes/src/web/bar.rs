//! The browser bar: back, forward, reload, the address field, the width chip, and Pick or,
//! where the pane is wide enough, the Browse, Pick, Design switch.

use rune::{Cursor, El, Icon, TextEdit, Tx, row, text, text_input};
use yd_core::address::attach::PICK;
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::web;
use yd_core::ids::{CommandId, PaneId};
use yd_core::row::Row;
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::chrome::tabs::GLOBE;
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::icons::glyph;
use yd_ui::kit::segmented::segmented;
use yd_ui::menu::command_box;
use yd_ui::overlay::pick;
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{CODE_LINE, LABEL, colour, white};
use yd_ui::win::{Win, field};

use super::pane::change;
use crate::design::pane as design;

/// The address field in the window that edits it.
const ADDRESS: &str = "address";
/// A pane at least this wide has the three-way switch; a narrower one keeps the Pick chip.
pub const SWITCH_FROM: f32 = 420.0;
const MODES: [&str; 3] = ["Browse", "Pick", "Design"];

pub static BACK: Icon = yd_ui::line_icon!("back", r#"<path d="m15 6-6 6 6 6"/>"#);
pub static FORWARD: Icon = yd_ui::line_icon!("chev", r#"<path d="m9 6 6 6-6 6"/>"#);
pub static RELOAD: Icon = yd_ui::line_icon!(
    "reload",
    r#"<path d="M20 12a8 8 0 1 1-2.600-5.900M20 4v5h-5"/>"#
);
static TARGET: Icon = yd_ui::line_icon!(
    "target",
    r#"<circle cx="12" cy="12" r="7"/><path d="M12 2v4M12 18v4M2 12h4M18 12h4"/>"#
);

/// The address as the field shows it: no `http://` or `https://`.
pub fn shown(url: &str) -> &str {
    ["https://", "http://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
        .unwrap_or(url)
}

/// The field Rune edits: this window's own, holding the page's address until it is typed in.
pub fn edit<'a>(win: &'a mut Win, pane: PaneId, url: &str) -> &'a mut TextEdit {
    let own = win.edits.entry(field(pane, ADDRESS)).or_default();
    if !own.focused {
        *own = TextEdit::new(shown(url));
    }
    own
}

fn drawn(pane: &PaneCx<'_>, url: &str) -> TextEdit {
    let own = pane
        .edits
        .and_then(|edits| edits.get(&field(pane.pane, ADDRESS)));
    own.filter(|own| own.focused)
        .cloned()
        .unwrap_or_else(|| TextEdit::new(shown(url)))
}

fn run(win: &mut Win, tx: &mut Tx, pane: PaneId, command: CommandId, words: Option<String>) {
    let mut asked = Run::new(command).on(Target::Pane(pane));
    asked.text = words;
    command_box::run(win, tx, asked);
}

/// Opens what was typed, or says why not under the bar.
fn submit(win: &mut Win, tx: &mut Tx, pane: PaneId) {
    let words = win
        .edits
        .get(&field(pane, ADDRESS))
        .map(|own| own.text.clone())
        .unwrap_or_default();
    let Some(url) = web::address(&words) else {
        let said = format!(
            "Not opened: “{}” is not an http, https or file address",
            words.trim()
        );
        change(win, pane, |state| {
            state.refused = Some(said.chars().take(160).collect())
        });
        return;
    };
    change(win, pane, |state| state.refused = None);
    run(win, tx, pane, web::OPEN, Some(url));
}

/// One of the bar's icon buttons, named for what it does: an icon alone says nothing to a
/// screen reader.
fn button(
    tokens: &Tokens,
    key: &str,
    name: &'static str,
    icon: (&'static Icon, f32),
    on: bool,
) -> El {
    let ink = if on { tokens.dim } else { tokens.faint };
    row()
        .key(format!("{key}.{}", name.to_lowercase()))
        .label(name)
        .center_y()
        .cursor(Cursor::Pointer)
        .child(glyph(icon.0, icon.1, ink))
}

/// Browse, Pick or Design: the page takes the input, the window picks things to attach, or
/// design mode holds the page and picks an element.
fn switch(pane: &PaneCx<'_>, key: &str) -> El {
    let picking = pick::context(pane.hub).is_some_and(|context| context.picking);
    let designing = design::mode(pane.hub).is_some_and(|mode| mode.on);
    let on = if designing { 2 } else { usize::from(picking) };
    let choose = |win: &mut Win, index: usize| {
        pick::pick(win, index == 1);
        if index == 2 {
            design::start(win);
        } else {
            design::set(win, |mode| *mode = design::Mode::default());
        }
    };
    segmented(pane.tokens(), &format!("{key}.mode"), &MODES, on, choose)
}

/// The bar over the page. `page` is the feed's row, none before the first address; `wide`
/// says the pane has room for the three-way switch.
pub fn bar(pane: &PaneCx<'_>, key: &str, page: Option<&Row>, wide: bool) -> El {
    let tokens = pane.tokens();
    let id = pane.pane;
    let word = |cell: &str| page.and_then(|p| p.words(cell)).unwrap_or_default();
    let url = word("url").to_owned();
    let go = |command: CommandId| move |win: &mut Win, tx: &mut Tx| run(win, tx, id, command, None);
    let back = button(
        tokens,
        key,
        "Back",
        (&BACK, 11.0),
        word("can_back") == "yes",
    );
    let ahead = word("can_forward") == "yes";
    let forward = button(tokens, key, "Forward", (&FORWARD, 11.0), ahead);
    let reload = button(tokens, key, "Reload", (&RELOAD, 10.0), true);
    let (back, forward) = (
        back.on_press_tx(go(web::BACK)),
        forward.on_press_tx(go(web::FORWARD)),
    );
    let reload = reload.on_press_tx(go(web::RELOAD));
    let seeded = url.clone();
    let input = text_input(&drawn(pane, &url), CODE_LINE)
        .key(format!("{key}.address.input"))
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .color(colour(tokens.fg))
        .placeholder("Type an address", colour(tokens.faint))
        .label("Address")
        .caret(colour(tokens.accent), a11y::selection(tokens))
        .on_edit(move |win: &mut Win| edit(win, id, &seeded))
        .on_submit(move |win: &mut Win, tx: &mut Tx| submit(win, tx, id));
    let address = row()
        .key(format!("{key}.address"))
        .grow(1.0)
        .min_w(0.0)
        .h(16.0)
        .px(6.0)
        .gap(5.0)
        .center_y()
        .radius(4.0)
        .fill(colour(tokens.raised))
        .clip_rounded(4.0)
        .child(glyph(&GLOBE, 10.0, tokens.dim))
        .child(input);
    let width = Some(word("width")).filter(|w| !w.is_empty()).map(|w| {
        chip(tokens, Chip::Plain, &format!("{w} wide"), None, false).key(format!("{key}.width"))
    });
    let pick = chip(tokens, Chip::Pill, "Pick", Some(&TARGET), false)
        .key(format!("{key}.pick"))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            command_box::run(win, tx, Run::new(PICK).on(Target::Pane(id)));
        });
    row()
        .key(format!("{key}.browser-bar"))
        .w_full()
        .h(tokens.bar)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .clip_rounded(0.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.inner_lines)))
        .child(back)
        .child(forward)
        .child(reload)
        .child(address)
        .children(width)
        .child(if wide { switch(pane, key) } else { pick })
}

/// The line under the bar: why an address was refused, else what the page last said.
pub fn note(tokens: &Tokens, key: &str, said: Option<&str>) -> Option<El> {
    let said = said.filter(|said| !said.is_empty())?;
    let line = text(said).font(LABEL).color(colour(tokens.warn));
    Some(
        row()
            .key(format!("{key}.note"))
            .w_full()
            .px(8.0)
            .py(3.0)
            .child(line),
    )
}
