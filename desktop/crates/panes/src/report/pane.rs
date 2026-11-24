//! Report pane: the newest page the agent wrote in the thread, drawn by the `browser` source as
//! a second page at a 600 point design width.

mod sample;
mod scenes;
#[cfg(test)]
mod tests;

use rune::{Cx, El, Tx, col, el, row, text};
use yd_core::command::by_kind::turn_of;
use yd_core::command::table::Run;
use yd_core::command::tools::web;
use yd_core::feed::{FeedStatus, SourceCmd};
use yd_core::ids::{FeedId, PaneId};
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::CallId;
use yd_ui::chrome::tabs::{FILE, Tab, Tabs};
use yd_ui::kit::icons::glyph;
use yd_ui::pane::{PaneCx, PaneDef};
use yd_ui::screen::input::Input;
use yd_ui::screen::surface::{Screen, surface};
use yd_ui::theme::look::{CODE_LINE, LABEL, colour, white};
use yd_ui::win::{Win, dispatch};

use crate::web::bar::RELOAD;
use crate::web::pane::send;
pub use sample::Sample;
use yd_ui::states::status::{Status, body, view as state_view};

use super::states::WORDS;
use crate::copy::empty::threadless;

/// The page's own instance of the `browser` source: the web pane has the first.
pub const FEED: FeedId = yd_core::feed::REPORT;
/// The width the agent's page is laid out at, in points.
pub const DESIGN: f32 = 600.0;

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Report,
    view,
    scenes: scenes::SCENES,
    commands: &[],
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: None,
};

/// What the pane keeps: the screen the page is drawn to and the room it was given.
#[derive(Debug, Default)]
pub struct State {
    pub screen: Screen,
    /// The page area's size in points, as last laid out.
    pub room: Option<(f32, f32)>,
    /// The page's words over a blank screen: only a scene has one.
    pub sample: Option<Sample>,
    /// The address last asked of the source.
    opened: Option<String>,
}

/// A page the agent wrote: the call that wrote it, where, how big, and in which turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub call: CallId,
    pub path: String,
    pub bytes: Option<usize>,
    pub turn: Option<u32>,
}

/// The newest `.html` file the thread's `write` or `edit` calls named.
pub fn newest(thread: &Thread) -> Option<Written> {
    let pages = thread.transcript.tools.values().filter_map(|call| {
        let wrote = matches!(call.name.as_str(), "write" | "edit");
        let path = call.args.get("path")?.as_str()?;
        let html = path.to_ascii_lowercase().ends_with(".html");
        (wrote && html).then_some((call, path))
    });
    let (call, path) = pages.max_by_key(|(call, _)| (call.started, call.id.clone()))?;
    let bytes = call
        .args
        .get("content")
        .and_then(|c| c.as_str())
        .map(str::len);
    Some(Written {
        call: call.id.clone(),
        path: path.to_owned(),
        bytes,
        turn: turn_of(thread, path),
    })
}

/// A `file://` address for `path`, under the thread's folder when it is relative. Bytes a
/// path may not hold as they are (`%`, `#`, `?`, spaces, controls, non-ASCII) are escaped.
pub fn address(folder: &str, path: &str) -> String {
    let full = match path.starts_with('/') {
        true => path.to_owned(),
        false => format!(
            "{}/{}",
            folder.trim_end_matches('/'),
            path.trim_start_matches("./")
        ),
    };
    let mut out = String::from("file://");
    for byte in full.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'-' | b'~' => {
                out.push(char::from(byte))
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// "3.1 KB", or bytes under one.
pub fn size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        _ => format!("{:.1} KB", bytes as f64 / 1024.0),
    }
}

/// How much the page is scaled in a room `width` points wide.
pub fn zoom(width: f32) -> f32 {
    (width / DESIGN).clamp(0.2, 1.1)
}

fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let written = pane.thread().and_then(newest);
    let parts = [
        written.as_ref().and_then(|w| w.bytes).map(size),
        written
            .as_ref()
            .and_then(|w| w.turn)
            .map(|turn| format!("turn {turn}")),
    ];
    let tab = Tab {
        meta: parts.into_iter().flatten().collect::<Vec<_>>().join(" · "),
        ..Tab::of(PaneKind::Report)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

/// Asks the source for `url` unless it was the last asked.
fn open(win: &mut Win, pane: PaneId, url: &str) {
    let mut fresh = false;
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
    {
        fresh = state.opened.as_deref() != Some(url);
        state.opened = Some(url.to_owned());
    }
    if fresh {
        let mut run = Run::new(web::OPEN);
        run.text = Some(url.to_owned());
        dispatch(win, Msg::Source(FEED, SourceCmd::Run(run)));
    }
}

/// The page area was laid out: keep its size, and make sure the source shows the file.
fn laid(win: &mut Win, pane: PaneId, room: (f32, f32), url: &str) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
        && state.room != Some(room)
    {
        state.room = Some(room);
    }
    open(win, pane, url);
}

/// The surface's box is the page at its zoom: the source lays the page out 600 wide.
fn resized(win: &mut Win, pane: PaneId, input: Input, scale: f32) {
    let input = match input {
        Input::Resized { height, .. } => Input::Resized {
            width: DESIGN as u32,
            height: (height as f32 / scale).round() as u32,
        },
        other => other,
    };
    send(win, pane, FEED, input);
}

fn bar(pane: &PaneCx<'_>, key: &str, written: &Written, scale: f32) -> El {
    let tokens = pane.tokens();
    let name = written.path.rsplit('/').next().unwrap_or(&written.path);
    let turn = written.turn.map(|turn| format!("turn {turn}"));
    let faint = |words: String| text(words).font(LABEL).color(colour(tokens.faint));
    let reload = row()
        .key(format!("{key}.reload"))
        .label("Reload")
        .child(glyph(&RELOAD, 10.0, tokens.faint))
        .on_press_tx(|win: &mut Win, _: &mut Tx| {
            dispatch(win, Msg::Source(FEED, SourceCmd::Refresh));
        });
    row()
        .key(format!("{key}.page-bar"))
        .w_full()
        .h(tokens.bar)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .clip_rounded(0.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.inner_lines)))
        .child(glyph(&FILE, 10.0, tokens.dim))
        .child(
            row()
                .w(0.0)
                .grow(1.0)
                .clip_rounded(0.0)
                .child(text(name).font(CODE_LINE).color(colour(tokens.fg))),
        )
        .children(turn.map(faint))
        .child(faint(format!("{:.0}%", scale * 100.0)).key(format!("{key}.zoom")))
        .child(reload)
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Report);
    let fallback = State::default();
    let state = pane.state::<State>().unwrap_or(&fallback);
    let thread = pane.thread();
    let Some((written, thread)) = thread.and_then(|t| Some((newest(t)?, t))) else {
        let (status, words) = threadless(pane).unwrap_or((Status::Empty, &WORDS));
        return body(pane, PaneKind::Report, &status, words);
    };
    let url = address(yd_ui::shown::folder(thread), &written.path);
    let scale = zoom(state.room.map_or(DESIGN, |(w, _)| w));
    let id = pane.pane;
    let status = pane.model().tables.get(FEED).map(|t| t.status());
    let page = match status {
        Some(FeedStatus::Error(why)) if state.sample.is_none() => {
            state_view(pane, &key, &Status::Error(why.clone()), &WORDS)
        }
        _ => {
            let on = move |win: &mut Win, input: Input| resized(win, id, input, scale);
            let words = state.sample.as_ref().map(|s| sample::page(s, &key, scale));
            el().w(DESIGN * scale)
                .h_full()
                .clip_rounded(0.0)
                .child(surface(&format!("{key}.page"), &state.screen, on))
                .children(words.map(|w| w.absolute(0.0, 0.0)))
        }
    };
    let watched = url.clone();
    let room = col()
        .key(format!("{key}.page.{}", written.call.as_str()))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(colour(yd_ui::theme::scale::PAPER.of(pane.tokens())))
        .on_layout_tx(move |win: &mut Win, r: [f32; 4], _: &mut Tx| {
            laid(win, id, (r[2], r[3]), &watched);
        })
        .child(page);
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(bar(pane, &key, &written, scale))
        .child(room)
}
