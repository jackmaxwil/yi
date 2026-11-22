//! Which kinds of pane have something to show in a window now. The grid rests a pane a
//! layout seated while its kind is not among them, and gives it its slot back once it is.

use std::io::Write as _;

use rune::Tx;
use yd_core::command::by_kind::{self, PANE_ALWAYS, PANE_NEVER};
use yd_core::command::table::Target;
use yd_core::feed::{FeedStatus, OPEN_FILES, Table};
use yd_core::grid::step::GridMsg;
use yd_core::ids::{CommandId, RowId, SettingKey, WindowId};
use yd_core::msg::{Msg, step};
use yd_core::pane::{Opens, PaneKind};
use yd_core::row::Rows;
use yd_core::settings::store::{OPENS, SettingsMsg};
use yd_ui::grid_view::drag::GLIDE;
use yd_ui::win::{Hub, Win, dispatch};

use crate::{agents, changes, files, remote, report, terminal, web};

/// Whether a feed has a failure to say: a pane shows that even with no rows.
fn failing(table: Option<&Table>) -> bool {
    let status = table.map(Table::status);
    matches!(status, Some(FeedStatus::Offline | FeedStatus::Error(_)))
}

fn rows(table: Option<&Table>) -> bool {
    table.is_some_and(|table| !table.is_empty())
}

/// Whether `kind` has something to show in `window`: rows, a page, a file, a failure. A kind
/// with no test here always has.
fn has(hub: &Hub, window: WindowId, kind: PaneKind) -> bool {
    let tables = &hub.model.tables;
    let thread = yd_ui::shown::thread(hub, window);
    match kind {
        PaneKind::Files => {
            let table = tables.get(files::pane::FEED);
            rows(table) || failing(table)
        }
        PaneKind::Changes => {
            let table = tables.get(changes::pane::FEED);
            !changes::pane::files(table).is_empty() || failing(table)
        }
        PaneKind::Code => rows(tables.get(OPEN_FILES)),
        PaneKind::Web => {
            let (feed, table) = (web::pane::FEED, tables.get(web::pane::FEED));
            let page = RowId::new(format!("page:{}", feed.instance));
            table.is_some_and(|table| table.find(&page).is_some()) || failing(table)
        }
        PaneKind::Remote => remote::pane::has(hub),
        PaneKind::Report => thread.and_then(report::pane::newest).is_some(),
        PaneKind::Terminal => terminal::pane::has(hub, window),
        PaneKind::Agents => agents::pane::has(hub, thread),
        _ => true,
    }
}

/// The kinds with something to show in `window`.
pub fn kinds(hub: &Hub, window: WindowId) -> Vec<PaneKind> {
    // A kind the person always opens has its pane to show, whatever is in it.
    let words = hub.model.settings.opens();
    let always = |kind: &PaneKind| matches!(words.get(kind), Some(Opens::Always(_)));
    let all = PaneKind::ALL.iter().copied();
    all.filter(|kind| always(kind) || has(hub, window, *kind))
        .collect()
}

/// Tells the grid the session this window shows and its kinds when either changed, so its
/// panes rest and come back; true when it was told. The window's poll calls it.
pub fn tell(win: &mut Win, tx: &mut Tx) -> bool {
    let window = win.window;
    let (kinds, thread) = {
        let Ok(hub) = win.hub.try_borrow() else {
            return false;
        };
        let grid = &hub.model.grid;
        let now = kinds(&hub, window);
        // New chat shows no session: only the panes kept across sessions stay with it.
        let fresh = yd_ui::shown::fresh(&hub, window);
        let thread = yd_ui::shown::thread(&hub, window).filter(|_| !fresh);
        let thread = thread.map(|thread| thread.id.clone());
        let told = grid.have.get(&(window, thread.clone()));
        let same = told.is_some_and(|told| told.iter().eq(now.iter()))
            && grid.shows.get(&window) == thread.as_ref();
        if same || grid.window(window).is_none() {
            return false;
        }
        (now, thread)
    };
    tx.motion_curve(GLIDE);
    let told = GridMsg::Have {
        window,
        kinds,
        thread,
    };
    dispatch(win, Msg::Grid(told));
    true
}

/// "Always open here" and "Never open by itself" on a pane or its tab: the word is written to
/// the settings file, or taken back when it was the one in force. True for either command.
pub fn carry(win: &mut Win, _: &mut Tx, command: CommandId, target: &Target) -> bool {
    if ![PANE_ALWAYS, PANE_NEVER].contains(&command) {
        return false;
    }
    let (Target::Thing(address), Ok(mut hub)) = (target, win.hub.try_borrow_mut()) else {
        return true;
    };
    let Some((pane, kind, word)) = by_kind::word(&hub.model.grid, command, address) else {
        return true;
    };
    let settings = &hub.model.settings;
    let word = Some(word).filter(|word| settings.opens().get(&kind) != Some(word));
    let key = SettingKey::fixed(OPENS);
    let set = SettingsMsg::Set(key.clone(), Some(settings.with_opens(kind, word)));
    step(&mut hub.model, Msg::Settings(set));
    if let Err(why) = crate::settings::pane::file::keep(&mut hub, &key) {
        let _ = writeln!(
            std::io::stderr(),
            "yi-desktop: the word was not kept: {why}"
        );
    }
    // The pane it was said on is the window's own from now: no second one joins it.
    if matches!(word, Some(Opens::Always(_))) {
        step(
            &mut hub.model,
            Msg::Grid(GridMsg::Across { pane, on: true }),
        );
    }
    true
}

#[cfg(test)]
mod tests;
