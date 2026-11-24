//! A pane torn or pulled off its grid becomes a window just off the slot it left.

use yd_core::grid::layout::{MIN_WINDOW, Size};
use yd_core::grid::step::{GridMsg, step};
use yd_core::grid::windows::{Frame, Windows};
use yd_core::ids::PaneId;

/// Record 2.8.1: the window opens 18 right of and 14 above the pane's slot, 4 taller.
const RIGHT: f32 = 18.0;
const UP: f32 = 14.0;
const TALLER: f32 = 4.0;

/// Where a pane that has just left opens with no place remembered: off its slot in the window
/// at `home`, never under the window minimum (a Files column alone was 208 points wide).
pub fn lifted(grid: &Windows, pane: PaneId, home: Frame) -> Option<Frame> {
    let origin = grid.origin_of(pane)?;
    // The slot is where the grid would draw the pane had it not left: ask a copy.
    let mut before = grid.clone();
    step(&mut before, GridMsg::Rejoin(pane));
    let size = Size {
        w: home.w,
        h: home.h,
    };
    let placed = before.place(origin, size);
    let (_, slot) = placed.into_iter().find(|(id, _)| *id == pane)?;
    Some(Frame {
        x: home.x + slot.x + RIGHT,
        y: home.y + slot.y - UP,
        w: slot.w.max(MIN_WINDOW.w),
        h: (slot.h + TALLER).max(MIN_WINDOW.h),
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::ids::WindowId;
    use yd_core::model::Model;
    use yd_core::msg::{Msg, step as step_model};
    use yd_core::pane::PaneKind;

    use super::*;

    const MAIN: WindowId = WindowId(0);
    const HOME: Frame = Frame {
        x: 100.0,
        y: 60.0,
        w: 1200.0,
        h: 750.0,
    };

    #[test]
    fn a_narrow_pane_that_leaves_opens_just_off_its_slot_at_a_usable_size()
    -> Result<(), Box<dyn Error>> {
        let mut model = Model::default();
        let show = GridMsg::Show {
            window: MAIN,
            saved: "everything",
        };
        step_model(&mut model, Msg::Grid(show));
        let size = Size {
            w: HOME.w,
            h: HOME.h,
        };
        let terminal = |pane: &&yd_core::grid::windows::Pane| pane.kind == PaneKind::Files;
        let pane = model
            .grid
            .panes
            .iter()
            .find(terminal)
            .ok_or("no files pane")?;
        let pane = pane.id;
        let docked = model.grid.place(MAIN, size);
        let (_, slot) = docked.iter().find(|(id, _)| *id == pane).ok_or("no slot")?;
        assert_eq!(lifted(&model.grid, pane, HOME), None, "it has not left");
        step_model(&mut model, Msg::Grid(GridMsg::Leave(pane)));
        let opens = lifted(&model.grid, pane, HOME).ok_or("no place for it")?;
        assert!(slot.w < MIN_WINDOW.w, "the Files column is narrow");
        let usable = (slot.w.max(MIN_WINDOW.w), (slot.h + 4.0).max(MIN_WINDOW.h));
        assert_eq!((opens.w, opens.h), usable);
        assert_eq!(
            (opens.x, opens.y),
            (HOME.x + slot.x + 18.0, HOME.y + slot.y - 14.0)
        );
        Ok(())
    }
}
