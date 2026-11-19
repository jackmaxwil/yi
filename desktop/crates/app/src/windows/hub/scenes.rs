//! Scenes of the app with panes out in windows of their own.

use yd_core::grid::step::GridMsg;
use yd_core::grid::windows::Frame;
use yd_core::ids::WindowId;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_ui::pane::{Scene, Stage};
use yd_ui::win::Hub;

use crate::root::MAIN;
use crate::shell;

/// The mockup's desktop of 1200 by 750 (frame 2): the app at the left under the menu bar,
/// the windows of the panes that left at its right.
pub const HOME: Frame = frame(4.0, 28.0, 862.0, 718.0);
pub const WEB: Frame = frame(884.0, 37.0, 312.0, 296.0);
pub const REMOTE: Frame = frame(884.0, 343.0, 312.0, 250.0);

const fn frame(x: f32, y: f32, w: f32, h: f32) -> Frame {
    Frame { x, y, w, h }
}

pub const SCENES: &[Scene] = &[
    Scene {
        name: "windows/two",
        stage: Stage::App,
        setup: two,
    },
    Scene {
        name: "windows/three",
        stage: Stage::App,
        setup: three,
    },
];

pub fn moved(hub: &mut Hub, window: WindowId, frame: Frame) {
    step(&mut hub.model, Msg::Grid(GridMsg::Moved { window, frame }));
}

/// Pulls the main window's pane of `kind` out, to a window of its own at `frame`.
fn out(hub: &mut Hub, kind: PaneKind, frame: Frame) {
    let pane = {
        let grid = &hub.model.grid;
        let docked = grid.window(MAIN).map(|window| window.layout.docked());
        let mut panes = docked.into_iter().flatten().map(|(pane, _)| pane);
        panes.find(|pane| grid.kind(*pane) == Some(kind))
    };
    let Some(pane) = pane else { return };
    step(&mut hub.model, Msg::Grid(GridMsg::Leave(pane)));
    if let Some(window) = hub.model.grid.window_of(pane) {
        moved(hub, window, frame);
    }
}

/// The whole app in one window at its place on the mockup's desktop, every pane docked.
pub fn docked(hub: &mut Hub) {
    let mut whole = shell::view::SCENES.iter();
    if let Some(scene) = whole.find(|scene| scene.name == "shell/everything") {
        (scene.setup)(hub);
    }
    moved(hub, MAIN, HOME);
}

/// One pane out: the web app in its own window, the grid closed up behind it.
pub fn two(hub: &mut Hub) {
    docked(hub);
    out(hub, PaneKind::Web, WEB);
}

/// The mockup's frame 2: the web app and the remote desktop each in a window.
pub fn three(hub: &mut Hub) {
    two(hub);
    out(hub, PaneKind::Remote, REMOTE);
}
