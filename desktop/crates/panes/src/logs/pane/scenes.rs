//! The pane's scenes: the mockup's frame, a filtered stream, and nothing yet.

use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::ids::RowId;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::rows_from_json;
use yd_ui::list::state::Lists;
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::win::Hub;

use super::FEED;
use super::state::{Filters, Level, State};

const MOCKUP: &str = include_str!("../../../../../fixtures/mockup/logs.rows.json");

pub const SCENES: &[Scene] = &[
    Scene {
        name: "logs/mockup",
        stage: Stage::Pane(PaneKind::Logs),
        setup: mockup,
    },
    Scene {
        name: "logs/filtered",
        stage: Stage::Pane(PaneKind::Logs),
        setup: filtered,
    },
    Scene {
        name: "logs/empty",
        stage: Stage::Pane(PaneKind::Logs),
        setup: empty,
    },
];

/// The mockup's fifteen lines, sent as the source would send them.
fn lines(hub: &mut Hub) {
    let event = match rows_from_json(MOCKUP) {
        Ok(rows) => FeedEvent::Reset(rows),
        Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
    };
    step(&mut hub.model, Msg::Feed(FEED, event));
}

fn filters(hub: &mut Hub, filters: Filters) {
    if let Some(state) = hub.panes.get_mut::<State>(Some(SCENE_PANE)) {
        state.filters = filters;
        state.asked = true;
    }
}

/// Record 5.5: the error line open. The mockup draws Debug off beside debug lines; a level
/// that is off hides its lines here, so Debug is on to list them.
fn mockup(hub: &mut Hub) {
    lines(hub);
    filters(
        hub,
        Filters {
            levels: [true; 4],
            ..Filters::default()
        },
    );
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists
            .list_mut("")
            .set_open(Some(RowId::new("17:58:03.412")));
    }
}

/// Only the API's warnings and errors.
fn filtered(hub: &mut Hub) {
    lines(hub);
    let mut only = Filters {
        source: Some("api".to_owned()),
        ..Filters::default()
    };
    only.flip(Level::Info);
    filters(hub, only);
}

fn empty(hub: &mut Hub) {
    step(
        &mut hub.model,
        Msg::Feed(FEED, FeedEvent::Reset(Vec::new())),
    );
    filters(hub, Filters::default());
}
