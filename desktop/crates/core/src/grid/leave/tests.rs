//! A pane leaves, closes, splits and comes back, measured against the record's frames and
//! the plan's "no hole, ever".

use crate::grid::layout::Track::{Fr, Px};
use crate::grid::layout::{ChatSide, Edge, GridError, Layout, Rect, Size, area};
use crate::grid::place::place;
use crate::grid::place::probe::{Check, FRAME_1, find, open, pane, rect, tiled};
use crate::grid::saved::{CODE, EVERYTHING, JUST_CHAT, REVIEW, SAVED, SCREENS};
use crate::ids::PaneId;
use crate::pane::PaneKind::{self, Changes, Chat, Code, Files, Remote, Report, Terminal, Web};

use super::{close, leave, rejoin, split};

/// The layout's panes in a grid of `size`, after checking they tile it exactly.
fn drawn(layout: &Layout, size: Size) -> Result<Vec<(PaneId, Rect)>, String> {
    let placed = place(layout, size, ChatSide::Right);
    tiled(&placed, size)?;
    Ok(placed)
}

fn expect(placed: &[(PaneId, Rect)], want: &[(PaneKind, Rect)]) -> Check {
    assert_eq!(placed.len(), want.len(), "panes drawn: {placed:?}");
    for (kind, rect) in want {
        assert_eq!(find(placed, pane(*kind))?, *rect, "{kind:?}");
    }
    Ok(())
}

/// Record 2.7.4: where Everything puts each pane in the frame 1 grid.
const EVERYTHING_PLACED: [(PaneKind, Rect); 8] = [
    (
        Files,
        Rect {
            x: 0.0,
            y: 0.0,
            w: 208.0,
            h: 676.0,
        },
    ),
    (
        Changes,
        Rect {
            x: 208.0,
            y: 0.0,
            w: 270.0,
            h: 416.0,
        },
    ),
    (
        Terminal,
        Rect {
            x: 208.0,
            y: 416.0,
            w: 270.0,
            h: 260.0,
        },
    ),
    (
        Web,
        Rect {
            x: 478.0,
            y: 0.0,
            w: 286.0,
            h: 226.0,
        },
    ),
    (
        Remote,
        Rect {
            x: 478.0,
            y: 226.0,
            w: 286.0,
            h: 190.0,
        },
    ),
    (
        Report,
        Rect {
            x: 478.0,
            y: 416.0,
            w: 286.0,
            h: 260.0,
        },
    ),
    (
        Code,
        Rect {
            x: 764.0,
            y: 0.0,
            w: 270.0,
            h: 226.0,
        },
    ),
    (
        Chat,
        Rect {
            x: 764.0,
            y: 226.0,
            w: 270.0,
            h: 450.0,
        },
    ),
];

/// Record 2.8.2, frame 2: web and the remote desktop leave; Changes and Chat grow up into
/// their room. The record's 118 terminal is held at the plan's 120; Changes gives the two.
#[test]
fn two_panes_leave_and_the_grid_closes_up_as_frame_2() -> Check {
    let size = Size { w: 704.0, h: 676.0 };
    let mut layout = Layout {
        cols: vec![Px(184.0), Fr(1.0), Px(250.0)],
        rows: vec![Px(214.0), Fr(1.0), Px(118.0)],
        areas: vec![
            (pane(Files), area(1, 1, 4, 2)),
            (pane(Web), area(1, 2, 2, 3)),
            (pane(Changes), area(2, 2, 3, 3)),
            (pane(Terminal), area(3, 2, 4, 3)),
            (pane(Remote), area(1, 3, 2, 4)),
            (pane(Chat), area(2, 3, 4, 4)),
        ],
        away: Vec::new(),
        stacks: Vec::new(),
    };
    leave(&mut layout, pane(Web))?;
    leave(&mut layout, pane(Remote))?;
    let want = [
        (Files, rect(0.0, 0.0, 184.0, 676.0)),
        (Changes, rect(184.0, 0.0, 270.0, 556.0)),
        (Terminal, rect(184.0, 556.0, 270.0, 120.0)),
        (Chat, rect(454.0, 0.0, 250.0, 676.0)),
    ];
    expect(&drawn(&layout, size)?, &want)
}

/// Record gap 8, plan default 5: Files fills a column of Everything, so when it leaves the
/// column collapses and the flexible column takes its 208 points.
#[test]
fn a_pane_that_fills_a_column_leaves_and_the_column_collapses() -> Check {
    let mut layout = open(&EVERYTHING);
    leave(&mut layout, pane(Files))?;
    let want = [
        (Changes, rect(0.0, 0.0, 478.0, 416.0)),
        (Terminal, rect(0.0, 416.0, 478.0, 260.0)),
        (Web, rect(478.0, 0.0, 286.0, 226.0)),
        (Remote, rect(478.0, 226.0, 286.0, 190.0)),
        (Report, rect(478.0, 416.0, 286.0, 260.0)),
        (Code, rect(764.0, 0.0, 270.0, 226.0)),
        (Chat, rect(764.0, 226.0, 270.0, 450.0)),
    ];
    expect(&drawn(&layout, FRAME_1)?, &want)
}

/// Record 2.8.6, the laptop: the three stacked screens leave one by one; the last holds the
/// whole column, which collapses to Files, Changes over terminal, open file over chat.
#[test]
fn a_column_that_empties_pane_by_pane_collapses() -> Check {
    let mut layout = open(&EVERYTHING);
    for kind in [Web, Remote, Report] {
        leave(&mut layout, pane(kind))?;
        drawn(&layout, FRAME_1)?;
    }
    let want = [
        (Files, rect(0.0, 0.0, 208.0, 676.0)),
        (Changes, rect(208.0, 0.0, 556.0, 416.0)),
        (Terminal, rect(208.0, 416.0, 556.0, 260.0)),
        (Code, rect(764.0, 0.0, 270.0, 226.0)),
        (Chat, rect(764.0, 226.0, 270.0, 450.0)),
    ];
    expect(&drawn(&layout, FRAME_1)?, &want)
}

/// Plan default 5: a column that empties collapses, so its room goes to the flexible
/// column, not to the panes next to it: the three screens keep their 286 and move right.
#[test]
fn a_collapsed_column_gives_its_room_to_the_flexible_column() -> Check {
    let mut layout = open(&EVERYTHING);
    leave(&mut layout, pane(Code))?;
    leave(&mut layout, pane(Chat))?;
    let want = [
        (Files, rect(0.0, 0.0, 208.0, 676.0)),
        (Changes, rect(208.0, 0.0, 540.0, 416.0)),
        (Terminal, rect(208.0, 416.0, 540.0, 260.0)),
        (Web, rect(748.0, 0.0, 286.0, 226.0)),
        (Remote, rect(748.0, 226.0, 286.0, 190.0)),
        (Report, rect(748.0, 416.0, 286.0, 260.0)),
    ];
    expect(&drawn(&layout, FRAME_1)?, &want)
}

/// Plan default 5: the terminal of Code lies under two panes; both share its room.
#[test]
fn panes_sharing_an_edge_share_the_room() -> Check {
    let mut layout = open(&CODE);
    leave(&mut layout, pane(Terminal))?;
    let want = [
        (Files, rect(0.0, 0.0, 220.0, 676.0)),
        (Code, rect(220.0, 0.0, 407.0, 676.0)),
        (Changes, rect(627.0, 0.0, 407.0, 676.0)),
    ];
    expect(&drawn(&layout, FRAME_1)?, &want)
}

/// Record gap 9: a swap while a pane is out must not break its return. Web comes back to
/// its slot and the two swapped panes stay swapped.
#[test]
fn a_pane_comes_back_to_its_slot_after_a_swap() -> Check {
    let mut layout = open(&EVERYTHING);
    leave(&mut layout, pane(Web))?;
    crate::grid::swap::swap(&mut layout, pane(Code), pane(Chat))?;
    rejoin(&mut layout, pane(Web))?;
    let placed = drawn(&layout, FRAME_1)?;
    assert_eq!(find(&placed, pane(Web))?, rect(478.0, 0.0, 286.0, 226.0));
    assert_eq!(
        find(&placed, pane(Remote))?,
        rect(478.0, 226.0, 286.0, 190.0)
    );
    assert_eq!(find(&placed, pane(Chat))?, rect(764.0, 0.0, 270.0, 226.0));
    assert_eq!(find(&placed, pane(Code))?, rect(764.0, 226.0, 270.0, 450.0));
    Ok(())
}

/// Record gap 9: a second pane leaves beside the neighbour that already grew. Each comes
/// back without overlap, and with both back the grid is Everything again.
#[test]
fn a_pane_comes_back_after_its_neighbour_grew_twice() -> Check {
    let mut layout = open(&EVERYTHING);
    leave(&mut layout, pane(Web))?;
    leave(&mut layout, pane(Report))?;
    assert_eq!(
        find(&drawn(&layout, FRAME_1)?, pane(Remote))?,
        rect(478.0, 0.0, 286.0, 676.0),
        "the remote desktop holds the column"
    );
    rejoin(&mut layout, pane(Web))?;
    let placed = drawn(&layout, FRAME_1)?;
    assert_eq!(find(&placed, pane(Web))?, rect(478.0, 0.0, 286.0, 226.0));
    assert_eq!(
        find(&placed, pane(Remote))?,
        rect(478.0, 226.0, 286.0, 450.0)
    );
    rejoin(&mut layout, pane(Report))?;
    expect(&drawn(&layout, FRAME_1)?, &EVERYTHING_PLACED)
}

/// Leaving and coming straight back changes nothing, for every pane of every saved layout.
#[test]
fn leave_then_rejoin_gives_back_the_same_layout() -> Check {
    for saved in SAVED.iter().filter(|saved| saved.areas.len() > 1) {
        for (kind, _) in saved.areas {
            let mut layout = open(saved);
            let before = (layout.clone(), drawn(&layout, FRAME_1)?);
            leave(&mut layout, pane(*kind))?;
            let without = drawn(&layout, FRAME_1)?;
            assert_eq!(
                without.len() + 1,
                saved.areas.len(),
                "{} {kind:?}",
                saved.name
            );
            rejoin(&mut layout, pane(*kind))?;
            assert_eq!((layout.clone(), drawn(&layout, FRAME_1)?), before);
        }
    }
    Ok(())
}

/// A window is a grid of one or more panes: the only pane of Just chat stays.
#[test]
fn the_last_pane_cannot_leave() {
    let mut layout = open(&JUST_CHAT);
    let before = layout.clone();
    assert_eq!(
        leave(&mut layout, pane(Chat)),
        Err(GridError::LastPane(pane(Chat)))
    );
    assert_eq!(
        close(&mut layout, pane(Chat)),
        Err(GridError::LastPane(pane(Chat)))
    );
    assert_eq!(layout, before);
}

/// Plan default 4: closing the terminal of Review removes it and Changes fills its room.
#[test]
fn closing_a_pane_lets_its_neighbour_fill() -> Check {
    let mut layout = open(&REVIEW);
    close(&mut layout, pane(Terminal))?;
    assert_eq!(layout.slot(pane(Terminal)), None, "its slot is gone");
    let want = [
        (Files, rect(0.0, 0.0, 220.0, 676.0)),
        (Changes, rect(220.0, 0.0, 514.0, 676.0)),
        (Chat, rect(734.0, 0.0, 300.0, 676.0)),
    ];
    expect(&drawn(&layout, FRAME_1)?, &want)
}

/// Plan default 4: a split puts the new pane to the right, in half the old pane's room.
#[test]
fn a_split_puts_a_new_pane_beside_the_old_in_half_its_room() -> Check {
    let mut layout = open(&REVIEW);
    let copy = PaneId(90);
    split(&mut layout, pane(Changes), Edge::Right, copy)?;
    let placed = drawn(&layout, FRAME_1)?;
    assert_eq!(placed.len(), 5);
    assert_eq!(
        find(&placed, pane(Changes))?,
        rect(220.0, 0.0, 257.0, 526.0)
    );
    assert_eq!(find(&placed, copy)?, rect(477.0, 0.0, 257.0, 526.0));
    assert_eq!(
        find(&placed, pane(Terminal))?,
        rect(220.0, 526.0, 514.0, 150.0)
    );
    assert_eq!(
        split(&mut layout, pane(Files), Edge::Right, copy),
        Err(GridError::AlreadyIn(copy)),
        "a pane has one slot"
    );
    Ok(())
}

/// A pane that already crosses a line is cut at that line, so the two halves line up with
/// their neighbours: Changes of Everything, split below, ends where web does.
#[test]
fn a_split_cuts_at_a_line_the_pane_already_crosses() -> Check {
    let mut layout = open(&EVERYTHING);
    let copy = PaneId(90);
    split(&mut layout, pane(Changes), Edge::Bottom, copy)?;
    assert_eq!(layout.rows.len(), 3, "no row was made");
    let placed = drawn(&layout, FRAME_1)?;
    assert_eq!(placed.len(), 9);
    assert_eq!(
        find(&placed, pane(Changes))?,
        rect(208.0, 0.0, 270.0, 226.0)
    );
    assert_eq!(find(&placed, copy)?, rect(208.0, 226.0, 270.0, 190.0));
    Ok(())
}

/// A split through a fixed column makes two fixed halves of it, so nothing else moves:
/// the 340 column of Screens becomes 170 and 170, and the report below still spans both.
#[test]
fn a_split_fixed_column_halves_and_moves_nothing_else() -> Check {
    let mut layout = open(&SCREENS);
    let copy = PaneId(90);
    split(&mut layout, pane(Remote), Edge::Right, copy)?;
    let placed = drawn(&layout, FRAME_1)?;
    assert_eq!(find(&placed, pane(Remote))?, rect(404.0, 0.0, 170.0, 338.0));
    assert_eq!(find(&placed, copy)?, rect(574.0, 0.0, 170.0, 338.0));
    let want = [
        (Web, rect(0.0, 0.0, 404.0, 676.0)),
        (Report, rect(404.0, 338.0, 340.0, 338.0)),
        (Chat, rect(744.0, 0.0, 290.0, 676.0)),
    ];
    for (kind, rect) in want {
        assert_eq!(find(&placed, pane(kind))?, rect, "{kind:?}");
    }
    Ok(())
}

/// The owner: the pane next door grows into the space and gives it back on return. With
/// its slot gone, the open file of Code docks left of Changes, which took its room there.
#[test]
fn a_pane_with_no_slot_docks_beside_the_pane_that_took_its_room() -> Check {
    let mut code = open(&CODE);
    leave(&mut code, pane(Code))?;
    let mut layout = open(&REVIEW);
    layout.away = code.away;
    rejoin(&mut layout, pane(Code))?;
    assert!(layout.away.is_empty(), "it is in again");
    let want = [
        (Files, rect(0.0, 0.0, 220.0, 676.0)),
        (Code, rect(220.0, 0.0, 257.0, 526.0)),
        (Changes, rect(477.0, 0.0, 257.0, 526.0)),
        (Terminal, rect(220.0, 526.0, 514.0, 150.0)),
        (Chat, rect(734.0, 0.0, 300.0, 676.0)),
    ];
    expect(&drawn(&layout, FRAME_1)?, &want)
}

/// Three by three, the middle row `d e p`: `p` has `b` above and `e` beside it. Were `b` to
/// take its room the grid would be a pinwheel round `e`, which could then never leave.
#[test]
fn a_gap_is_filled_so_that_the_next_one_can_be() -> Check {
    let size = Size { w: 900.0, h: 600.0 };
    let [a, b, d, e, p, c] = [1, 2, 3, 4, 5, 6].map(PaneId);
    let mut layout = Layout {
        cols: vec![Fr(1.0); 3],
        rows: vec![Fr(1.0); 3],
        areas: vec![
            (a, area(1, 1, 2, 3)),
            (b, area(1, 3, 2, 4)),
            (d, area(2, 1, 4, 2)),
            (e, area(2, 2, 3, 3)),
            (p, area(2, 3, 3, 4)),
            (c, area(3, 2, 4, 4)),
        ],
        away: Vec::new(),
        stacks: Vec::new(),
    };
    layout.check()?;
    leave(&mut layout, p)?;
    let placed = drawn(&layout, size)?;
    assert_eq!(find(&placed, e)?, rect(300.0, 200.0, 600.0, 200.0));
    assert_eq!(find(&placed, b)?, rect(600.0, 0.0, 300.0, 200.0));
    leave(&mut layout, e)?;
    let placed = drawn(&layout, size)?;
    assert_eq!(placed.len(), 4);
    assert_eq!(find(&placed, c)?, rect(300.0, 200.0, 600.0, 400.0));
    Ok(())
}

/// A grid has slots for sixty-four panes: the split that would make one more is refused,
/// and a layout with more slots than that is not one a file may hold.
#[test]
fn a_grid_holds_sixty_four_panes_and_no_more() -> Check {
    let mut layout = Layout::single(PaneId(1));
    for id in 2..=32 {
        split(&mut layout, PaneId(id - 1), Edge::Right, PaneId(id))?;
    }
    for id in 1..=32 {
        split(&mut layout, PaneId(id), Edge::Bottom, PaneId(id + 32))?;
    }
    layout.check()?;
    let shape = (layout.cols.len(), layout.rows.len(), layout.areas.len());
    assert_eq!(shape, (32, 2, 64));
    let size = Size {
        w: 5120.0,
        h: 240.0,
    };
    assert_eq!(drawn(&layout, size)?.len(), 64);
    let one_more = split(&mut layout, PaneId(1), Edge::Bottom, PaneId(65));
    assert_eq!(one_more, Err(GridError::Full));
    let slot = |at: u16| area(at / 9 + 1, at % 9 + 1, at / 9 + 2, at % 9 + 2);
    let crowded = Layout {
        cols: vec![Fr(1.0); 9],
        rows: vec![Fr(1.0); 8],
        areas: (0..72)
            .map(|at| (PaneId(u32::from(at)), slot(at)))
            .collect(),
        away: Vec::new(),
        stacks: Vec::new(),
    };
    assert_eq!(crowded.check(), Err(GridError::NotTiled));
    Ok(())
}

/// A layout from a file is refused unless its slots tile the grid and come apart cut by
/// cut: an overlap, a gap, a pinwheel and a pane with two slots all fail.
#[test]
fn a_layout_that_does_not_tile_is_refused() -> Check {
    for saved in SAVED {
        open(&saved).check()?;
    }
    let with = |areas: Vec<(PaneId, crate::grid::layout::Area)>| Layout {
        cols: vec![Fr(1.0); 3],
        rows: vec![Fr(1.0); 3],
        areas,
        away: Vec::new(),
        stacks: Vec::new(),
    };
    let [a, b, c, d, e] = [1, 2, 3, 4, 5].map(PaneId);
    let overlap = with(vec![(a, area(1, 1, 4, 3)), (b, area(1, 2, 4, 4))]);
    let gap = with(vec![(a, area(1, 1, 4, 2)), (b, area(1, 3, 4, 4))]);
    let pinwheel = with(vec![
        (a, area(1, 1, 2, 3)),
        (b, area(1, 3, 3, 4)),
        (c, area(3, 2, 4, 4)),
        (d, area(2, 1, 4, 2)),
        (e, area(2, 2, 3, 3)),
    ]);
    for (name, layout) in [("overlap", overlap), ("gap", gap), ("pinwheel", pinwheel)] {
        assert_eq!(layout.check(), Err(GridError::NotTiled), "{name}");
    }
    let twice = with(vec![(a, area(1, 1, 4, 2)), (a, area(1, 2, 4, 4))]);
    assert_eq!(twice.check(), Err(GridError::AlreadyIn(a)));
    Ok(())
}
