//! A window's panes as the Panes panel lists them: each once, under the place of its slot.

use super::windows::Windows;
use crate::ids::{PaneId, WindowId};
use crate::pane::{PaneKind, Place};

/// Where a listed pane stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Drawn: alone in its slot, or the tab in front.
    Front,
    /// A tab behind the pane its slot draws.
    Behind,
    /// Pulled out into a window of its own.
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listed {
    pub pane: PaneId,
    pub kind: PaneKind,
    pub place: Place,
    pub standing: Standing,
}

impl Windows {
    /// The panes `window` shows in the session it is on: slot by slot as laid out and tab by
    /// tab, then the ones pulled out of it. A pane resting with nothing to show is not one.
    #[must_use]
    pub fn listed(&self, window: WindowId) -> Vec<Listed> {
        let Some(layout) = self.window(window).map(|found| &found.layout) else {
            return Vec::new();
        };
        let entry = |pane: PaneId, place: Option<Place>, standing| {
            Some(Listed {
                pane,
                kind: self.kind(pane)?,
                place: place?,
                standing,
            })
        };
        let mut list = Vec::new();
        for (held, _) in layout.docked() {
            let place = self.slot_place(held);
            // A pane drawn is listed though its session is not the one shown: it is there.
            let mut tabs = self.tabs(window, held);
            if !tabs.contains(&held) {
                tabs.insert(0, held);
            }
            for pane in tabs {
                let standing = match pane == held {
                    true => Standing::Front,
                    false => Standing::Behind,
                };
                list.extend(entry(pane, place, standing));
            }
        }
        let out = layout.away.iter().filter(|hole| !hole.resting);
        for pane in out.map(|hole| hole.pane) {
            let own = self.panes.iter().find(|other| other.id == pane);
            let place = own.map(|own| self.place_of(own));
            list.extend(entry(pane, place, Standing::Out));
        }
        list
    }

    /// The kinds with something to show in `window` that it lists no pane of, the chat
    /// aside. None before the window is first told what its session has.
    #[must_use]
    pub fn unopened(&self, window: WindowId) -> Vec<PaneKind> {
        let listed = self.listed(window);
        let open = |kind: &PaneKind| listed.iter().any(|entry| entry.kind == *kind);
        let have = self.have.get(&window).into_iter().flatten().copied();
        have.filter(|kind| *kind != PaneKind::Chat && !open(kind))
            .collect()
    }
}
