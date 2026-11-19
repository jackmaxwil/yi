//! Web app pane: the running app, live, under its browser bar.

mod sample;
pub mod scenes;
#[cfg(test)]
mod tests;

use rune::{Cx, El, col, el};
use yd_core::command::table::{Command, Route};
use yd_core::command::tools::web;
use yd_core::feed::{FeedStatus, SourceCmd};
use yd_core::ids::{FeedId, PaneId, RowId};
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef};
use yd_ui::screen::input::Input;
use yd_ui::screen::surface::{Mark, Screen, marks, surface};
use yd_ui::win::{Win, dispatch};

use super::bar::{SWITCH_FROM, bar, note};
use crate::design::handles;
pub use sample::Sample;
use yd_ui::states::status::{Status, view as state_view};

use super::states::WORDS;

/// The feed this pane shows; the `browser` source fills it.
pub const FEED: FeedId = yd_core::feed::WEB;

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Web,
    view,
    scenes: scenes::SCENES,
    commands: COMMANDS,
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Keyed(handles::thing_at)),
};

const fn source(id: yd_core::ids::CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[],
        route: Route::Source(FEED),
    }
}

/// The browser bar's buttons, as the command box and the menus list them.
pub const COMMANDS: &[Command] = &[
    source(web::OPEN, "Open Address"),
    source(web::BACK, "Back"),
    source(web::FORWARD, "Forward"),
    source(web::RELOAD, "Reload Page"),
    source(web::WIDTH, "Page Width"),
];

/// What the pane keeps: the screen its source draws to, and what the bar last refused.
#[derive(Debug, Default)]
pub struct State {
    pub screen: Screen,
    pub refused: Option<String>,
    /// Boxes over the page, in its pixels.
    pub marks: Vec<Mark>,
    /// The page's words over a blank screen: only a scene has one.
    pub sample: Option<Sample>,
    told: Option<(u16, u16)>,
}

/// Changes this pane's state from a handler.
pub fn change(win: &mut Win, pane: PaneId, edit: impl FnOnce(&mut State)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
    {
        edit(state);
    }
}

/// The page row of `feed`: `page:<instance>`.
pub fn page<'a>(pane: &PaneCx<'a>, feed: FeedId) -> Option<&'a Row> {
    let table = pane.model().tables.get(feed)?;
    table.find(&RowId::new(format!("page:{}", feed.instance)))
}

/// "200 · 38 ms": the page's last status and how long it took, either left out when unknown.
pub fn meta(page: Option<&Row>) -> String {
    let word = |cell: &str| page.and_then(|p| p.words(cell)).filter(|w| !w.is_empty());
    let ms = word("ms").map(|ms| format!("{ms} ms"));
    let parts: Vec<String> = [word("status").map(str::to_owned), ms]
        .into_iter()
        .flatten()
        .collect();
    parts.join(" · ")
}

/// The one tab: the page's host, its status and latency at the bar's right.
fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let page = page(pane, FEED);
    let title = page
        .and_then(|p| p.words("title"))
        .filter(|t| !t.is_empty());
    let tab = Tab {
        title: title.unwrap_or(PaneKind::Web.title()).to_owned(),
        meta: meta(page),
        ..Tab::of(PaneKind::Web)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// Hands the surface's input to the source: its size as a resize, the rest as it is typed.
pub fn send(win: &mut Win, pane: PaneId, feed: FeedId, input: Input) {
    let ask = match input {
        Input::Resized { width, height } => {
            let side = |v: u32| u16::try_from(v).unwrap_or(u16::MAX).max(1);
            let size = (side(width), side(height));
            let mut fresh = false;
            change(win, pane, |state| {
                fresh = state.told != Some(size);
                state.told = Some(size);
            });
            if !fresh {
                return;
            }
            SourceCmd::Resize {
                cols: size.0,
                rows: size.1,
            }
        }
        other => match other.ask() {
            Some(input) => SourceCmd::Screen(input),
            None => return,
        },
    };
    dispatch(win, Msg::Source(feed, ask));
}

fn view(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Web);
    let tokens = pane.tokens();
    let fallback = State::default();
    let state = pane.state::<State>().unwrap_or(&fallback);
    // Invariant: both layout reads are made on every run, whatever the body turns out to be.
    let wide = cx
        .layout_size(&key)
        .is_some_and(|size| size[0] >= SWITCH_FROM);
    let design = handles::layer(pane, cx, &key, &state.screen);
    let row = page(pane, FEED);
    let status = pane.model().tables.get(FEED).map(|t| t.status());
    let id = pane.pane;
    let body = match (status, row, &state.sample) {
        (Some(FeedStatus::Error(why)), _, None) => {
            state_view(pane, &key, &Status::Error(why.clone()), &WORDS)
        }
        (Some(FeedStatus::Offline), None, None) => {
            state_view(pane, &key, &Status::Offline(None), &WORDS)
        }
        (_, None, None) => state_view(pane, &key, &Status::Empty, &WORDS),
        _ => {
            let on = move |win: &mut Win, input: Input| {
                if let Some(input) = handles::took(win, input) {
                    send(win, id, FEED, input);
                }
            };
            let words = state.sample.as_ref().map(|s| sample::page(s, &key));
            el().w_full()
                .grow(1.0)
                .min_h(0.0)
                .clip_rounded(0.0)
                .child(surface(&format!("{key}.page"), &state.screen, on))
                .children(words.map(|w| w.absolute(0.0, 0.0)))
                .child(marks(tokens, &state.screen, &state.marks).absolute(0.0, 0.0))
                .children(design)
        }
    };
    let said_line = state
        .refused
        .as_deref()
        .or(row.and_then(|r| r.words("note")));
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(bar(pane, &key, row, wide))
        .children(note(tokens, &key, said_line))
        .child(body)
}
