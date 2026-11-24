//! Script pane: a Luau script drawing with host pieces, reloaded live.

pub mod draw;
pub mod library;
pub mod scenes;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::PathBuf;

use rune::{Cursor, Cx, El, ScrollSpec, Tx, col, row, text};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::command::tools::{fetch, plugin};
use yd_core::feed::{FETCH, FeedEvent};
use yd_core::ids::{PaneId, TabId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::reduce::step::commands::PROMPT;
use yd_core::row::{Dot, Tone};
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::disk::Disk;
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::status::dot;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE, Stage, TabAsk};
use yd_ui::theme::look::{LABEL, META, colour, tone};
use yd_ui::win::{Hub, Win, dispatch};

use crate::script_pane::host::{KIT, Piece};
use library::{Asks, FEED, Inputs, Library, Plugin};
use yd_ui::states::status::{Status, view as view_state};

use super::states::WORDS;

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Script,
    view,
    scenes: scenes::SCENES,
    commands: &[Command {
        id: plugin::BUILD,
        title: "Build a pane…",
        key: None,
        menu: None,
        kinds: &[],
        route: Route::Model,
    }],
    settings: &[],
    tabs: Some(tabs),
    on_tab: Some(on_tab),
    finder: Some(Finder::Keyed(thing_at)),
};

/// The plugin a script pane has in front, by name; none is the first.
#[derive(Debug, Default)]
pub struct Front(pub Option<String>);

/// The library of a hub, made empty the first time.
pub fn library(hub: &mut Hub) -> Option<&mut Library> {
    hub.panes.get_mut::<Library>(None)
}

/// Starts looking at `folder` for plugins: the start of the native app calls it.
pub fn open(hub: &mut Hub, folder: PathBuf) {
    if let Some(library) = library(hub) {
        library.folder = Some(folder);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        library.trash = library
            .trash
            .take()
            .or_else(|| home.map(|home| home.join(".Trash")));
    }
}

/// The plugin pane `pane` shows.
fn front(hub: &Hub, pane: PaneId) -> Option<&Plugin> {
    let library = hub.panes.get::<Library>(None)?;
    let picked = hub
        .panes
        .get::<Front>(Some(pane))
        .and_then(|f| f.0.as_deref());
    picked
        .and_then(|name| library.find(name))
        .or_else(|| library.plugins.first())
}

/// The plugin of the first script pane a window draws, else the first plugin: what the kit
/// pane's switches are for.
pub fn in_front(hub: &Hub) -> Option<&Plugin> {
    let grid = &hub.model.grid;
    let drawn = grid
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Script && grid.window_of(p.id).is_some());
    front(hub, drawn.map_or(SCENE_PANE, |p| p.id))
}

/// The plugins some script pane shows now; timers run for these alone.
fn shown(hub: &Hub) -> BTreeSet<String> {
    let grid = &hub.model.grid;
    let mut panes: Vec<PaneId> = grid
        .panes
        .iter()
        .filter(|pane| pane.kind == PaneKind::Script && grid.window_of(pane.id).is_some())
        .map(|pane| pane.id)
        .collect();
    if matches!(hub.stage, Stage::Pane(PaneKind::Script)) {
        panes.push(SCENE_PANE);
    }
    let names = panes
        .into_iter()
        .filter_map(|pane| front(hub, pane).map(|p| p.name.clone()));
    names.collect()
}

/// One pass over the library at `now`, its list rows folded into the model. What is left is
/// for a window to send.
pub fn settle(hub: &mut Hub, now: f64) -> Asks {
    let shown = shown(hub);
    let Hub { model, panes, .. } = hub;
    let Some(library) = panes.get_mut::<Library>(None) else {
        return Asks::default();
    };
    let mut asks = library.advance(model, &shown, now);
    if let Some(rows) = asks.rows.take() {
        step(model, Msg::Feed(FEED, FeedEvent::Reset(rows)));
    }
    for gone in asks.forget.drain(..) {
        step(model, Msg::Feed(FETCH, FeedEvent::Remove(gone)));
    }
    asks
}

/// The window's turn: the folder looked at once a second, timers, fetches asked and stores
/// written. True when the window has something new to draw.
pub fn tick(win: &mut Win, now: f64, tx: &mut Tx) -> bool {
    let asks = match win.hub.try_borrow_mut() {
        Ok(mut hub) => settle(&mut hub, now),
        Err(_) => return false,
    };
    if let Some(next) = asks.next {
        tx.wake_at(next);
    }
    send(win, asks)
}

fn send(win: &mut Win, asks: Asks) -> bool {
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        for (path, text) in asks.writes {
            let job = move || {
                let _ = library::write(&path, &text);
            };
            match hub.panes.get_mut::<Disk>(None) {
                Some(disk) => disk.run(job),
                None => job(),
            }
        }
    }
    for url in asks.fetch {
        let run = Run {
            text: Some(url),
            ..Run::new(fetch::GET)
        };
        dispatch(win, Msg::Command(Route::Source(FETCH), run));
    }
    asks.drawn
}

/// A button of a plugin's pane: the script hears it, then draws again.
pub fn press(win: &mut Win, _tx: &mut Tx, plugin: &str, id: &str) {
    let asks = match win.hub.try_borrow_mut() {
        Ok(mut hub) => {
            let now = hub.model.now.0 as f64 / 1000.0;
            let Hub { model, panes, .. } = &mut *hub;
            let found = panes
                .get_mut::<Library>(None)
                .and_then(|l| l.find_mut(plugin));
            let fetch = found.map(|p| p.press(id, &Inputs::of(model), now));
            Asks {
                fetch: fetch.unwrap_or_default(),
                drawn: true,
                ..Asks::default()
            }
        }
        Err(_) => return,
    };
    send(win, asks);
}

/// "Yi builds a pane": the thread the window shows is asked, in the user's words, to write a
/// plugin into the folder the app watches. False with no thread or no folder.
pub fn build(win: &mut Win, words: &str) -> bool {
    let asked = {
        let Ok(hub) = win.hub.try_borrow() else {
            return false;
        };
        let folder = hub
            .panes
            .get::<Library>(None)
            .and_then(|l| l.folder.clone());
        let thread = yd_ui::shown::thread(&hub, win.window).map(|t| t.id.clone());
        folder.zip(thread)
    };
    let Some((folder, thread)) = asked else {
        return false;
    };
    let Ok(target) = Address::new(Kind::Thread, thread.as_str()) else {
        return false;
    };
    let prompt = format!(
        "{words}\n\nWrite this as a Yi Desktop plugin pane in a new folder under {}. \
         The app loads it as soon as both files are saved.\n\n{KIT}",
        folder.display()
    );
    let run = Run {
        text: Some(prompt),
        ..Run::new(PROMPT).on(Target::Thing(target))
    };
    dispatch(win, Msg::Command(Route::Wire, run));
    true
}

/// A stable tab id from a plugin's name, so a tab keeps its id as others come and go.
fn tab_id(name: &str) -> TabId {
    let hash = name.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    TabId(hash)
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let Some(library) = pane.hub.panes.get::<Library>(None) else {
        return Tabs::shown(None, PaneKind::Script);
    };
    let tabs = library.plugins.iter().map(|plugin| Tab {
        id: tab_id(&plugin.name),
        title: plugin.title().to_owned(),
        fixed: true,
        ..Tab::of(PaneKind::Script)
    });
    Tabs {
        tabs: tabs.collect(),
        active: front(pane.hub, pane.pane).map(|p| tab_id(&p.name)),
        adds: false,
        tool: None,
    }
}

fn on_tab(win: &mut Win, pane: PaneId, ask: TabAsk) {
    let TabAsk::Show(id) = ask else { return };
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let name = hub
        .panes
        .get::<Library>(None)
        .and_then(|l| l.plugins.iter().find(|p| tab_id(&p.name) == id))
        .map(|p| p.name.clone());
    if let (Some(name), Some(front)) = (name, hub.panes.get_mut::<Front>(Some(pane))) {
        front.0 = Some(name);
    }
}

/// A stat is a number and a table row a part of its plugin, so either can be attached.
fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    let plugin = front(hub, pane)?;
    let rest = key.strip_prefix(
        &PaneCx {
            hub,
            pane,
            focused: false,
            edits: None,
        }
        .key(PaneKind::Script),
    )?;
    let (what, part) = rest.strip_prefix('.')?.split_once('.')?;
    let (kind, label, excerpt) = match what {
        "stat" => {
            let stat = plugin.picture.iter().find_map(|piece| match piece {
                Piece::Stat { label, value, .. } if draw::part(label) == part => {
                    Some((label, value))
                }
                _ => None,
            })?;
            (Kind::Number, stat.0.clone(), stat.1.clone())
        }
        "row" => (Kind::Plugin, part.to_owned(), plugin.title().to_owned()),
        _ => return None,
    };
    let id = match kind {
        Kind::Number => [plugin.name.as_str(), "/", part].concat(),
        _ => plugin.name.clone(),
    };
    let address = Address::new(kind, id).ok()?;
    let address = if kind == Kind::Plugin {
        address.part(["row-", part].concat())
    } else {
        address
    };
    Some(Thing {
        label,
        address,
        excerpt: Some(excerpt),
    })
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let tokens = pane.tokens();
    let key = pane.key(PaneKind::Script);
    let body = col()
        .key(&key)
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)));
    let Some(plugin) = front(pane.hub, pane.pane) else {
        return body.child(view_state(pane, &key, &Status::Empty, &WORDS));
    };
    let hosts: Vec<&str> = plugin
        .reach()
        .iter()
        .filter_map(|r| r.rsplit_once(':').map(|(h, _)| h))
        .collect();
    let live = chip(tokens, &format!("live · version {}", plugin.version));
    let head = header(
        tokens,
        vec![
            lead(tokens, plugin.title()),
            text(hosts.join(", ")).font(LABEL).color(colour(tokens.dim)),
        ],
        vec![live.key(format!("{key}.live"))],
    )
    .key(format!("{key}.pane-header"));
    let every = plugin
        .every()
        .map(|n| format!(" · every {n} s"))
        .unwrap_or_default();
    let foot = footer(
        tokens,
        vec![
            text(format!("from plugin {}{every}", plugin.name))
                .font(META)
                .color(colour(tokens.faint)),
        ],
        vec![text("Yi built this").font(META).color(colour(tokens.faint))],
    )
    .key(format!("{key}.footer"));
    let error = plugin
        .error
        .as_deref()
        .map(|why| failure(tokens, &key, why));
    let picture = draw::pieces(tokens, &key, &plugin.name, &plugin.picture);
    let shown = col()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .grow(1.0)
        .min_h(0.0)
        .child(col().w_full().children(error).child(picture));
    body.child(head).child(shown).child(foot)
}

fn chip(tokens: &Tokens, words: &str) -> El {
    row()
        .h(16.0)
        .px(7.0)
        .gap(5.0)
        .center_y()
        .radius(9.0)
        .fill(colour(yd_ui::theme::scale::CHIP_QUIET.of(tokens)))
        .cursor(Cursor::Default)
        .child(dot(tokens, Dot::Need))
        .child(text(words).font(META).color(colour(tokens.dim)))
}

/// Why the script did not draw, over the last picture that did.
fn failure(tokens: &Tokens, key: &str, why: &str) -> El {
    col()
        .key(format!("{key}.error"))
        .w_full()
        .px(tokens.gutter)
        .py(6.0)
        .gap(2.0)
        .fill(colour(tone(tokens, Tone::Bad).alpha(0.12)))
        .child(
            text(why)
                .font(LABEL)
                .color(colour(tone(tokens, Tone::Bad)))
                .wrap(),
        )
        .child(
            text("The last picture that drew is kept below until the script runs again.")
                .font(META)
                .color(colour(tokens.faint)),
        )
}
