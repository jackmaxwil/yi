//! Plugins pane: what is installed, what each adds, what each may do.

#[cfg(test)]
mod tests;

use rune::{Cx, El, Tx, col};
use yd_core::address::uri::Address;
use yd_core::command::table::{Command, Route, Target};
use yd_core::feed::{FILES, FeedEvent, FeedStatus, SourceCmd};
use yd_core::ids::{CommandId, RowId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Value, rows_from_json};
use yd_ui::grid_view::view::{open_file, show_pane};
use yd_ui::kit::bars::{header, lead};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::list::state::Lists;
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win, dispatch};

use crate::plugins::desc::{DESC, OPEN_SOURCE, RELOAD, REMOVE};
use crate::script_pane::pane::library::FEED;
use crate::script_pane::pane::{library, scenes as script, settle};

use super::states::WORDS;

const MOCKUP: &str = include_str!("../../../../fixtures/mockup/plugins.rows.json");

const fn on_plugin(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Plugin],
        route: Route::Model,
    }
}

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Plugins,
    view,
    scenes: &[
        Scene {
            name: "plugins/mockup",
            stage: Stage::Pane(PaneKind::Plugins),
            setup: mockup,
        },
        Scene {
            name: "plugins/live",
            stage: Stage::Pane(PaneKind::Plugins),
            setup: live,
        },
    ],
    commands: &[on_plugin(REMOVE, "Remove")],
    settings: &[],
    tabs: None,
    on_tab: None,
    finder: Some(Finder::Rows(&DESC)),
};

/// The ids [`carry`] serves, for the window's chain.
pub const CARRIED: &[CommandId] = &[RELOAD, OPEN_SOURCE, REMOVE];

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let tokens = pane.tokens();
    let key = pane.key(PaneKind::Plugins);
    let add = chip(tokens, Chip::Plain, "Add", None, false)
        .key(format!("{key}.add"))
        .on_press_tx(|win: &mut Win, tx: &mut Tx| {
            show_pane(win, tx, PaneKind::PluginKit);
        });
    col()
        .key(&key)
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(
            header(tokens, vec![lead(tokens, "Installed")], vec![add])
                .key(format!("{key}.pane-header")),
        )
        .child(worded(
            pane,
            PaneKind::Plugins,
            "",
            &DESC,
            pane.model().tables.get(FEED),
            &WORDS,
            false,
        ))
}

/// The plugin a command is aimed at: a plugin's own row, or a number or a row it drew, whose
/// address opens with the plugin's name.
fn plugin_of(address: &Address) -> Option<&str> {
    match address.kind() {
        Kind::Plugin => Some(address.id()),
        Kind::Number => address.id().split('/').next(),
        _ => None,
    }
}

/// Reload, Open source and Remove of a plugin's row, the first two also from the right-click
/// menu of anything it drew. Remove asks once, then moves the folder to the Trash.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(address) = target else {
        return false;
    };
    // Remove is a button of the plugin's own row alone, never of something it drew.
    let own = address.kind() == Kind::Plugin || command != REMOVE;
    let Some(name) = plugin_of(address).filter(|_| own && CARRIED.contains(&command)) else {
        return false;
    };
    let name = name.to_owned();
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    let row = hub
        .model
        .tables
        .get(FEED)
        .and_then(|t| t.find(&RowId::new(name.as_str())).cloned());
    let Some(row) = row else { return false };
    match command {
        RELOAD => {
            let reloaded = library(&mut hub).is_some_and(|l| l.reload_now(&name));
            let now = hub.model.now.0 as f64 / 1000.0;
            settle(&mut hub, now);
            reloaded
        }
        OPEN_SOURCE => {
            let file = row
                .words("path")
                .filter(|p| !p.is_empty())
                .and_then(|p| Address::new(Kind::File, p).ok());
            let folder = library(&mut hub).and_then(|l| l.folder.clone());
            drop(hub);
            let (Some(file), Some(folder)) = (file, folder) else {
                return false;
            };
            // The files source reads outside the thread's folder only under this one.
            dispatch(win, Msg::Source(FILES, SourceCmd::Readable(folder)));
            open_file(win, tx, file);
            true
        }
        _ if !matches!(row.get("confirm"), Some(Value::Flag(true))) => {
            let asked = row.cell("confirm", Value::Flag(true));
            step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(asked)));
            true
        }
        _ => {
            let moved = library(&mut hub).map(|l| l.remove(&name));
            let now = hub.model.now.0 as f64 / 1000.0;
            settle(&mut hub, now);
            if let Some(Err(why)) = moved {
                step(
                    &mut hub.model,
                    Msg::Feed(FEED, FeedEvent::Upsert(row.text("error", why))),
                );
            }
            true
        }
    }
}

/// The mockup's nine plugins, docker open, as rows; live, only script plugins are listed.
fn mockup(hub: &mut Hub) {
    let event = match rows_from_json(MOCKUP) {
        Ok(rows) => FeedEvent::Reset(rows),
        Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
    };
    step(&mut hub.model, Msg::Feed(FEED, event));
    open(hub, "docker");
}

/// The Queue depth plugin as the library lists it, opened.
fn live(hub: &mut Hub) {
    script::queue_depth(hub);
    open(hub, script::NAME);
}

fn open(hub: &mut Hub, id: &str) {
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists.list_mut("").set_open(Some(RowId::new(id)));
    }
}
