//! Plugin kit pane: what a script-drawn pane may use and reach.

#[cfg(test)]
mod tests;

use rune::{Cx, El, TextEdit, Tx, col, row, spacer, text, text_input};
use yd_core::command::table::Target;
use yd_core::command::tools::plugin;
use yd_core::ids::CommandId;
use yd_core::pane::PaneKind;
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::kit::bars::footer;
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::label::section;
use yd_ui::kit::status::tag;
use yd_ui::kit::toggle::toggle;
use yd_ui::pane::{PaneCx, PaneDef, Scene, Stage};
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::win::{Win, field};

use crate::script_pane::host::KIT;
use crate::script_pane::pane::library::{GRANTS, GRANTS_ROW, grant};
use crate::script_pane::pane::{build, in_front, scenes};
use yd_ui::states::status::bar;

use crate::copy::empty::{SERVER, down};

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::PluginKit,
    view,
    scenes: &[Scene {
        name: "plugin-kit/kit",
        stage: Stage::Pane(PaneKind::PluginKit),
        setup: scenes::queue_depth,
    }],
    commands: &[],
    settings: &[GRANTS_ROW],
    tabs: None,
    on_tab: None,
    finder: None,
};

/// The record's three tag groups; a piece or a reach marked `true` is not built yet.
const GROUPS: [(&str, &[(&str, bool)]); 3] = [
    (
        "Pieces a pane can use",
        &[
            ("stat", false),
            ("table", false),
            ("tree", false),
            ("list", false),
            ("chart", true),
            ("spark", false),
            ("form", true),
            ("button", false),
            ("log view", false),
            ("code", false),
            ("diff", true),
            ("screen", true),
        ],
    ),
    (
        "What it can ask for",
        &[
            ("the web, one host at a time", false),
            ("files, read only", true),
            ("a command, with your go-ahead", true),
            ("threads", false),
            ("findings", false),
            ("logs", false),
            ("its own small store", false),
        ],
    ),
    (
        "When it runs",
        &[
            ("every n seconds", false),
            ("when something happens", false),
            ("when you open it", false),
        ],
    ),
];
const LATER: [&str; 2] = ["Read your files", "Run commands"];
/// The field the user says what pane they want in.
const WANT: &str = "want";

/// "Build a pane…" from the command box: this pane in front, its field holding the keyboard
/// for the words the build is asked in.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, _: &Target) -> bool {
    if command != plugin::BUILD {
        return false;
    }
    let Some(pane) = yd_ui::grid_view::view::show_pane(win, tx, PaneKind::PluginKit) else {
        return false;
    };
    let field = format!("{}-{}.{WANT}", PaneKind::PluginKit.slug(), pane.0);
    tx.focus_when_shown(field, false);
    true
}

/// What the pane last said about a press: a switch not built, a build with nowhere to go.
#[derive(Debug, Default)]
pub struct Said(pub Option<String>);

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let tokens = pane.tokens();
    let key = pane.key(PaneKind::PluginKit);
    let groups = GROUPS.iter().map(|(title, tags)| {
        let shown = tags.iter().map(|(words, later)| {
            let drawn = tag(tokens, words, false);
            if *later { drawn.opacity(0.45) } else { drawn }
        });
        let flow = row()
            .w_full()
            .min_w(0.0)
            .px(tokens.gutter)
            .gap(4.0)
            .flex_wrap()
            .children(shown);
        let note = title
            .starts_with("Pieces")
            .then_some("chart, form, diff, screen: later");
        col()
            .w_full()
            .gap(2.0)
            .child(section(tokens, title, None, note))
            .child(flow)
    });
    let said = pane.state::<Said>().and_then(|s| s.0.as_deref());
    col()
        .key(&key)
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(4.0)
                .py(4.0)
                .children(groups)
                .child(may(pane, &key)),
        )
        .children(down(pane).map(|down| bar(pane, &key, &down, &SERVER)))
        .child(ask(pane, &key))
        .children(said.map(|said| {
            row()
                .px(tokens.gutter)
                .child(text(said).font(META).color(colour(tokens.faint)))
        }))
        .child(
            footer(
                tokens,
                vec![
                    text("It behaves like a built-in pane: tabs, pull-out, context boxes")
                        .font(META)
                        .color(colour(tokens.faint)),
                ],
                Vec::new(),
            )
            .key(format!("{key}.footer")),
        )
}

/// "This plugin may": a switch for each host its manifest names, and two that are not built.
fn may(pane: &PaneCx<'_>, key: &str) -> El {
    let tokens = pane.tokens();
    let mut rows = Vec::new();
    if let Some(plugin) = in_front(pane.hub) {
        let grants = crate::script_pane::pane::library::Inputs::of(pane.model()).grants;
        for pair in plugin.reach() {
            let (host, port) = pair.rsplit_once(':').unwrap_or((pair.as_str(), ""));
            let on = grants
                .lines()
                .any(|line| line == [plugin.name.as_str(), " ", pair].concat());
            let (name, pair) = (plugin.name.clone(), pair.clone());
            let id = pane.pane;
            let flip = move |win: &mut Win, _: &mut Tx| switch(win, id, &name, &pair, !on);
            let label = format!("Reach {host} on port {port}");
            rows.push(
                permission(
                    tokens,
                    &format!("{key}.may.{}", pair_key(&label)),
                    &label,
                    on,
                )
                .on_press_tx(flip),
            );
        }
    }
    for label in LATER {
        let id = pane.pane;
        let later = move |win: &mut Win, _: &mut Tx| {
            if let Ok(mut hub) = win.hub.try_borrow_mut()
                && let Some(said) = hub.panes.get_mut::<Said>(Some(id))
            {
                said.0 = Some(format!("{label} is not available yet"));
            }
        };
        rows.push(
            permission(
                tokens,
                &format!("{key}.may.{}", pair_key(label)),
                label,
                false,
            )
            .on_press_tx(later),
        );
    }
    col()
        .w_full()
        .gap(2.0)
        .child(section(tokens, "This plugin may", None, None))
        .children(rows)
}

fn pair_key(label: &str) -> String {
    label.to_lowercase().replace([' ', '.', ':'], "-")
}

fn permission(tokens: &Tokens, key: &str, label: &str, on: bool) -> El {
    row()
        .key(key)
        .w_full()
        .h(tokens.row)
        .px(tokens.gutter)
        .center_y()
        .child(text(label).font(LABEL).color(colour(tokens.dim)))
        .child(spacer())
        .child(toggle(tokens, &format!("{key}.toggle"), on))
}

/// Switches a grant and keeps it in the app's settings file.
fn switch(win: &mut Win, pane: yd_core::ids::PaneId, plugin: &str, pair: &str, on: bool) {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    grant(&mut hub.model, plugin, pair, on);
    let key = yd_core::ids::SettingKey::fixed(GRANTS);
    if let Err(why) = crate::settings::pane::file::keep(&mut hub, &key)
        && let Some(said) = hub.panes.get_mut::<Said>(Some(pane))
    {
        said.0 = Some(why.to_string());
    }
}

/// "Build a pane…": the words go to the shown thread with the kit; "Copy for Yi" copies it.
fn ask(pane: &PaneCx<'_>, key: &str) -> El {
    let tokens = pane.tokens();
    let blank = TextEdit::new("");
    let edit = pane
        .edits
        .and_then(|edits| edits.get(&field(pane.pane, WANT)));
    let id = pane.pane;
    let input = text_input(edit.unwrap_or(&blank), LABEL)
        .key(format!("{key}.want"))
        .single_line()
        .label("Build a pane")
        .placeholder("Build a pane…", colour(tokens.faint))
        .grow(1.0)
        .min_w(0.0)
        .color(colour(tokens.fg))
        .caret(colour(tokens.fg), a11y::selection(tokens))
        .on_edit(move |win: &mut Win| win.edits.entry(field(id, WANT)).or_default())
        .on_submit(move |win: &mut Win, _: &mut Tx| submit(win, id));
    let copy = button(tokens, Button::Ghost, "Copy for Yi", None)
        .key(format!("{key}.copy"))
        .on_press_tx(|_: &mut Win, tx: &mut Tx| tx.copy(KIT.to_owned()));
    row()
        .w_full()
        .px(tokens.gutter)
        .py(4.0)
        .gap(6.0)
        .center_y()
        .child(input)
        .child(copy)
}

fn submit(win: &mut Win, pane: yd_core::ids::PaneId) {
    let words = win
        .edits
        .entry(field(pane, WANT))
        .or_default()
        .text
        .trim()
        .to_owned();
    if words.is_empty() {
        return;
    }
    let built = build(win, &words);
    if built {
        win.edits.entry(field(pane, WANT)).or_default().clear();
    }
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(said) = hub.panes.get_mut::<Said>(Some(pane))
    {
        said.0 = Some(if built {
            "Sent to the thread: the pane appears as a tab when Yi saves its files".to_owned()
        } else {
            "Open a thread first: Yi builds the pane in the thread this window shows".to_owned()
        });
    }
}
