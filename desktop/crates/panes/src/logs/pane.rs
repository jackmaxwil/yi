//! Logs pane: the server's worker log, the app's own log, and what the model holds about the
//! server and the agent, in one stream filtered by level, source and text.

mod header;
mod scenes;
mod shown;
mod state;
#[cfg(test)]
mod tests;
mod wire;

use std::rc::Rc;

use rune::{Cursor, Cx, El, Tx, col, row, text};
use yd_core::address::attach::ATTACH as ATTACH_LINE;
use yd_core::address::thing::Thing;
use yd_core::command::by_kind::{LOG_AROUND, LOG_SOURCE, NEW_FINDING};
use yd_core::command::table::{Route, Run, Target};
use yd_core::feed::{FeedStatus, SourceCmd, Table};
use yd_core::ids::{CommandId, FeedId, PaneId, RowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows};
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::list::row::{id_of, thing};
use yd_ui::list::state::Lists;
use yd_ui::list::view::worded;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::theme::look::{META, colour};
use yd_ui::win::{Hub, Win, dispatch};

use super::desc::{AROUND, ATTACH, COMMANDS, DESC, MAKE_FINDING, ONLY_SOURCE, SHOW_IN_TRACE};
use shown::{Shown, append, shown, stamp, when};
use state::{Filters, Made, State, change, drawn};
pub use wire::lines;

use super::states::WORDS;

/// The feed this pane lists; its source declares the same name.
pub const FEED: FeedId = FeedId::new("logs");

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Logs,
    view,
    scenes: scenes::SCENES,
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Keyed(thing_at)),
};

/// The list for this pane: kept while nothing it reads changed, extended by the few lines
/// that were appended since, made again otherwise.
fn made(pane: &PaneCx<'_>) -> (Rc<Shown>, Filters) {
    let fallback = State::default();
    let state = pane.state::<State>().unwrap_or(&fallback);
    let model = pane.model();
    let lines = wire::lines(model, pane.thread());
    let text = drawn(pane).map_or("", |field| field.text.as_str());
    let feed = model.tables.get(FEED);
    let rest = stamp(&lines, &state.filters, text);
    let version = feed.map_or(0, Table::version);
    if let Ok(mut slot) = state.made.try_borrow_mut()
        && let Some(kept) = slot.as_mut().filter(|kept| kept.rest == rest)
    {
        let len = feed.map_or(0, Table::len);
        if (kept.version, kept.fed) == (version, len) {
            return (kept.shown.clone(), state.filters.clone());
        }
        let same_tail = kept
            .fed
            .checked_sub(1)
            .and_then(|at| feed?.get(at))
            .map(|row| &row.id)
            == kept.tail.as_ref();
        if let (Some(table), true, Some(shown)) = (
            feed,
            same_tail && len > kept.fed,
            Rc::get_mut(&mut kept.shown),
        ) && append(shown, table, kept.fed, &state.filters, text)
        {
            (kept.version, kept.fed) = (version, len);
            kept.tail = len
                .checked_sub(1)
                .and_then(|at| table.get(at))
                .map(|row| row.id.clone());
            return (kept.shown.clone(), state.filters.clone());
        }
    }
    let now = Rc::new(shown(feed, &lines, &state.filters, text));
    if let Ok(mut slot) = state.made.try_borrow_mut() {
        let len = feed.map_or(0, Table::len);
        *slot = Some(Made {
            rest,
            fed: len,
            tail: len
                .checked_sub(1)
                .and_then(|at| feed?.get(at))
                .map(|row| row.id.clone()),
            version,
            shown: now.clone(),
        });
    }
    (now, state.filters.clone())
}

/// The one tab: its name, and "5 sources · live" at the bar's right.
fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let (shown, _) = made(pane);
    let status = pane.model().tables.get(FEED).map(|table| table.status());
    let live = match status {
        Some(FeedStatus::Live) => "live",
        Some(FeedStatus::Offline | FeedStatus::Error(_)) => "offline",
        Some(FeedStatus::Empty) => "no lines",
        Some(FeedStatus::Loading) | None => "loading",
    };
    let noun = if shown.sources.len() == 1 {
        "source"
    } else {
        "sources"
    };
    // The word is the feed's, which can hold nothing while the model's own lines are listed.
    let lines = shown.total;
    let live = match (lines, live) {
        (0, live) => live.to_owned(),
        (1, "no lines" | "loading") => "1 line".to_owned(),
        (n, "no lines" | "loading") => format!("{n} lines"),
        (_, live) => live.to_owned(),
    };
    let tab = Tab {
        meta: format!("{} {noun} · {live}", shown.sources.len()),
        ..Tab::of(PaneKind::Logs)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// The line with this id: the feed's row, else one the model gave.
fn line(hub: &Hub, pane: PaneId, id: &RowId) -> Option<Row> {
    let feed = hub.model.tables.get(FEED).and_then(|table| table.find(id));
    feed.cloned().or_else(|| {
        let thread = yd_ui::shown::of_pane(hub, pane);
        let lines = wire::lines(&hub.model, thread);
        lines.into_iter().find(|row| row.id == *id)
    })
}

/// What a line's key stands for, for pick mode and the right-click menu.
fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    let row = line(hub, pane, &id_of(key)?)?;
    thing(&DESC, &row).ok()
}

/// Asks the source for lines the first time the pane is laid out: sources start on an ask.
fn start(win: &mut Win, pane: PaneId) {
    let mut fresh = false;
    change(win, pane, |state| {
        fresh = !state.asked;
        state.asked = true;
    });
    if fresh {
        dispatch(win, Msg::Source(FEED, SourceCmd::Refresh));
    }
}

/// The pill over the list's foot that goes back to the newest line and closes the open one.
fn latest(key: &str, pane: PaneId, tokens: &Tokens) -> El {
    let list = format!("{key}.rows");
    let pill = row()
        .key(format!("{key}.latest"))
        .h(19.0)
        .px(8.0)
        .center_y()
        .radius(9.5)
        .fill(colour(tokens.accent.alpha(0.9)))
        .cursor(Cursor::Pointer)
        .child(
            text("Jump to latest ↓")
                .font(META)
                .color(colour(tokens.window)),
        )
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            if let Ok(mut hub) = win.hub.try_borrow_mut()
                && let Some(lists) = hub.panes.get_mut::<Lists>(Some(pane))
            {
                lists.list_mut("").set_open(None);
            }
            tx.instant();
            tx.scroll_to_bottom_instant(list.clone());
        });
    row()
        .pin(Some(0.0), None, Some(0.0), Some(8.0))
        .h(19.0)
        .center()
        .child(pill)
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Logs);
    let tokens = pane.tokens();
    let (shown, filters) = made(pane);
    let id = pane.pane;
    let hidden = shown.table.len() == 0 && shown.total > 0;
    let body = if hidden {
        let said = format!("No line matches the filters · {} hidden", shown.total);
        row()
            .key(format!("{key}.hidden"))
            .w_full()
            .h(tokens.row)
            .px(tokens.gutter)
            .center_y()
            .child(text(said).font(META).color(colour(tokens.faint)))
    } else {
        let list = worded(
            pane,
            PaneKind::Logs,
            "",
            &DESC,
            Some(&shown.table),
            &WORDS,
            true,
        );
        row().w_full().grow(1.0).min_h(0.0).child(list)
    };
    let away = !header::following(pane) && !hidden;
    let list = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(body)
        .children(away.then(|| latest(&key, id, tokens)));
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .on_layout_tx(move |win: &mut Win, _rect, _tx: &mut Tx| start(win, id))
        .child(header::bar(pane, &key, &shown, &filters))
        .child(list)
        .child(header::foot(pane, &key, &shown))
}

/// The panes of this kind a window has, or the scene's one.
fn panes(hub: &Hub) -> Vec<PaneId> {
    let all = hub.model.grid.panes.iter();
    let mut found: Vec<PaneId> = all
        .filter(|pane| pane.kind == PaneKind::Logs)
        .map(|pane| pane.id)
        .collect();
    if found.is_empty() {
        found.push(SCENE_PANE);
    }
    found
}

/// Opens the Trace pane of this window, on the step `line` came from when it names one.
fn show_in_trace(win: &mut Win, tx: &mut Tx, line: &RowId) -> bool {
    let step = win.hub.try_borrow().ok().and_then(|hub| {
        let row = panes(&hub)
            .iter()
            .find_map(|pane| self::line(&hub, *pane, line))?;
        row.words("span").map(RowId::new)
    });
    let Some(trace) = yd_ui::grid_view::view::show_pane(win, tx, PaneKind::Trace) else {
        return false;
    };
    if let (Some(step), Ok(mut hub)) = (step, win.hub.try_borrow_mut())
        && let Some(lists) = hub.panes.get_mut::<Lists>(Some(trace))
    {
        lists.list_mut("").set_open(Some(step));
    }
    true
}

/// Shows this window's Logs pane listing one source alone: another pane's way to its log.
pub fn only(win: &mut Win, tx: &mut Tx, source: &str) -> bool {
    let Some(pane) = yd_ui::grid_view::view::show_pane(win, tx, PaneKind::Logs) else {
        return false;
    };
    change(win, pane, |state| {
        state.filters = Filters {
            source: Some(source.to_owned()),
            ..Filters::default()
        };
    });
    true
}

/// Carries out a button of an opened line, or a right-click item on one: attach it, make a
/// finding of it, open the trace on its step, or filter this pane to its source or moment.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(address) = target else {
        return false;
    };
    if address.kind() != Kind::LogLine {
        return false;
    }
    let id = RowId::new(address.id());
    let passed = match command {
        ATTACH => Some(ATTACH_LINE),
        MAKE_FINDING => Some(NEW_FINDING),
        _ => None,
    };
    if let Some(passed) = passed {
        let run = Run::new(passed).on(target.clone());
        dispatch(win, Msg::Command(Route::Model, run));
        return true;
    }
    if command == SHOW_IN_TRACE {
        return show_in_trace(win, tx, &id);
    }
    let only = command == ONLY_SOURCE || command == LOG_SOURCE;
    if !only && command != AROUND && command != LOG_AROUND {
        return false;
    }
    let found = {
        let Ok(hub) = win.hub.try_borrow() else {
            return false;
        };
        let targets = panes(&hub);
        let row = targets.iter().find_map(|pane| line(&hub, *pane, &id));
        row.map(|row| (row.words("source").map(str::to_owned), when(&row), targets))
    };
    let Some((source, at, targets)) = found else {
        return false;
    };
    for pane in targets {
        change(win, pane, |state| {
            state.filters = if only {
                Filters {
                    source: source.clone(),
                    ..Filters::default()
                }
            } else {
                Filters {
                    levels: [true; 4],
                    around: at,
                    ..Filters::default()
                }
            };
        });
    }
    true
}
