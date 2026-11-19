//! Where each window was, remembered across runs.

use std::path::PathBuf;

use rune::prefs::{Prefs, PrefsError};
use rune::{Display, OverlayPlacement};
use yd_core::grid::windows::{Frame, Windows};
use yd_core::ids::WindowId;

/// The folder under the Mac's application support that holds this app's own file.
const APP: &str = "yi-desktop";
/// The one value kept: the whole grid, each window with the frame it was last seen at.
const KEY: &str = "windows";
const SHOWN: &str = "shown";
const CHILDREN: &str = "children";
const SIDEBAR: &str = "sidebar";
const APART: &str = "apart";

/// The main window's sidebar hidden, the thread it showed, and each thread's own checkout.
pub type MainShown = (bool, Option<String>, Vec<(String, String)>);

/// Where each window of its own a pane kind opens in (Settings, Themes) was last: its slug.
pub type KeptApart = Vec<(String, Frame)>;

/// Each chat pane that shows a child agent: the pane, the thread and the child.
pub type KeptChildren = Vec<(u32, String, String)>;

/// One screen of the desktop as it is now: its bounds in points and its pixels a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub bounds: Frame,
    pub scale: f32,
    pub main: bool,
}

/// The desktop's screens. Rune gives each in pixels that start at its point origin times
/// its own scale, so points are one space across screens of any scale.
pub fn screens(displays: &[Display]) -> Vec<Screen> {
    let screen = |display: &Display| {
        let scale = if display.scale > 0.0 {
            display.scale
        } else {
            1.0
        };
        // Desktop pixels and sizes are far inside what an `f32` holds whole.
        let points = |pixels: f64| (pixels / scale) as f32;
        Screen {
            bounds: Frame {
                x: points(f64::from(display.position.0)),
                y: points(f64::from(display.position.1)),
                w: points(f64::from(display.size.0)),
                h: points(f64::from(display.size.1)),
            },
            scale: scale as f32,
            main: display.primary,
        }
    };
    displays.iter().map(screen).collect()
}

/// The screen that holds the middle of `frame`.
fn holding<'a>(frame: &Frame, screens: &'a [Screen]) -> Option<&'a Screen> {
    let (x, y) = (frame.x + frame.w / 2.0, frame.y + frame.h / 2.0);
    let holds = |screen: &&Screen| {
        let on = screen.bounds;
        x >= on.x && x < on.x + on.w && y >= on.y && y < on.y + on.h
    };
    screens.iter().find(holds)
}

fn main_of(screens: &[Screen]) -> Option<&Screen> {
    screens
        .iter()
        .find(|screen| screen.main)
        .or(screens.first())
}

/// Where a remembered window opens: where it was while a screen still holds its middle,
/// else in the middle of the main screen, no larger than that screen.
pub fn settle(frame: Frame, screens: &[Screen]) -> Frame {
    if holding(&frame, screens).is_some() {
        return frame;
    }
    let Some(main) = main_of(screens) else {
        return frame;
    };
    let on = main.bounds;
    let (w, h) = (frame.w.min(on.w), frame.h.min(on.h));
    Frame {
        x: on.x + (on.w - w) / 2.0,
        y: on.y + (on.h - h) / 2.0,
        w,
        h,
    }
}

/// A new window's frame moved, not resized, until the screen that holds its middle (else the
/// main one) holds all of it, or its top left where it is larger than the screen.
pub fn inside(frame: Frame, screens: &[Screen]) -> Frame {
    let Some(on) = holding(&frame, screens).or_else(|| main_of(screens)) else {
        return frame;
    };
    let on = on.bounds;
    let along = |at: f32, long: f32, from: f32, room: f32| at.min(from + room - long).max(from);
    Frame {
        x: along(frame.x, frame.w, on.x, on.w),
        y: along(frame.y, frame.h, on.y, on.h),
        ..frame
    }
}

/// A frame as Rune's `place` takes it: a rectangle in points from the main screen's top left.
pub fn from_main(frame: Frame, screens: &[Screen]) -> OverlayPlacement {
    let main = main_of(screens).map_or((0.0, 0.0), |main| (main.bounds.x, main.bounds.y));
    OverlayPlacement::Rect {
        x: frame.x - main.0,
        y: frame.y - main.1,
        width: frame.w,
        height: frame.h,
    }
}

/// A frame as Rune places a window: its top left and its size in desktop pixels, by the
/// scale of the screen that holds it.
pub fn pixels(frame: Frame, screens: &[Screen]) -> ((i32, i32), (u32, u32)) {
    let on = holding(&frame, screens).or_else(|| main_of(screens));
    let scale = on.map_or(1.0, |screen| screen.scale);
    // A float cast saturates, and a desktop is far inside `i32`.
    let at = |points: f32| (points * scale).round() as i32;
    let long = |points: f32| (points * scale).round().max(1.0) as u32;
    ((at(frame.x), at(frame.y)), (long(frame.w), long(frame.h)))
}

/// A window's place as Rune reports it, in points.
pub fn points(position: (i32, i32), size: (u32, u32), scale: f64) -> Frame {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let points = |pixels: f64| (pixels / scale) as f32;
    Frame {
        x: points(f64::from(position.0)),
        y: points(f64::from(position.1)),
        w: points(f64::from(size.0)),
        h: points(f64::from(size.1)),
    }
}

/// The file the grid is kept in between runs.
#[derive(Debug)]
pub struct Places(Prefs);

impl Places {
    /// The app's own file. A second run reads what the first one kept.
    pub fn open() -> Self {
        Self(Prefs::open(APP))
    }

    pub fn at(file: impl Into<PathBuf>) -> Self {
        Self(Prefs::at(file))
    }

    /// Kept for this run only: what a headless run and a scene use.
    pub fn memory() -> Self {
        Self(Prefs::memory())
    }

    /// The grid as it was kept, every window settled on a screen there is now. None when
    /// nothing was kept, the file's grid does not hold together, or it lacks `main`.
    pub fn read(&self, main: WindowId, screens: &[Screen]) -> Option<Windows> {
        let mut grid: Windows = self.0.get(KEY)?;
        grid.window(main)?;
        for window in &mut grid.windows {
            window.frame = window.frame.map(|frame| settle(frame, screens));
        }
        Some(grid)
    }

    /// Keeps the grid, written whole through a rename: a crash leaves the old file or the new.
    pub fn keep(&mut self, grid: &Windows) -> Result<(), PrefsError> {
        self.0.set(KEY, grid)
    }

    /// What the main window showed when it was last kept.
    pub fn read_shown(&self) -> Option<MainShown> {
        self.0.get(SHOWN)
    }

    /// The main window's sidebar width as a drag of its edge last left it; none: the default.
    pub fn read_width(&self) -> Option<f32> {
        self.0.get::<Option<f32>>(SIDEBAR).flatten()
    }

    pub fn keep_shown(&mut self, shown: &MainShown, width: Option<f32>) -> Result<(), PrefsError> {
        self.0.set(SHOWN, shown)?;
        self.0.set(SIDEBAR, &width)
    }

    /// The chat panes that showed a child agent when last kept.
    pub fn read_children(&self) -> Option<KeptChildren> {
        self.0.get(CHILDREN)
    }

    pub fn keep_children(&mut self, children: &KeptChildren) -> Result<(), PrefsError> {
        self.0.set(CHILDREN, children)
    }

    /// Where Settings and Themes were last, each settled on a screen there is now.
    pub fn read_apart(&self, screens: &[Screen]) -> KeptApart {
        let kept: KeptApart = self.0.get(APART).unwrap_or_default();
        let settled = |(slug, frame): (String, Frame)| (slug, settle(frame, screens));
        kept.into_iter().map(settled).collect()
    }

    pub fn keep_apart(&mut self, apart: &KeptApart) -> Result<(), PrefsError> {
        self.0.set(APART, apart)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::grid::step::GridMsg;
    use yd_core::model::Model;
    use yd_core::msg::{Msg, step};

    use super::*;

    const MAIN: WindowId = WindowId(0);

    fn display(position: (i32, i32), size: (u32, u32), scale: f64, primary: bool) -> Display {
        Display {
            id: None,
            name: None,
            position,
            size,
            scale,
            primary,
        }
    }

    /// A laptop screen of 1512 by 982 points at 2x, and a second 1920 by 1080 at 1x on its right.
    fn two() -> Vec<Screen> {
        screens(&[
            display((0, 0), (3024, 1964), 2.0, true),
            display((1512, 0), (1920, 1080), 1.0, false),
        ])
    }

    fn frame(x: f32, y: f32) -> Frame {
        Frame {
            x,
            y,
            w: 424.0,
            h: 292.0,
        }
    }

    #[test]
    fn a_window_on_the_second_screen_opens_there_again() {
        let there = frame(2218.0, 40.0);
        assert_eq!(settle(there, &two()), there);
        assert_eq!(pixels(there, &two()), ((2218, 40), (424, 292)));
        let laptop = frame(36.0, 40.0);
        assert_eq!(pixels(laptop, &two()), ((72, 80), (848, 584)));
    }

    #[test]
    fn a_window_whose_screen_is_gone_opens_on_the_main_screen() {
        let laptop = &two()[..1];
        let back = settle(frame(2218.0, 40.0), laptop);
        assert_eq!(back, frame(544.0, 345.0), "the same size, in the middle");
        let huge = Frame {
            w: 1900.0,
            h: 1060.0,
            ..frame(1520.0, 10.0)
        };
        let fitted = settle(huge, laptop);
        assert_eq!((fitted.w, fitted.h), (1512.0, 982.0), "no larger than it");
        assert_eq!((fitted.x, fitted.y), (0.0, 0.0));
    }

    #[test]
    fn a_new_window_past_the_edge_of_its_screen_is_moved_onto_it() {
        let above = inside(frame(662.0, -14.0), &two());
        assert_eq!(above, frame(662.0, 0.0), "its bar stays in reach");
        let low = inside(frame(1000.0, 800.0), &two());
        assert_eq!(low, frame(1000.0, 690.0), "all of it shows");
        let second = inside(frame(3100.0, 900.0), &two());
        assert_eq!(second, frame(3008.0, 788.0), "on the screen that holds it");
    }

    #[test]
    fn what_rune_reports_reads_back_as_the_points_it_was_placed_at() {
        let there = frame(2218.0, 40.0);
        let (position, size) = pixels(there, &two());
        assert_eq!(points(position, size, 1.0), there);
        let (position, size) = pixels(frame(36.0, 40.0), &two());
        assert_eq!(points(position, size, 2.0), frame(36.0, 40.0));
    }

    /// The grid of a run with one pane out on the second screen.
    fn kept() -> Result<(Windows, WindowId), Box<dyn Error>> {
        let mut model = Model::default();
        let show = GridMsg::Show {
            window: MAIN,
            saved: "everything",
        };
        step(&mut model, Msg::Grid(show));
        let pane = model.grid.panes.first().ok_or("no pane")?.id;
        step(&mut model, Msg::Grid(GridMsg::Leave(pane)));
        let own = model.grid.window_of(pane).ok_or("the pane is nowhere")?;
        let moved = GridMsg::Moved {
            window: own,
            frame: frame(2218.0, 40.0),
        };
        step(&mut model, Msg::Grid(moved));
        Ok((model.grid, own))
    }

    #[test]
    fn the_grid_kept_by_one_run_is_read_by_the_next() -> Result<(), Box<dyn Error>> {
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/wn")
            .join(format!("places-{}.json", std::process::id()));
        let (grid, own) = kept()?;
        Places::at(&file).keep(&grid)?;
        let next = Places::at(&file);
        assert_eq!(next.read(MAIN, &two()), Some(grid.clone()));
        let one = next.read(MAIN, &two()[..1]).ok_or("nothing was read")?;
        let place = one.window(own).and_then(|window| window.frame);
        assert_eq!(place, Some(frame(544.0, 345.0)), "its screen is gone");
        assert_eq!(next.read(WindowId(9), &two()), None, "no main window in it");
        std::fs::remove_file(&file)?;
        assert_eq!(Places::at(&file).read(MAIN, &two()), None);
        Ok(())
    }
}
