//! Random runs of grid changes from fixed seeds: whatever the order, the panes tile each
//! window exactly, keep the minimum, and a pane that leaves and returns changes nothing.

use crate::address::uri::Address;
use crate::command::table::{FLIP_CHAT, Route, Run};
use crate::grid::layout::{Axis, Edge, Line, MAX_PANES, MAX_TRACKS, MIN_PANE, Point, Rect, Size};
use crate::grid::leave::split;
use crate::grid::place::probe::{Along, Check, FRAME_1, across, down, edges, roomy, tiled};
use crate::grid::saved::SAVED;
use crate::grid::swap::Spot;
use crate::grid::windows::{Frame, Pane, Window, Windows};
use crate::ids::{PaneId, ThreadId, WindowId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::pane::{PaneKind, Place};
use crate::settings::schema::SettingValue;
use crate::settings::store::OPENS_ROW;

use super::GridMsg;

const MAIN: WindowId = WindowId(0);
const RUNS: u64 = 300;
const CHANGES: usize = 80;
const WORDS: [&str; 3] = [
    "changes never\nagents never",
    "logs work\nchanges shelf\nweb side",
    "agents chat\nfiles work\nterminal side",
];

/// Grid sizes a window meets: the record's two frames, the smallest window's grid, a large
/// display, and one too small for the minimums.
const SIZES: [Size; 5] = [
    FRAME_1,
    Size { w: 704.0, h: 676.0 },
    Size { w: 562.0, h: 438.0 },
    Size {
        w: 2380.0,
        h: 1290.0,
    },
    Size { w: 300.0, h: 200.0 },
];

/// Xorshift64*: a fixed seed gives the same run on every machine.
struct Dice(u64);

impl Dice {
    fn roll(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, count: usize) -> usize {
        let count = u64::try_from(count.max(1)).unwrap_or(1);
        usize::try_from(self.roll() % count).unwrap_or(0)
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> Option<T> {
        from.get(self.below(from.len())).copied()
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        let step = u16::try_from(self.below(10_001)).unwrap_or(0);
        low + (high - low) * f32::from(step) / 10_000.0
    }
}

/// One thing done to the grid: a message through `step`, or a new pane cut in beside
/// another on any edge, which no message asks for yet but the split behind them allows.
#[derive(Debug, Clone)]
enum Change {
    Said(Msg),
    Cut(WindowId, PaneId, Edge),
}

/// A line of `window` to drag: mostly one a pane really ends on, sometimes any at all.
fn any_line(dice: &mut Dice, grid: &Windows, window: WindowId, size: Size) -> Option<Line> {
    let axis = dice.pick(&[Axis::Cols, Axis::Rows])?;
    let (_, rect) = dice.pick(&grid.place(window, size))?;
    let (x, y) = match axis {
        Axis::Cols => (rect.x + rect.w, rect.y + rect.h / 2.0),
        Axis::Rows => (rect.x + rect.w / 2.0, rect.y + rect.h),
    };
    let end = Point { x, y };
    let after = u16::try_from(dice.below(6)).ok()?;
    let real = grid
        .line_at(window, size, end)
        .filter(|_| dice.below(5) > 0);
    Some(real.unwrap_or(Line { axis, after }))
}

/// One change a user could make to `grid`, aimed at panes and lines that are there.
fn any_change(dice: &mut Dice, grid: &Windows, size: Size) -> Option<Change> {
    let windows: Vec<WindowId> = grid.windows.iter().map(|window| window.id).collect();
    let window = dice.pick(&windows).unwrap_or(MAIN);
    let placed = grid.place(window, size);
    let drawn: Vec<PaneId> = placed.iter().map(|(pane, _)| *pane).collect();
    let all: Vec<PaneId> = grid.panes.iter().map(|pane| pane.id).collect();
    let holes = grid.windows.iter().flat_map(|window| &window.layout.away);
    let out: Vec<PaneId> = holes.map(|hole| hole.pane).collect();
    let change = match dice.below(34) {
        0 | 1 => {
            let saved = dice.pick(&SAVED)?.slug;
            let window = if dice.below(4) > 0 { MAIN } else { window };
            GridMsg::Show { window, saved }
        }
        2..=4 => {
            let line = any_line(dice, grid, window, size)?;
            let to = dice.between(-50.0, size.w + 50.0);
            GridMsg::Resize {
                window,
                line,
                to,
                size,
            }
        }
        5 => {
            let line = any_line(dice, grid, window, size)?;
            GridMsg::Even { window, line, size }
        }
        6 | 7 => {
            let (a, b) = (dice.pick(&drawn)?, dice.pick(&drawn)?);
            GridMsg::Swap { window, a, b }
        }
        8..=10 => GridMsg::Leave(dice.pick(&drawn)?),
        11..=13 => GridMsg::Rejoin(dice.pick(&out)?),
        14 => GridMsg::Split(dice.pick(&drawn)?),
        15 => {
            let edges = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];
            let (pane, edge) = (dice.pick(&all)?, dice.pick(&edges)?);
            GridMsg::SplitAt { pane, edge }
        }
        16 => GridMsg::Close(dice.pick(&all)?),
        17 => {
            return Some(Change::Said(Msg::Command(
                Route::Model,
                Run::new(FLIP_CHAT),
            )));
        }
        18 => {
            let (x, y) = (dice.between(-200.0, 3000.0), dice.between(-50.0, 1500.0));
            let (w, h) = (size.w, size.h);
            let frame = Frame { x, y, w, h };
            GridMsg::Moved { window, frame }
        }
        19 | 20 => {
            let edge = dice.pick(&[Edge::Top, Edge::Bottom, Edge::Left, Edge::Right])?;
            return Some(Change::Cut(window, dice.pick(&drawn)?, edge));
        }
        21..=23 => {
            let edge = dice.pick(&[Edge::Top, Edge::Bottom, Edge::Left, Edge::Right])?;
            let (pane, other) = (dice.pick(&drawn)?, dice.pick(&drawn)?);
            let at = match dice.below(3) {
                0 => Spot::Onto(other),
                1 => Spot::Beside(other, edge),
                _ => Spot::Side(edge),
            };
            GridMsg::Drop {
                window,
                pane,
                at,
                size,
            }
        }
        24 | 25 => {
            let kinds = PaneKind::ALL.iter().copied();
            let kinds = kinds.filter(|_| dice.below(2) > 0).collect();
            let thread = dice.pick(&["", "a", "b", "c"])?;
            let thread = (!thread.is_empty()).then(|| ThreadId::new(thread));
            GridMsg::Have {
                window,
                kinds,
                thread,
            }
        }
        26 => {
            let kind = dice.pick(PaneKind::ALL)?;
            GridMsg::Open { window, kind }
        }
        27 => {
            let (pane, on) = (dice.pick(&drawn)?, dice.below(2) > 0);
            GridMsg::Across { pane, on }
        }
        28 => GridMsg::Front(dice.pick(&all)?),
        29 => {
            let (pane, place) = (dice.pick(&all)?, dice.pick(&Place::ALL)?);
            GridMsg::Place { pane, place }
        }
        30 | 31 => {
            let name = dice.pick(&["a/one", "a/two", "b/one"])?;
            let thing = Address::new(Kind::Agent, name).ok()?;
            let kind = PaneKind::Chat;
            GridMsg::About {
                window,
                kind,
                thing,
            }
        }
        32 => GridMsg::Fill(dice.pick(&all)?),
        _ => GridMsg::Leave(dice.pick(&all)?),
    };
    Some(Change::Said(Msg::Grid(change)))
}

fn make(model: &mut Model, change: &Change) {
    match change {
        Change::Said(msg) => drop(step(model, msg.clone())),
        Change::Cut(window, beside, edge) => {
            let grid = &mut model.grid;
            let highest = grid.panes.iter().map(|pane| pane.id.0).max().unwrap_or(0);
            let (id, kind) = (PaneId(highest + 1), grid.kind(*beside));
            let window = grid.windows.iter_mut().find(|other| other.id == *window);
            let (Some(window), Some(kind)) = (window, kind) else {
                return;
            };
            if split(&mut window.layout, *beside, *edge, id).is_ok() {
                window.saved = None;
                let (seated, scope) = (false, Default::default());
                grid.panes.push(Pane {
                    id,
                    kind,
                    seated,
                    scope,
                    thing: None,
                    at: None,
                    unseen: false,
                });
            }
        }
    }
}

/// What must hold after every change: the grid is sound, and each window's panes tile a
/// grid of `size` exactly and keep the minimum where the tracks have room for it.
fn whole(grid: &Windows, size: Size) -> Result<(), String> {
    grid.check().map_err(|error| error.to_string())?;
    for window in &grid.windows {
        let placed = grid.place(window.id, size);
        let docked: Vec<PaneId> = window.layout.docked().map(|(pane, _)| pane).collect();
        let shown: Vec<PaneId> = placed.iter().map(|(pane, _)| *pane).collect();
        if shown != docked {
            return Err(format!("{:?} draws {shown:?}, holds {docked:?}", window.id));
        }
        tiled(&placed, size)?;
        roomy(&placed, size)?;
    }
    Ok(())
}

/// How many lines along one axis are not where they were.
fn moved(before: &[(PaneId, Rect)], after: &[(PaneId, Rect)], of: Along) -> usize {
    let (was, is) = (edges(before, of), edges(after, of));
    let apart = |(a, b): (&f32, &f32)| (a - b).abs() > 0.02;
    was.iter().zip(&is).filter(|pair| apart(*pair)).count() + was.len().abs_diff(is.len())
}

/// A drag moves the line it names along its own axis and no other line on either.
fn one_line(before: &Windows, after: &Windows, change: &Change, size: Size) -> Result<(), String> {
    let Change::Said(Msg::Grid(GridMsg::Resize { window, line, .. })) = change else {
        return Ok(());
    };
    let (was, is) = (before.place(*window, size), after.place(*window, size));
    let (cols, rows) = (moved(&was, &is, across), moved(&was, &is, down));
    let (dragged, other) = match line.axis {
        Axis::Cols => (cols, rows),
        Axis::Rows => (rows, cols),
    };
    if dragged > 1 || other > 0 {
        return Err(format!("{cols} column and {rows} row lines moved"));
    }
    Ok(())
}

/// A change the grid has no reason to refuse must take: a pane leaves or closes whenever
/// another stays in its window, and a pane that is out comes back to a slot kept for it.
fn took(before: &Windows, change: &Change, after: &Windows) -> Result<(), String> {
    let Change::Said(Msg::Grid(change)) = change else {
        return Ok(());
    };
    let drawn_in = |pane: PaneId| before.window_of(pane).and_then(|id| before.window(id));
    let others = |pane: PaneId| drawn_in(pane).is_some_and(|w| w.layout.docked().nth(1).is_some());
    let refused = match change {
        GridMsg::Leave(pane) => others(*pane) && after.window_of(*pane) == before.window_of(*pane),
        GridMsg::Close(pane) => others(*pane) && after.window_of(*pane) == before.window_of(*pane),
        GridMsg::Rejoin(pane) => {
            let home = before.origin_of(*pane).and_then(|id| before.window(id));
            let kept = home.is_some_and(|window| window.layout.slot(*pane).is_some());
            let free = others(*pane) || drawn_in(*pane).is_some_and(|w| w.layout.away.is_empty());
            kept && free && after.origin_of(*pane).is_some()
        }
        GridMsg::Split(pane) => {
            let full =
                |w: &Window| w.layout.cols.len() >= MAX_TRACKS || w.layout.areas.len() >= MAX_PANES;
            drawn_in(*pane).is_some_and(|w| !full(w)) && after.panes.len() == before.panes.len()
        }
        GridMsg::Show { .. }
        | GridMsg::Resize { .. }
        | GridMsg::Even { .. }
        | GridMsg::Swap { .. }
        | GridMsg::Drop { .. }
        | GridMsg::Open { .. }
        | GridMsg::About { .. }
        | GridMsg::Have { .. }
        | GridMsg::SplitAt { .. }
        | GridMsg::Place { .. }
        | GridMsg::Across { .. }
        | GridMsg::Front(_)
        | GridMsg::Fill(_)
        | GridMsg::Moved { .. }
        | GridMsg::Keep { .. }
        | GridMsg::Wear { .. }
        | GridMsg::Rename { .. }
        | GridMsg::Forget(_) => false,
    };
    if refused {
        return Err("the grid refused it".to_owned());
    }
    Ok(())
}

/// The grid under any fill: what every property of the arrangement is checked on.
fn unfilled(grid: &Windows) -> Windows {
    let mut plain = grid.clone();
    plain.filled.clear();
    plain
}

/// A window with a pane filling it draws that pane alone, over the whole grid, and the pane
/// is one the window draws without the fill; a fill names a window that is there.
fn filled(grid: &Windows, size: Size) -> Result<(), String> {
    let plain = unfilled(grid);
    for window in grid.filled.keys() {
        let Some(pane) = grid.fills(*window) else {
            continue;
        };
        let (x, y, Size { w, h }) = (0.0, 0.0, size);
        if grid.place(*window, size) != [(pane, Rect { x, y, w, h })] {
            return Err(format!("{pane:?} does not fill {window:?}"));
        }
        if !plain
            .place(*window, size)
            .iter()
            .any(|(drawn, _)| *drawn == pane)
        {
            return Err(format!("{pane:?} fills {window:?} and is not drawn in it"));
        }
    }
    Ok(())
}

/// Runs seeded changes from Everything, handing `each` the grid before a change, the change
/// and the grid after it, with the grid size that turn drew at.
fn runs(
    salt: u64,
    mut each: impl FnMut(&Windows, &Change, &Windows, Size) -> Result<(), String>,
) -> Check {
    for seed in 1..=RUNS {
        let mut dice = Dice(seed.wrapping_mul(salt));
        let mut model = Model::default();
        let (window, saved) = (MAIN, "everything");
        step(&mut model, Msg::Grid(GridMsg::Show { window, saved }));
        // A third of the runs carry the person's words on what opens by itself, and where.
        if let Some(words) = WORDS.get(dice.below(3 * WORDS.len())) {
            let mut row = OPENS_ROW;
            row.value = Some(SettingValue::Text((*words).into()));
            model.settings.rows.push(row);
            // Read now, as the first grid change would: a refused change then changes nothing.
            model.grid.rules = model.settings.opens();
        }
        for turn in 0..CHANGES {
            let size = dice.pick(&SIZES).unwrap_or(FRAME_1);
            let Some(change) = any_change(&mut dice, &model.grid, size) else {
                continue;
            };
            let before = unfilled(&model.grid);
            make(&mut model, &change);
            let failed = |error| format!("seed {seed}, change {turn}, {change:?}: {error}");
            filled(&model.grid, size).map_err(failed)?;
            each(&before, &change, &unfilled(&model.grid), size).map_err(failed)?;
        }
    }
    Ok(())
}

/// The brief's first two properties over 300 seeded runs of 80 changes: an exact tiling and
/// the pane minimum after every change; and a drag moves one line and no other.
#[test]
fn any_run_of_changes_keeps_every_window_whole() -> Check {
    let mut changed = 0;
    runs(0x9E37_79B9_7F4A_7C15, |before, change, after, size| {
        changed += usize::from(before != after);
        whole(after, size)?;
        took(before, change, after)?;
        one_line(before, after, change, size)
    })?;
    assert!(changed > 10_000, "only {changed} changes took");
    Ok(())
}

/// The brief's third: leave followed by rejoin with nothing in between gives back the same
/// grid, from any state a run reaches, for every pane that is at home in its window.
#[test]
fn a_pane_that_leaves_and_comes_straight_back_changes_nothing() -> Check {
    let mut tried = 0;
    runs(0xD1B5_4A32_D192_ED03, |_, _, grid, _| {
        let at_home = |pane: &PaneId| grid.origin_of(*pane).is_none();
        for pane in grid.panes.iter().map(|pane| pane.id).filter(at_home) {
            let mut trial = grid.clone();
            if super::step(&mut trial, GridMsg::Leave(pane)).is_empty() {
                continue;
            }
            if trial == *grid {
                return Err(format!("{pane:?} left and nothing changed"));
            }
            super::step(&mut trial, GridMsg::Rejoin(pane));
            // A tab drawn while the pane was out has been seen: its dot does not come back.
            let mut grid = grid.clone();
            for pane in trial.panes.iter_mut().chain(grid.panes.iter_mut()) {
                pane.unseen = false;
            }
            // A neighbour a pane out remembers is a slot: any tab of it names the same one.
            for window in trial.windows.iter_mut().chain(grid.windows.iter_mut()) {
                let layout = window.layout.clone();
                let holes = window.layout.away.iter_mut();
                for (other, _) in holes.flat_map(|hole| hole.beside.iter_mut()) {
                    *other = layout.front(*other).unwrap_or(*other);
                }
            }
            let grid = &grid;
            if trial != *grid {
                return Err(format!("{pane:?} came back to {trial:?}, not {grid:?}"));
            }
            tried += 1;
        }
        Ok(())
    })?;
    assert!(tried > 10_000, "only {tried} panes left and came back");
    Ok(())
}

/// What every drop that takes must keep: the same panes, each in the window it was drawn in,
/// the dropped one still drawn there, and every pane of that window at the minimum or more.
fn dropped(before: &Windows, change: &Change, after: &Windows, size: Size) -> Result<(), String> {
    let Change::Said(Msg::Grid(GridMsg::Drop { window, pane, .. })) = change else {
        return Ok(());
    };
    if before == after {
        return Ok(());
    }
    let ids = |grid: &Windows| grid.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
    if ids(before) != ids(after) {
        return Err(format!("panes {:?} became {:?}", ids(before), ids(after)));
    }
    let homes = |grid: &Windows| {
        ids(grid)
            .into_iter()
            .map(|id| grid.window_of(id).or(grid.home_of(id)))
            .collect::<Vec<_>>()
    };
    if homes(before) != homes(after) || after.window_of(*pane) != Some(*window) {
        return Err(format!(
            "a pane changed windows: {:?} to {:?}",
            homes(before),
            homes(after)
        ));
    }
    let stack = |grid: &Windows| grid.window(*window).map(|w| w.layout.stack(*pane).len());
    let Change::Said(Msg::Grid(GridMsg::Drop { at, .. })) = change else {
        return Ok(());
    };
    if !matches!(at, Spot::Onto(_)) && stack(after) != Some(0) {
        return Err(format!("{pane:?} took its tabs along to an edge"));
    }
    let small =
        |(_, rect): &&(PaneId, Rect)| rect.w < MIN_PANE.w - 0.02 || rect.h < MIN_PANE.h - 0.02;
    match after.place(*window, size).iter().find(small) {
        Some(cramped) => Err(format!("{cramped:?} is under the minimum after a drop")),
        None => Ok(()),
    }
}

/// Drops at every kind of spot over 300 seeded runs: each keeps every pane, in its window,
/// over the minimum, and the grid whole; enough of each kind take to count.
#[test]
fn any_drop_keeps_every_pane_and_the_minimum() -> Check {
    let (mut took, mut refused) = (0, 0);
    runs(0xA076_1D64_78BD_642F, |before, change, after, size| {
        if matches!(change, Change::Said(Msg::Grid(GridMsg::Drop { .. }))) {
            if before == after {
                refused += 1
            } else {
                took += 1
            }
        }
        whole(after, size)?;
        dropped(before, change, after, size)
    })?;
    assert!(
        took > 500 && refused > 500,
        "{took} drops took, {refused} refused"
    );
    Ok(())
}
