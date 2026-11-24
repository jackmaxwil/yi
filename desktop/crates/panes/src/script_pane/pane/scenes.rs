//! The script pane's scenes: the Queue depth plugin of frame 13, and one whose script fails.

use yd_core::feed::{FETCH, FeedEvent};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_ui::pane::{Scene, Stage};
use yd_ui::win::Hub;

use super::library::{Plugin, grant};
use super::{library, settle};

pub const MANIFEST: &str = include_str!("fixtures/plugin.json");
/// Version 6 of the pane, before "Make retries stand out".
pub const BEFORE: &str = include_str!("fixtures/pane.v6.luau");
pub const SOURCE: &str = include_str!("fixtures/pane.luau");
pub const BROKEN: &str = include_str!("fixtures/broken.luau");
pub const ANSWER: &str = include_str!("fixtures/queue.json");
pub const NAME: &str = "queue-depth";
pub const URL: &str = "http://ci-ubuntu:9100/queue";

pub const SCENES: &[Scene] = &[
    Scene {
        name: "script/queue-depth",
        stage: Stage::Pane(PaneKind::Script),
        setup: queue_depth,
    },
    Scene {
        name: "script/error",
        stage: Stage::Pane(PaneKind::Script),
        setup: error,
    },
];

/// The grant switched on and the queue's answer arrived from the fetch source, as rows.
pub fn answered(hub: &mut Hub) {
    grant(&mut hub.model, NAME, "ci-ubuntu:9100", true);
    let row = Row::new(["get:", URL].concat())
        .text("url", URL)
        .text("status", "200")
        .text("body", ANSWER)
        .text("error", "");
    step(&mut hub.model, Msg::Feed(FETCH, FeedEvent::Upsert(row)));
}

/// Yi wrote version 6, then edited one line: the pane reloaded in place.
pub fn queue_depth(hub: &mut Hub) {
    answered(hub);
    with(hub, BEFORE, SOURCE);
}

/// A save that fails as it runs: the error with its line over the last picture that drew.
fn error(hub: &mut Hub) {
    answered(hub);
    with(hub, SOURCE, BROKEN);
}

fn with(hub: &mut Hub, first: &str, then: &str) {
    if let Some(library) = library(hub) {
        library.add(Plugin::new(NAME, MANIFEST, first, ""));
    }
    settle(hub, 0.0);
    if let Some(plugin) = library(hub).and_then(|l| l.find_mut(NAME)) {
        plugin.reload(MANIFEST, then);
    }
    settle(hub, 0.0);
}
