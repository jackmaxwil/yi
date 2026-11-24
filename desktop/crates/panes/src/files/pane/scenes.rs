//! The pane's scenes: the mockup's sample tree (record 4.1) fed through the real feeds and
//! the thread's tool calls replayed as server lines.

use rune::{Cx, El};
use yd_core::feed::FeedEvent;
use yd_core::ids::FeedId;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::rows_from_json;
use yd_core::wire::event::WireEvent;
use yd_ui::pane::{PaneCx, SCENE_PANE, Scene, Stage};
use yd_ui::win::{Hub, Win};

use super::frame::framed;
use super::{CHANGES, FEED, State};

pub const SCENES: &[Scene] = &[
    Scene {
        name: "files/empty",
        stage: Stage::Pane(PaneKind::Files),
        setup: empty,
    },
    Scene {
        name: "files/mockup",
        stage: Stage::View(mockup),
        setup: |hub| sample(hub, None),
    },
    Scene {
        name: "files/search-open",
        stage: Stage::View(|win, hub, cx| copy(win, hub, cx, "Files · search pressed")),
        setup: |hub| sample(hub, Some("")),
    },
    Scene {
        name: "files/typing",
        stage: Stage::View(|win, hub, cx| copy(win, hub, cx, "Files · typing")),
        setup: |hub| sample(hub, Some("rate")),
    },
];

/// The working folder read, with nothing in it and nothing changed.
fn empty(hub: &mut Hub) {
    for feed in [FEED, CHANGES] {
        step(
            &mut hub.model,
            Msg::Feed(feed, FeedEvent::Reset(Vec::new())),
        );
    }
}

/// The sample: the tree, the changed files, one selected, and the agent's reads and edits.
/// Invariant: a scene brings its own thread only to a hub that has none (a window's scene).
pub fn sample(hub: &mut Hub, search: Option<&str>) {
    feed(hub, FEED, include_str!("../mockup.tree.json"));
    feed(hub, CHANGES, include_str!("../mockup.changes.json"));
    let alone = hub.model.wire.threads.is_empty();
    for line in include_str!("../mockup.wire.jsonl")
        .lines()
        .filter(|_| alone)
    {
        step(&mut hub.model, Msg::Wire(WireEvent::Line(line.to_owned())));
    }
    if let Some(state) = hub.panes.get_mut::<State>(Some(SCENE_PANE)) {
        state.selected = Some("change:src/middleware/rate_limit.rs".to_owned());
        state.search = search.map(str::to_owned);
    }
}

/// Folds a fixture's rows into a feed's table; a fixture that does not parse leaves it empty.
pub fn feed(hub: &mut Hub, id: FeedId, rows: &str) {
    let rows = rows_from_json(rows).unwrap_or_default();
    step(&mut hub.model, Msg::Feed(id, FeedEvent::Reset(rows)));
}

/// The pane inside the frame's own tab, labelled as frame 20 labels its first copy.
fn mockup(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    copy(win, hub, cx, "Files · at rest")
}

/// One of frame 20's three copies of the pane, under its label.
fn copy(_win: &Win, hub: &Hub, cx: &mut Cx<Win>, label: &str) -> El {
    let pane = PaneCx {
        hub,
        pane: SCENE_PANE,
        focused: true,
        edits: None,
    };
    let key = pane.key(PaneKind::Files);
    framed(
        &hub.model.tokens,
        &key,
        &[(label, "")],
        "",
        (super::DEF.view)(&pane, cx),
    )
}
