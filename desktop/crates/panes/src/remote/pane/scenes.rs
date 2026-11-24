//! The Remote desktop pane's scenes: frame 1 as the record draws it, alone, no host, Control.

use yd_core::command::tools::remote::{NO_HOST, RECORDED_ONLY, sample};
use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::win::Hub;

use super::{FEED, Mode, State};
use crate::web::pane::scenes::framed;

pub const SCENES: &[Scene] = &[
    Scene {
        name: "desktop/mockup",
        stage: Stage::View(|win, hub, cx| framed(win, hub, cx, PaneKind::Remote, (286.0, 190.0))),
        setup: mockup,
    },
    Scene {
        name: "remote/recorded",
        stage: Stage::Pane(PaneKind::Remote),
        setup: mockup,
    },
    Scene {
        name: "desktop/none",
        stage: Stage::Pane(PaneKind::Remote),
        setup: none,
    },
    Scene {
        name: "desktop/control",
        stage: Stage::Pane(PaneKind::Remote),
        setup: control,
    },
];

/// The recorded screen as the source sends it: its rows, then its picture in the pane's screen.
fn recorded(hub: &mut Hub) {
    let shown = sample();
    let event = match &shown {
        Ok(seen) => FeedEvent::Reset(seen.rows.clone()),
        Err(why) => FeedEvent::Status(FeedStatus::Error(why.clone())),
    };
    step(&mut hub.model, Msg::Feed(FEED, event));
    step(
        &mut hub.model,
        Msg::Feed(FEED, FeedEvent::Status(FeedStatus::Live)),
    );
    if let (Ok(seen), Some(state)) = (shown, hub.panes.get_mut::<State>(Some(SCENE_PANE))) {
        let _ = state.screen.push(seen.width, seen.height, seen.rgba);
    }
}

fn mockup(hub: &mut Hub) {
    recorded(hub);
}

fn none(hub: &mut Hub) {
    let status = FeedStatus::Error(NO_HOST.to_owned());
    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Status(status)));
}

/// Control on recorded data: the source has refused the first press, and the bar says so.
fn control(hub: &mut Hub) {
    recorded(hub);
    if let Some(state) = hub.panes.get_mut::<State>(Some(SCENE_PANE)) {
        state.mode = Mode::Control;
    }
    let first = hub
        .model
        .tables
        .get(FEED)
        .and_then(|t| t.find(&yd_core::ids::RowId::new("screen:0")));
    if let Some(row) = first.cloned() {
        let noted = FeedEvent::Upsert(row.text("note", RECORDED_ONLY));
        step(&mut hub.model, Msg::Feed(FEED, noted));
    }
}
