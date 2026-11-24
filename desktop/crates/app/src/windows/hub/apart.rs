//! Settings and Themes, each in a window of its own, and what every window apart from the main
//! one shares: its least size and ⌘`.

use std::io::Write;

use yd_core::grid::layout::{MIN_PANE, MIN_WINDOW};
use yd_core::grid::step::GridMsg;
use yd_core::grid::windows::Frame;
use yd_core::ids::WindowId;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_ui::win::{Shared, Win, dispatch};

use super::{Registry, State, with};
use crate::root::MAIN;

/// Panes shown in a window of their own, 760 by 560: beside eight panes they were too narrow
/// to press.
pub const APART: [PaneKind; 2] = [PaneKind::Settings, PaneKind::Themes];
pub(super) const APART_SIZE: (f32, f32) = (760.0, 560.0);
/// Where that window opens when the window it comes from has no place to read.
pub(super) const APART_AT: (f32, f32) = (80.0, 80.0);

/// Shows a pane of `kind` in a window of its own, 760 by 560: the one that is out already
/// comes forward, else the pane is pulled out of the grid, made first in the main window.
pub fn apart(win: &mut Win, kind: PaneKind) -> bool {
    let held = win.hub.try_borrow().ok().and_then(|hub| {
        let grid = &hub.model.grid;
        let held = grid.panes.iter().filter(|pane| pane.kind == kind);
        let mut held = held.filter_map(|pane| Some((pane.id, grid.window_of(pane.id)?)));
        held.next()
            .map(|(pane, window)| (pane, window, grid.origin_of(pane).is_some()))
    });
    let pane = match held {
        Some((_, window, true)) => return bring(&win.hub, window),
        Some((pane, _, false)) => pane,
        // Incident: made in the pane's window ⌘, was pressed in, Settings split that window and
        // its closing left a stray window holding Settings' place.
        None => {
            dispatch(win, Msg::Grid(GridMsg::Open { window: MAIN, kind }));
            let made = win.hub.try_borrow().ok().and_then(|hub| {
                let grid = &hub.model.grid;
                let mut docked = grid.window(MAIN)?.layout.docked();
                docked.find_map(|(pane, _)| (grid.kind(pane) == Some(kind)).then_some(pane))
            });
            match made {
                Some(pane) => pane,
                None => return false,
            }
        }
    };
    dispatch(win, Msg::Grid(GridMsg::Leave(pane)));
    true
}

/// Brings window `id` in front with the keys.
fn bring(shared: &Shared, id: WindowId) -> bool {
    let brought = with(shared, |registry, _| {
        let open = registry.find_mut(id)?;
        open.app.show();
        open.app.focus();
        open.state = State::Up;
        Some(())
    });
    brought.flatten().is_some()
}

/// The least a pane's window may be resized to: its pane's minimum at the zoom it is laid
/// out in, and the app window's minimum for Settings and Themes, whose rows need the width.
pub(super) fn least(kind: Option<PaneKind>, zoom: f32) -> (f32, f32) {
    match kind {
        Some(kind) if APART.contains(&kind) => (MIN_WINDOW.w, MIN_WINDOW.h),
        _ => (MIN_PANE.w * zoom, MIN_PANE.h * zoom),
    }
}

/// Brings forward the window after the one whose turn this is, as ⌘` does.
pub fn cycle(win: &Win) -> bool {
    let id = win.window;
    let brought = with(&win.hub, |registry, _| {
        let mut up = registry.open.iter().filter(|open| open.state == State::Up);
        let mut after = up.clone().skip_while(|open| open.id != id).skip(1);
        let next = after.next().or_else(|| up.next())?;
        (next.id != id).then(|| next.app.focus())
    });
    brought.flatten().is_some()
}

impl Registry {
    /// Keeps where Settings or Themes is, for the next time it opens.
    pub(super) fn remember(&mut self, kind: PaneKind, frame: Frame) {
        let slug = kind.slug().to_owned();
        self.apart.retain(|(kept, _)| *kept != slug);
        self.apart.push((slug, frame));
        let kept = self.places.as_mut().map(|p| p.keep_apart(&self.apart));
        if let Some(Err(error)) = kept {
            let _ = writeln!(
                std::io::stderr(),
                "yi-desktop: where {} was is not kept: {error}",
                kind.title()
            );
        }
    }
}
