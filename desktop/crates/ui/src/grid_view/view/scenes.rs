//! The grid's scenes: the five saved layouts, chat on the left, and a pane in the hand, each
//! with the tabs the mockup's first frame gives its panes (record 2.7.4).

use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::grid::layout::{ChatSide, Point};
use yd_core::grid::saved::Slug;
use yd_core::grid::step::GridMsg;
use yd_core::ids::{TabId, WindowId};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;

use super::{Grip, Grips, grid};
use crate::chrome::tabs::{Tab, Tabs, icon_of};
use crate::grid_view::drag::{Gesture, Held};
use crate::pane::{Scene, Stage};
use crate::win::Hub;

/// The window a single-window run has, and so the one every scene draws.
pub const WINDOW: WindowId = WindowId(0);

/// What the mockup writes at the right of a bar, by the title of the tab in front.
const META: &str = include_str!("../../../../../fixtures/mockup/data/PMETA.json");

/// A tab as the mockup writes one: its title and the count after it.
type Titled = (&'static str, Option<u32>);

/// The tabs of each pane in the mockup's first frame.
const TABS: &[(PaneKind, &[Titled])] = &[
    (PaneKind::Files, &[("Files", None)]),
    (PaneKind::Changes, &[("Changes", Some(2))]),
    (
        PaneKind::Terminal,
        &[("zsh", None), ("agent: cargo test · waiting", None)],
    ),
    (PaneKind::Web, &[("localhost:5173", None)]),
    (PaneKind::Remote, &[("ci-ubuntu", None)]),
    (PaneKind::Report, &[("Report", None)]),
    (
        PaneKind::Code,
        &[("rate_limit.rs", None), ("upload.rs", None)],
    ),
    (PaneKind::Chat, &[("Chat", None)]),
];

/// The meta the fixture gives a tab title: one `"title": "meta"` pair a line.
fn meta_of(title: &str) -> &'static str {
    let pair = |line: &'static str| {
        let (name, meta) = line.trim().trim_end_matches(',').split_once("\": \"")?;
        Some((name.strip_prefix('"')?, meta.strip_suffix('"')?))
    };
    let found = META
        .lines()
        .filter_map(pair)
        .find(|(name, _)| *name == title);
    found.map_or("", |(_, meta)| meta)
}

/// Shows a saved layout through the grid's own step and gives each pane the mockup's tabs.
pub(super) fn show(hub: &mut Hub, saved: Slug) {
    step(
        &mut hub.model,
        Msg::Grid(GridMsg::Show {
            window: WINDOW,
            saved,
        }),
    );
    if hub.model.grid.side == ChatSide::Left {
        step(
            &mut hub.model,
            Msg::Command(Route::Model, Run::new(FLIP_CHAT)),
        );
    }
    let panes = hub.model.grid.panes.clone();
    for pane in panes {
        let titles = TABS.iter().find(|(kind, _)| *kind == pane.kind);
        let Some(tabs) = hub.panes.get_mut::<Tabs>(Some(pane.id)) else {
            continue;
        };
        let titled = titles.map_or(&[][..], |(_, titles)| *titles);
        tabs.active = None;
        tabs.tabs = (0u32..)
            .zip(titled)
            .map(|(id, (title, count))| Tab {
                id: TabId(id),
                icon: icon_of(pane.kind),
                title: (*title).to_owned(),
                count: *count,
                meta: meta_of(title).to_owned(),
                fixed: false,
                preview: false,
                dot: false,
            })
            .collect();
    }
    if let Some(grips) = hub.panes.get_mut::<Grips>(None) {
        grips.set(WINDOW, Grip::default());
    }
}

fn chat_left(hub: &mut Hub) {
    show(hub, "everything");
    step(
        &mut hub.model,
        Msg::Command(Route::Model, Run::new(FLIP_CHAT)),
    );
}

/// The terminal held `by` from home with the pointer at `at`, leaning 2.4 degrees.
fn holding(hub: &mut Hub, by: [f32; 2], at: Point) {
    show(hub, "review");
    let found = hub
        .model
        .grid
        .panes
        .iter()
        .find(|pane| pane.kind == PaneKind::Terminal);
    let Some(terminal) = found.map(|pane| pane.id) else {
        return;
    };
    let held = Held::posed(terminal, by, 2.4, Some(at));
    if let Some(grips) = hub.panes.get_mut::<Grips>(None) {
        let gesture = Gesture::Pane(held);
        grips.set(
            WINDOW,
            Grip {
                gesture,
                ..Grip::default()
            },
        );
    }
}

/// Record 2.11.2's posed still: the terminal held at (150, −150) by its bar, the pointer
/// over the middle of the Changes pane it would swap with.
fn dragging(hub: &mut Hub) {
    holding(hub, [150.0, -150.0], Point { x: 477.0, y: 386.0 });
}

/// The terminal carried by its bar to the top edge of chat: the top of chat's slot shows
/// where it would go.
fn drop_edge(hub: &mut Hub) {
    holding(hub, [503.0, -476.0], Point { x: 980.0, y: 60.0 });
}

const fn scene(name: &'static str, setup: fn(&mut Hub)) -> Scene {
    Scene {
        name,
        stage: Stage::View(grid),
        setup,
    }
}

pub(super) const SCENES: &[Scene] = &[
    scene("grid/everything", |hub| show(hub, "everything")),
    scene("grid/review", |hub| show(hub, "review")),
    scene("grid/screens", |hub| show(hub, "screens")),
    scene("grid/code", |hub| show(hub, "code")),
    scene("grid/just-chat", |hub| show(hub, "just-chat")),
    scene("grid/chat-left", chat_left),
    scene("grid/dragging", dragging),
    scene("grid/drop-edge", drop_edge),
];
