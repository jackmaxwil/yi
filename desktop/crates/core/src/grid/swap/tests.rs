//! Each drop a carried pane can make, measured in the record's frame 1 grid.

use crate::grid::layout::{ChatSide, Edge, GridError, Layout, Point, Rect, Size};
use crate::grid::leave::leave;
use crate::grid::place::place;
use crate::grid::place::probe::{Check, FRAME_1, find, open, pane, rect, tiled};
use crate::grid::saved::{EVERYTHING, REVIEW};
use crate::grid::step::{GridMsg, step};
use crate::grid::windows::{Pane, Window, Windows};
use crate::ids::WindowId;
use crate::pane::PaneKind::{self, Changes, Chat, Code, Files, Remote, Report, Terminal, Web};

use super::{Spot, land, swap, toward, zone_at};

fn expect(layout: &Layout, side: ChatSide, want: &[(PaneKind, Rect)]) -> Check {
    let placed = place(layout, FRAME_1, side);
    tiled(&placed, FRAME_1)?;
    assert_eq!(placed.len(), want.len(), "{placed:?}");
    for (kind, rect) in want {
        assert_eq!(find(&placed, pane(*kind))?, *rect, "{kind:?}");
    }
    Ok(())
}

/// Record 2.7.9: spans travel with the slot, so a pane dropped on the full-height chat
/// takes the full-height slot.
#[test]
fn a_dropped_pane_takes_the_slot_with_its_span() -> Check {
    let mut layout = open(&REVIEW);
    swap(&mut layout, pane(Terminal), pane(Chat))?;
    let want = [
        (Files, rect(0.0, 0.0, 220.0, 676.0)),
        (Changes, rect(220.0, 0.0, 514.0, 526.0)),
        (Chat, rect(220.0, 526.0, 514.0, 150.0)),
        (Terminal, rect(734.0, 0.0, 300.0, 676.0)),
    ];
    expect(&layout, ChatSide::Right, &want)
}

/// Record 2.7.9: only a pane that is in the grid is a target. One that is out as a
/// window keeps the slot it will come back to.
#[test]
fn a_pane_that_is_out_is_no_target() -> Check {
    let mut layout = open(&REVIEW);
    leave(&mut layout, pane(Files))?;
    let before = layout.clone();
    let refused = Err(GridError::NoSuchPane(pane(Files)));
    assert_eq!(swap(&mut layout, pane(Chat), pane(Files)), refused);
    assert_eq!(layout, before);
    Ok(())
}

/// The terminal dropped on the top edge of chat takes the top of chat's slot, and Changes
/// closes up over the room the terminal left.
#[test]
fn a_pane_dropped_on_an_edge_of_another_takes_that_half_of_its_slot() -> Check {
    let mut layout = open(&REVIEW);
    let at = Spot::Beside(pane(Chat), Edge::Top);
    land(&mut layout, ChatSide::Right, pane(Terminal), at, FRAME_1)?;
    let want = [
        (Files, rect(0.0, 0.0, 220.0, 676.0)),
        (Changes, rect(220.0, 0.0, 514.0, 676.0)),
        (Terminal, rect(734.0, 0.0, 300.0, 526.0)),
        (Chat, rect(734.0, 526.0, 300.0, 150.0)),
    ];
    expect(&layout, ChatSide::Right, &want)
}

/// With chat on the left an edge is the one drawn: the terminal dropped on the drawn left
/// of Changes is drawn on its left.
#[test]
fn an_edge_is_the_one_drawn_with_chat_on_the_left() -> Check {
    let mut layout = open(&REVIEW);
    let at = Spot::Beside(pane(Changes), Edge::Left);
    land(&mut layout, ChatSide::Left, pane(Terminal), at, FRAME_1)?;
    let want = [
        (Chat, rect(0.0, 0.0, 300.0, 676.0)),
        (Terminal, rect(300.0, 0.0, 257.0, 676.0)),
        (Changes, rect(557.0, 0.0, 257.0, 676.0)),
        (Files, rect(814.0, 0.0, 220.0, 676.0)),
    ];
    expect(&layout, ChatSide::Left, &want)
}

/// Files dropped on the window's right edge becomes a full-height column there, as wide as
/// Review's chat column, and its own column closes.
#[test]
fn a_pane_dropped_on_the_windows_edge_becomes_a_column_across_the_grid() -> Check {
    let mut layout = open(&REVIEW);
    land(
        &mut layout,
        ChatSide::Right,
        pane(Files),
        Spot::Side(Edge::Right),
        FRAME_1,
    )?;
    let want = [
        (Changes, rect(0.0, 0.0, 434.0, 526.0)),
        (Terminal, rect(0.0, 526.0, 434.0, 150.0)),
        (Chat, rect(434.0, 0.0, 300.0, 676.0)),
        (Files, rect(734.0, 0.0, 300.0, 676.0)),
    ];
    expect(&layout, ChatSide::Right, &want)
}

#[test]
fn a_pane_dropped_on_the_windows_foot_becomes_a_row_across_the_grid() -> Check {
    let mut layout = open(&REVIEW);
    land(
        &mut layout,
        ChatSide::Right,
        pane(Files),
        Spot::Side(Edge::Bottom),
        FRAME_1,
    )?;
    let want = [
        (Changes, rect(0.0, 0.0, 734.0, 376.0)),
        (Terminal, rect(0.0, 376.0, 734.0, 150.0)),
        (Chat, rect(734.0, 0.0, 300.0, 526.0)),
        (Files, rect(0.0, 526.0, 1034.0, 150.0)),
    ];
    expect(&layout, ChatSide::Right, &want)
}

/// A fifth column in Everything's narrowest grid leaves every column under the 160
/// minimum: refused, and the grid is as it was.
#[test]
fn a_drop_that_would_go_under_the_minimum_is_refused() -> Check {
    let narrow = Size { w: 704.0, h: 676.0 };
    let mut layout = open(&EVERYTHING);
    let at = Spot::Beside(pane(Code), Edge::Left);
    let refused = land(&mut layout, ChatSide::Right, pane(Terminal), at, narrow);
    assert!(matches!(refused, Err(GridError::Cramped(_))), "{refused:?}");
    let kinds = [Files, Changes, Terminal, Web, Remote, Report, Code, Chat];
    let panes = kinds.map(|kind| Pane {
        id: pane(kind),
        kind,
        seated: false,
        scope: Default::default(),
        thing: None,
        unseen: false,
    });
    let window = Window {
        id: WindowId(0),
        layout: open(&EVERYTHING),
        saved: None,
        own: None,
        frame: None,
    };
    let mut grid = Windows {
        side: ChatSide::Right,
        panes: panes.to_vec(),
        windows: vec![window],
        own: Vec::new(),
        ..Windows::default()
    };
    let before = grid.clone();
    let drop = GridMsg::Drop {
        window: WindowId(0),
        pane: pane(Terminal),
        at,
        size: narrow,
    };
    assert!(step(&mut grid, drop).is_empty());
    assert_eq!(grid, before);
    Ok(())
}

/// The zones over chat while the terminal is carried: a quarter from each edge, the middle,
/// and the window's own 12-point edge, each saying where the terminal would be drawn.
#[test]
fn each_zone_says_where_the_carried_pane_would_land() -> Check {
    let layout = open(&REVIEW);
    let zone = |x: f32, y: f32| {
        let at = Point { x, y };
        zone_at(&layout, ChatSide::Right, FRAME_1, pane(Terminal), at)
    };
    let middle = zone(884.0, 338.0).ok_or("no zone in chat's middle")?;
    assert_eq!(middle.spot, Spot::Onto(pane(Chat)));
    assert_eq!(middle.lands, Some(rect(734.0, 0.0, 300.0, 676.0)));
    let top = zone(884.0, 100.0).ok_or("no zone at chat's top")?;
    assert_eq!(top.spot, Spot::Beside(pane(Chat), Edge::Top));
    assert_eq!(top.band, rect(734.0, 0.0, 300.0, 169.0));
    assert_eq!(top.lands, Some(rect(734.0, 0.0, 300.0, 526.0)));
    let left = zone(744.0, 338.0).ok_or("no zone at chat's left")?;
    assert_eq!(left.spot, Spot::Beside(pane(Chat), Edge::Left));
    // Chat's column cut in two is written 150 a side and drawn at the 160 minimum.
    assert_eq!(left.lands, Some(rect(714.0, 0.0, 160.0, 676.0)));
    let side = zone(1030.0, 338.0).ok_or("no zone at the window's edge")?;
    assert_eq!(side.spot, Spot::Side(Edge::Right));
    assert_eq!(side.band, rect(1022.0, 0.0, 12.0, 676.0));
    assert_eq!(side.lands, Some(rect(734.0, 0.0, 300.0, 676.0)));
    assert_eq!(
        zone(400.0, 600.0),
        None,
        "over its own slot nothing changes"
    );
    Ok(())
}

/// A key moves chat onto the left edge of Changes, the pane drawn just left of it; Files,
/// with nothing left of it, goes along the window's left edge.
#[test]
fn a_key_moves_a_pane_onto_the_edge_of_its_neighbour() -> Check {
    let layout = open(&REVIEW);
    let side = ChatSide::Right;
    let chat = toward(&layout, side, FRAME_1, pane(Chat), Edge::Left);
    assert_eq!(chat, Spot::Beside(pane(Changes), Edge::Left));
    let files = toward(&layout, side, FRAME_1, pane(Files), Edge::Left);
    assert_eq!(files, Spot::Side(Edge::Left));
    let mut moved = layout.clone();
    land(&mut moved, side, pane(Chat), chat, FRAME_1)?;
    let want = [
        (Files, rect(0.0, 0.0, 220.0, 676.0)),
        (Chat, rect(220.0, 0.0, 407.0, 526.0)),
        (Changes, rect(627.0, 0.0, 407.0, 526.0)),
        (Terminal, rect(220.0, 526.0, 814.0, 150.0)),
    ];
    expect(&moved, side, &want)
}

/// A drop is a layout change like any other: the grid file written after it reads back the
/// same, the new column included.
#[test]
fn a_docked_grid_is_saved_and_read_back_as_it_is() -> Check {
    let kinds = [Files, Changes, Terminal, Chat];
    let window = Window {
        id: WindowId(0),
        layout: open(&REVIEW),
        saved: Some(REVIEW.slug),
        own: None,
        frame: None,
    };
    let mut grid = Windows {
        side: ChatSide::Right,
        panes: kinds
            .map(|kind| Pane {
                id: pane(kind),
                kind,
                seated: false,
                scope: Default::default(),
                thing: None,
                unseen: false,
            })
            .to_vec(),
        windows: vec![window],
        own: Vec::new(),
        ..Windows::default()
    };
    let at = Spot::Side(Edge::Bottom);
    let drop = GridMsg::Drop {
        window: WindowId(0),
        pane: pane(Files),
        at,
        size: FRAME_1,
    };
    step(&mut grid, drop);
    assert_eq!(grid.windows.first().and_then(|window| window.saved), None);
    let file = serde_json::to_string(&grid)?;
    assert_eq!(serde_json::from_str::<Windows>(&file)?, grid);
    Ok(())
}
