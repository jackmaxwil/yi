//! Language servers pane: two tabs, the servers with what each can do and the problems they
//! report, as lists over the `lsp` feed told apart by the prefix of their row ids.

use rune::{Cx, El, Tx, col};
use yd_core::address::thing::Thing;
use yd_core::command::by_kind::{PROBLEM_FIX, PROBLEM_OPEN};
use yd_core::command::table::{Route, Run, Target};
use yd_core::feed::{FeedStatus, LSP};
use yd_core::ids::{CommandId, FeedId, PaneId, RowId, TabId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::Rows;
use yd_ui::chrome::tabs::{SHIELD, SLIDERS, Tab, Tabs};
use yd_ui::grid_view::view::{open_file, show_pane};
use yd_ui::list::row;
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, TabAsk};
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win, dispatch};

use super::desc::{COMMANDS, LOGS, OPEN_FINDING, PROBLEMS, SERVERS, SETTINGS, place, quick_fix};
use bars::{Summary, footer, problems_head, servers_head};
use tables::{problems, servers};

use super::states::{NO_PROBLEMS, WORDS};

mod bars;
pub(super) mod scenes;
mod tables;
#[cfg(test)]
mod tests;

/// The feed this pane lists; its source declares the same name.
pub const FEED: FeedId = LSP;

const SERVERS_TAB: TabId = TabId(1);
const PROBLEMS_TAB: TabId = TabId(2);
/// What an opened server or problem needs below its row, as the record measures it, and
/// room for a wrapped line.
const OPEN_SERVER: f32 = 124.0;

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::LanguageServers,
    view,
    scenes: scenes::SCENES,
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: Some(on_tab),
    finder: Some(Finder::Keyed(thing_at)),
};

/// Which of the two lists the pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Show {
    #[default]
    Servers,
    Problems,
}

/// The pane's own state: the tab in front.
#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    pub show: Show,
}

fn shown(pane: &PaneCx<'_>) -> Show {
    pane.state::<State>()
        .map(|state| state.show)
        .unwrap_or_default()
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let model = pane.model();
    let (servers, problems) = (servers(model), problems(model, false));
    let count = |rows: usize| u32::try_from(rows).ok();
    let front = shown(pane);
    let summary = Summary::of(&problems);
    let requests = bars::requests(&servers);
    let tab = |id, icon, title: &str, rows, meta: String| Tab {
        id,
        icon,
        title: title.to_owned(),
        count: count(rows),
        meta,
        fixed: true,
        preview: false,
        dot: false,
    };
    Tabs {
        tabs: vec![
            tab(
                SERVERS_TAB,
                &SLIDERS,
                "Language servers",
                servers.len(),
                requests.map_or_else(String::new, |n| format!("{n} requests")),
            ),
            tab(
                PROBLEMS_TAB,
                &SHIELD,
                "Problems",
                problems.len(),
                summary.to_string(),
            ),
        ],
        active: Some(match front {
            Show::Servers => SERVERS_TAB,
            Show::Problems => PROBLEMS_TAB,
        }),
        adds: false,
        tool: None,
    }
}

fn on_tab(win: &mut Win, pane: PaneId, ask: TabAsk) {
    let show = match ask {
        TabAsk::Show(tab) if tab == SERVERS_TAB => Show::Servers,
        TabAsk::Show(tab) if tab == PROBLEMS_TAB => Show::Problems,
        TabAsk::Show(_) | TabAsk::Close(_) | TabAsk::New => return,
    };
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
    {
        state.show = show;
    }
}

/// The server or problem a keyed element of this pane stands for.
fn thing_at(hub: &Hub, _: PaneId, key: &str) -> Option<Thing> {
    let id = row::id_of(key)?;
    let found = hub.model.tables.get(LSP)?.find(&id)?;
    let desc = if id.as_str().starts_with("server:") {
        &SERVERS
    } else {
        &PROBLEMS
    };
    row::thing(desc, found).ok()
}

/// Carries out what a server's and a problem's buttons and right-click rows ask of the
/// window: its log, the settings, its finding, its line, its fix. Ignoring one is not built.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(at) = target else {
        return false;
    };
    let shows = |win: &mut Win, tx: &mut Tx, kind| show_pane(win, tx, kind).is_some();
    match (command, at.kind()) {
        (LOGS, Kind::LanguageServer) => {
            let name = at.id().strip_prefix("server:").unwrap_or(at.id());
            crate::logs::pane::only(win, tx, name)
        }
        (SETTINGS, Kind::LanguageServer) => shows(win, tx, PaneKind::Settings),
        (OPEN_FINDING, Kind::Problem) => shows(win, tx, PaneKind::Findings),
        (PROBLEM_OPEN | PROBLEM_FIX, Kind::Problem) => {
            let id = RowId::new(at.id());
            let found = win.hub.try_borrow().ok().and_then(|hub| {
                let row = problems(&hub.model, false).find(&id).cloned()?;
                Some((place(&row), quick_fix(&row)))
            });
            match (command, found) {
                (PROBLEM_OPEN, Some((Some(line), _))) => open_file(win, tx, line),
                (PROBLEM_FIX, Some((_, (fix, Some(aim))))) => {
                    let run = Run::new(fix).on(Target::Thing(aim));
                    dispatch(win, Msg::Command(Route::Source(LSP), run));
                }
                _ => return false,
            }
            true
        }
        _ => false,
    }
}

/// The room the servers' list takes: titles, a line a server, the open one's detail.
// ponytail: estimated, not measured; a detail that wraps further scrolls inside its list.
fn servers_height(pane: &PaneCx<'_>, rows: usize, live: bool) -> f32 {
    let row = pane.tokens().row;
    let lines = rows.saturating_add(usize::from(!live)).saturating_add(1);
    let lines = f32::from(u16::try_from(lines).unwrap_or(u16::MAX));
    let open = pane
        .state::<yd_ui::list::state::Lists>()
        .and_then(|lists| lists.list("servers"))
        .is_some_and(|list| list.open().is_some());
    // With no server the list says its state, which needs the room of a few rows.
    let lines = if rows == 0 { lines.max(5.0) } else { lines };
    row * lines + if open { OPEN_SERVER } else { 0.0 }
}

fn servers_tab(pane: &PaneCx<'_>, key: &str) -> El {
    let model = pane.model();
    let (servers, problems) = (servers(model), problems(model, false));
    let tokens = pane.tokens();
    let live = *servers.status() == FeedStatus::Live;
    let high = servers_height(pane, servers.len(), live);
    let list = worded(
        pane,
        PaneKind::LanguageServers,
        "servers",
        &SERVERS,
        Some(&servers),
        &WORDS,
        false,
    );
    col()
        .child(servers_head(tokens, key, &servers))
        .child(col().w_full().h(high).min_h(0.0).child(list))
        .child(problems_head(tokens, &format!("{key}.problems-head"), &problems).mt(8.0))
        .child(worded(
            pane,
            PaneKind::LanguageServers,
            "problems",
            &PROBLEMS,
            Some(&problems),
            &NO_PROBLEMS,
            false,
        ))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
}

fn problems_tab(pane: &PaneCx<'_>, key: &str) -> El {
    let model = pane.model();
    let grouped = problems(model, true);
    let head = problems_head(pane.tokens(), &format!("{key}.pane-header"), &grouped);
    let list = worded(
        pane,
        PaneKind::LanguageServers,
        "problems",
        &PROBLEMS,
        Some(&grouped),
        &NO_PROBLEMS,
        false,
    );
    col().w_full().grow(1.0).min_h(0.0).child(head).child(list)
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::LanguageServers);
    let tokens = pane.tokens();
    let body = match shown(pane) {
        Show::Servers => servers_tab(pane, &key),
        Show::Problems => problems_tab(pane, &key),
    };
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(body)
        .child(footer(tokens, &key))
}
