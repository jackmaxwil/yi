//! The pane's scenes: the mockup's two files (record 4.2) fed through the `changes` feed, and
//! the comment the frame draws under line 20.

use rune::{Cx, El};
use yd_core::feed::FeedEvent;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::rows_from_json;
use yd_ui::pane::{PaneCx, SCENE_PANE, Scene, Stage};
use yd_ui::win::{Hub, Win};

use crate::files::pane::frame::framed;
use crate::files::pane::scenes::feed;

use super::{FEED, State, meta};

pub const SCENES: &[Scene] = &[
    Scene {
        name: "changes/empty",
        stage: Stage::Pane(PaneKind::Changes),
        setup: |hub| {
            let none = Msg::Feed(FEED, FeedEvent::Reset(Vec::new()));
            step(&mut hub.model, none);
        },
    },
    Scene {
        name: "changes/mockup",
        stage: Stage::View(mockup),
        setup: sample,
    },
    Scene {
        name: "changes/revert-asked",
        stage: Stage::View(mockup),
        setup: |hub| {
            sample(hub);
            if let Some(state) = hub.panes.get_mut::<State>(Some(SCENE_PANE)) {
                state.confirm = Some(("src/routes/upload.rs".to_owned(), 0));
            }
        },
    },
];

/// The two files and the comment on line 20.
pub fn sample(hub: &mut Hub) {
    feed(hub, FEED, include_str!("../mockup.changes.json"));
    if let Some(state) = hub.panes.get_mut::<State>(Some(SCENE_PANE)) {
        state.comments =
            rows_from_json(include_str!("../mockup.comments.json")).unwrap_or_default();
    }
}

/// The pane under frame 14's tab bar: its own tab first, then the tabs it shares the bar with.
fn mockup(_win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let pane = PaneCx {
        hub,
        pane: SCENE_PANE,
        focused: true,
        edits: None,
    };
    let key = pane.key(PaneKind::Changes);
    let count = super::files(hub.model.tables.get(FEED)).len().to_string();
    let tabs = [
        ("Changes", count.as_str()),
        ("Files", ""),
        ("Report", ""),
        ("localhost:5173", ""),
        ("ci-ubuntu", ""),
    ];
    framed(
        &hub.model.tokens,
        &key,
        &tabs,
        &meta(&pane),
        (super::DEF.view)(&pane, cx),
    )
}
