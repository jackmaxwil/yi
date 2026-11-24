//! The web pane's scenes: the mockup's compact form, and Chrome not found.

use rune::{Cx, El, col};
use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::reduce::turns::ToolCall;
use yd_core::row::Row;
use yd_ui::grid_view::view::alone;
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::screen::input::Point;
use yd_ui::screen::surface::Mark;
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win};

use super::{FEED, Sample, State};

const TEXT: &str = include_str!("../../../../../fixtures/mockup/panes/web.text.json");
/// The mockup's page area at 2x: 286 points wide, under the tab bar and the browser bar.
const BLANK: (u32, u32) = (572, 368);

pub const SCENES: &[Scene] = &[
    Scene {
        name: "web/mockup",
        stage: Stage::View(|win, hub, cx| framed(win, hub, cx, PaneKind::Web, (286.0, 226.0))),
        setup: mockup,
    },
    Scene {
        name: "web/missing",
        stage: Stage::Pane(PaneKind::Web),
        setup: missing,
    },
];

/// The pane alone at the size frame 1 gives it, at the window's corner.
pub fn framed(win: &Win, hub: &Hub, cx: &mut Cx<Win>, kind: PaneKind, (w, h): (f32, f32)) -> El {
    let tokens = &hub.model.tokens;
    col()
        .w_full()
        .h_full()
        .padding(16.0)
        .fill(colour(tokens.window))
        .child(
            col()
                .w(w)
                .h(h)
                .clip_rounded(0.0)
                .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
                .child(alone(win, hub, cx, kind)),
        )
}

/// A fixture's on-screen strings. Incident: this crate cannot name the JSON reader, so they
/// are read through a tool call's arguments.
pub fn strings(fixture: &str) -> Vec<String> {
    let holder = ToolCall {
        args: fixture.parse().unwrap_or_default(),
        ..ToolCall::default()
    };
    let listed = holder
        .args
        .get("strings")
        .and_then(|s| s.as_array().cloned());
    let words = listed.unwrap_or_default().into_iter();
    words
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}

/// Frame 1's page: its row as the source sends it, its words, and the agent's pointer.
fn mockup(hub: &mut Hub) {
    let s = strings(TEXT);
    let at = |i: usize| s.get(i).cloned().unwrap_or_default();
    let meta = at(1);
    let (status, took) = meta.split_once(" · ").unwrap_or(("", &meta));
    let row = Row::new("page:0")
        .text("url", format!("http://{}", at(2)))
        .text("title", at(0))
        .text("status", status)
        .text("ms", took.trim_end_matches(" ms"))
        .text("width", "")
        .text("can_back", "yes")
        .text("can_forward", "no")
        .text("loading", "no")
        .text("note", "");
    step(
        &mut hub.model,
        Msg::Feed(FEED, FeedEvent::Status(FeedStatus::Live)),
    );
    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(row)));
    let Some(state) = hub.panes.get_mut::<State>(Some(SCENE_PANE)) else {
        return;
    };
    state.sample = Some(Sample {
        nav: s.get(4..9).map(<[String]>::to_vec).unwrap_or_default(),
        alert: at(9),
        look: None,
        drop: at(10),
        button: at(11),
    });
    // The record puts the pointer over the drop zone's button: 60% across it, 56% down.
    state.marks = vec![Mark {
        at: Point { x: 336, y: 256 },
        width: 28,
        height: 36,
        label: at(12),
    }];
    let (w, h) = BLANK;
    let white = vec![255; (w * h * 4) as usize];
    let _ = state.screen.push(w, h, white);
}

/// Chrome is not installed: the status the source sends, and no page.
fn missing(hub: &mut Hub) {
    let why = "Chrome was not found; looked in /Applications/Google Chrome.app/Contents/MacOS/\
               Google Chrome, /Applications/Chromium.app/Contents/MacOS/Chromium, google-chrome, \
               chromium on PATH";
    let status = FeedStatus::Error(why.to_owned());
    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Status(status)));
}
