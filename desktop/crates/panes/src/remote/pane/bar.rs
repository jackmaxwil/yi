//! The bar over the screen: machine, size and latency, the Watch, Control and Mark up switch,
//! and the screenshot button; and the line under it.

use rune::{Cursor, El, Icon, Tx, row, text};
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::remote;
use yd_core::ids::PaneId;
use yd_core::row::Row;
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::MONITOR;
use yd_ui::kit::icons::glyph;
use yd_ui::kit::segmented::segmented;
use yd_ui::menu::command_box;
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{LABEL, colour, white};
use yd_ui::win::Win;

use super::{Mode, change};

/// What the bar says while it is in Mark up: the one gesture the mode has.
pub const MARK_UP_HINT: &str = "Drag over the screen to mark a region, then type a note.";

static CAMERA: Icon = yd_ui::line_icon!(
    "camera",
    r#"<path d="M4 8h3l2-3h6l2 3h3v11H4z"/><circle cx="12" cy="13" r="3.500"/>"#
);

/// "1920 × 1080 · 38 ms", each part left out when the host did not say it.
fn size(row: Option<&Row>) -> String {
    let word = |field: &str| row.and_then(|r| r.words(field)).filter(|w| !w.is_empty());
    let ms = word("ms").map(|ms| format!("{ms} ms"));
    let parts: Vec<String> = [word("size").map(str::to_owned), ms]
        .into_iter()
        .flatten()
        .collect();
    parts.join(" · ")
}

/// The bar, 22 high. `head` is the screen's `screen:0` row, none before the host answers.
pub fn bar(pane: &PaneCx<'_>, key: &str, mode: Mode, head: Option<&Row>) -> El {
    let tokens = pane.tokens();
    let id: PaneId = pane.pane;
    let machine = head.and_then(|r| r.words("machine")).unwrap_or_default();
    let names: Vec<&str> = Mode::ALL.iter().map(|m| m.title()).collect();
    let at = Mode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
    let pick = move |win: &mut Win, i: usize| {
        if let Some(chosen) = Mode::ALL.get(i) {
            let chosen = *chosen;
            change(win, id, |state| state.mode = chosen);
        }
    };
    let shot = row()
        .key(format!("{key}.screenshot"))
        .label("Screenshot")
        .center_y()
        .cursor(Cursor::Pointer)
        .child(glyph(&CAMERA, 11.0, tokens.faint))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            let ask = Run::new(remote::SCREENSHOT).on(Target::Pane(id));
            command_box::run(win, tx, ask);
        });
    row()
        .key(format!("{key}.bar"))
        .w_full()
        .h(tokens.bar)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .clip_rounded(0.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.inner_lines)))
        .child(glyph(&MONITOR, 10.0, tokens.dim))
        .child(text(machine).font(LABEL).color(colour(tokens.fg)))
        .child(
            text(size(head))
                .font(LABEL)
                .color(colour(tokens.faint))
                .truncate()
                .min_w(0.0),
        )
        .child(rune::spacer())
        .child(segmented(tokens, &format!("{key}.mode"), &names, at, pick))
        .child(shot)
}

/// What the line under the bar says: how a region is marked, else the host's last word.
pub fn note_of(mode: Mode, row: Option<&Row>, failed: Option<&str>) -> Option<String> {
    if mode == Mode::MarkUp {
        return Some(MARK_UP_HINT.to_owned());
    }
    let said = row.and_then(|r| r.words("note")).filter(|n| !n.is_empty());
    let said = said.map(|note| match mode {
        Mode::Control => format!("Pointer and keys are {note}."),
        Mode::Watch | Mode::MarkUp => note.to_owned(),
    });
    said.or_else(|| row.and(failed).map(str::to_owned))
}

pub fn note(tokens: &Tokens, key: &str, said: &str) -> El {
    row()
        .key(format!("{key}.note"))
        .w_full()
        .px(8.0)
        .py(3.0)
        .child(text(said).font(LABEL).color(colour(tokens.warn)))
}
