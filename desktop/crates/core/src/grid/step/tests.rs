//! Grid changes through the model's one `step`: layouts shown, panes pulled out and docked,
//! split and closed, and the grid written to a file and read back.

use crate::command::table::{FLIP_CHAT, Route, Run};
use crate::grid::layout::{Axis, Edge, Line, MAX_TRACKS, Point, Rect, Size};
use crate::grid::place::probe::{Check, FRAME_1, find, rect, tiled};
use crate::grid::windows::{Frame, WindowOp, Windows};
use crate::ids::{PaneId, ThreadId, WindowId};
use crate::model::Model;
use crate::msg::{Effect, Msg, step};
use crate::pane::PaneKind::{self, Changes, Chat, Code, Files, Remote, Report, Terminal, Web};

use super::GridMsg;

pub(super) const MAIN: WindowId = WindowId(0);

/// Applies one grid change and gives back what it asked of the real windows.
pub(super) fn change(model: &mut Model, change: GridMsg) -> Vec<WindowOp> {
    let effects = step(model, Msg::Grid(change));
    let op = |effect: Effect| match effect {
        Effect::Window(op) => Some(op),
        Effect::Wire(_) | Effect::Source(..) | Effect::Notify(_) => None,
    };
    effects.into_iter().filter_map(op).collect()
}

/// A model drawn as the record measures its frames: chat on the right.
pub(super) fn record() -> Model {
    let mut model = Model::default();
    model.grid.side = crate::grid::layout::ChatSide::Right;
    model
}

pub(super) fn showing(saved: &'static str) -> Model {
    let mut model = record();
    let window = MAIN;
    change(&mut model, GridMsg::Show { window, saved });
    model
}

pub(super) fn pane(model: &Model, kind: PaneKind) -> Result<PaneId, String> {
    let found = model.grid.panes.iter().find(|pane| pane.kind == kind);
    found
        .map(|pane| pane.id)
        .ok_or(format!("there is no {kind:?} pane"))
}

/// The main window's panes in the frame 1 grid, after checking they tile it exactly.
pub(super) fn drawn(model: &Model) -> Result<Vec<(PaneId, Rect)>, String> {
    let placed = model.grid.place(MAIN, FRAME_1);
    tiled(&placed, FRAME_1)?;
    Ok(placed)
}

pub(super) const HAVE_KINDS: [PaneKind; 6] = [Chat, Code, Changes, Files, Terminal, Web];

pub(super) fn have(model: &mut Model, kinds: &[PaneKind], thread: Option<&str>) {
    let thread = thread.map(ThreadId::new);
    change(
        model,
        GridMsg::Have {
            window: MAIN,
            kinds: kinds.to_vec(),
            thread,
        },
    );
}

pub(super) fn expect(model: &Model, want: &[(PaneKind, Rect)]) -> Check {
    let placed = drawn(model)?;
    assert_eq!(placed.len(), want.len(), "panes drawn: {placed:?}");
    for (kind, rect) in want {
        assert_eq!(find(&placed, pane(model, *kind)?)?, *rect, "{kind:?}");
    }
    Ok(())
}

/// Record 2.7.4: Review in the frame 1 grid.
pub(super) const REVIEW_PLACED: [(PaneKind, Rect); 4] = [
    (
        Files,
        Rect {
            x: 0.0,
            y: 0.0,
            w: 220.0,
            h: 676.0,
        },
    ),
    (
        Changes,
        Rect {
            x: 220.0,
            y: 0.0,
            w: 514.0,
            h: 526.0,
        },
    ),
    (
        Terminal,
        Rect {
            x: 220.0,
            y: 526.0,
            w: 514.0,
            h: 150.0,
        },
    ),
    (
        Chat,
        Rect {
            x: 734.0,
            y: 0.0,
            w: 300.0,
            h: 676.0,
        },
    ),
];

/// The first layout shown makes the window and a pane of each kind the record lists, and
/// the window says which layout it shows: the shell reads that to know Just chat.
#[test]
fn a_saved_layout_shows_in_a_window_by_its_slug() -> Check {
    let mut model = record();
    let (window, saved) = (MAIN, "review");
    let ops = change(&mut model, GridMsg::Show { window, saved });
    assert_eq!(ops, [WindowOp::Open(MAIN)]);
    assert_eq!(model.grid.panes.len(), 4);
    expect(&model, &REVIEW_PLACED)?;
    let shows = |model: &Model| model.grid.window(MAIN).and_then(|window| window.saved);
    assert_eq!(shows(&model), Some("review"));
    let saved = "no such layout";
    assert_eq!(change(&mut model, GridMsg::Show { window, saved }), []);
    expect(&model, &REVIEW_PLACED)?;
    let saved = "just-chat";
    assert_eq!(change(&mut model, GridMsg::Show { window, saved }), []);
    assert_eq!(shows(&model), Some("just-chat"));
    Ok(())
}

/// Record 2.8.1 and 2.8.3: a pane pulled out is a window of one and the grid closes up
/// behind it; docked back, everything is as it was.
#[test]
fn pulling_a_pane_out_opens_a_window_and_docking_it_closes_it() -> Check {
    let mut model = showing("everything");
    let before = model.grid.clone();
    let web = pane(&model, Web)?;
    let ops = change(&mut model, GridMsg::Leave(web));
    let [WindowOp::Open(own)] = ops[..] else {
        return Err(format!("pulling out asked for {ops:?}").into());
    };
    assert_eq!(model.grid.window_of(web), Some(own));
    assert_eq!(model.grid.origin_of(web), Some(MAIN));
    let size = Size { w: 312.0, h: 296.0 };
    assert_eq!(
        model.grid.place(own, size),
        [(web, rect(0.0, 0.0, 312.0, 296.0))]
    );
    let placed = drawn(&model)?;
    assert_eq!(placed.len(), 7);
    assert_eq!(
        find(&placed, pane(&model, Remote)?)?,
        rect(478.0, 0.0, 286.0, 416.0)
    );
    assert_eq!(
        change(&mut model, GridMsg::Rejoin(web)),
        [WindowOp::Close(own)]
    );
    assert_eq!(model.grid, before);
    Ok(())
}

/// Record 2.7.5: a new layout closes up behind a pane that is a window, and the panes it
/// leaves out are kept: shown again, they are the same panes.
#[test]
fn a_layout_switch_keeps_what_is_out_out_and_what_is_hidden_alive() -> Check {
    let mut model = showing("everything");
    let panes = model.grid.panes.clone();
    let web = pane(&model, Web)?;
    change(&mut model, GridMsg::Leave(web));
    let own = model.grid.window_of(web);
    let window = MAIN;
    let saved = "review";
    assert_eq!(change(&mut model, GridMsg::Show { window, saved }), []);
    expect(&model, &REVIEW_PLACED)?;
    let saved = "everything";
    change(&mut model, GridMsg::Show { window, saved });
    assert_eq!(model.grid.panes, panes, "no pane was lost or made");
    assert_eq!(
        model.grid.window_of(web),
        own,
        "web is still its own window"
    );
    let placed = drawn(&model)?;
    assert_eq!(placed.len(), 7);
    assert_eq!(
        find(&placed, pane(&model, Remote)?)?,
        rect(478.0, 0.0, 286.0, 416.0)
    );
    change(&mut model, GridMsg::Rejoin(web));
    assert_eq!(find(&drawn(&model)?, web)?, rect(478.0, 0.0, 286.0, 226.0));
    Ok(())
}

/// With two chats, one out as a window and one in view, a new layout gives its chat slot
/// to the one in view: what the window showed stays, and the window of the other too.
#[test]
fn a_layout_switch_keeps_the_pane_in_view_in_view() -> Check {
    let mut model = showing("just-chat");
    let chat = pane(&model, Chat)?;
    change(
        &mut model,
        GridMsg::SplitAt {
            pane: chat,
            edge: Edge::Right,
        },
    );
    let copy = model.grid.panes.last().ok_or("no pane")?.id;
    change(&mut model, GridMsg::Leave(chat));
    let own = model.grid.window_of(chat);
    let (window, saved) = (MAIN, "everything");
    assert_eq!(change(&mut model, GridMsg::Show { window, saved }), []);
    assert_eq!(model.grid.window_of(copy), Some(MAIN));
    assert_eq!(
        model.grid.window_of(chat),
        own,
        "the first chat keeps its window"
    );
    let placed = drawn(&model)?;
    assert_eq!(placed.len(), 8);
    assert_eq!(find(&placed, copy)?, rect(764.0, 226.0, 270.0, 450.0));
    Ok(())
}

/// The brief: docked into a layout with no slot for it, a pane goes beside the pane it
/// last neighboured. Web left Everything; in Review it lands to the right of Changes.
#[test]
fn a_pane_docks_beside_its_old_neighbour_when_its_slot_is_gone() -> Check {
    let mut model = showing("everything");
    let web = pane(&model, Web)?;
    change(&mut model, GridMsg::Leave(web));
    let (window, saved) = (MAIN, "review");
    change(&mut model, GridMsg::Show { window, saved });
    assert_eq!(change(&mut model, GridMsg::Rejoin(web)).len(), 1);
    let want = [
        (Files, rect(0.0, 0.0, 220.0, 676.0)),
        (Changes, rect(220.0, 0.0, 257.0, 526.0)),
        (Web, rect(477.0, 0.0, 257.0, 526.0)),
        (Terminal, rect(220.0, 526.0, 514.0, 150.0)),
        (Chat, rect(734.0, 0.0, 300.0, 676.0)),
    ];
    expect(&model, &want)?;
    let shows = model.grid.window(MAIN).and_then(|window| window.saved);
    assert_eq!(shows, None, "it is Review no longer");
    Ok(())
}

/// A layout whose only pane is out as a window cannot close up round it: the pane comes
/// home and its window closes.
#[test]
fn a_layout_made_only_of_a_pane_that_is_out_brings_it_home() -> Check {
    let mut model = showing("everything");
    let chat = pane(&model, Chat)?;
    change(&mut model, GridMsg::Leave(chat));
    let own = model.grid.window_of(chat).ok_or("chat has no window")?;
    let (window, saved) = (MAIN, "just-chat");
    let ops = change(&mut model, GridMsg::Show { window, saved });
    assert_eq!(ops, [WindowOp::Close(own)]);
    assert_eq!(drawn(&model)?, [(chat, rect(0.0, 0.0, 1034.0, 676.0))]);
    Ok(())
}

/// Plan default 4: a split makes a second pane of the same kind on the right as drawn,
/// which with chat on the left is the other side of the layout.
#[test]
fn a_split_goes_to_the_right_as_drawn_with_chat_on_either_side() -> Check {
    for flipped in [false, true] {
        let mut model = showing("review");
        if flipped {
            step(&mut model, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
        }
        let changes = pane(&model, Changes)?;
        change(
            &mut model,
            GridMsg::SplitAt {
                pane: changes,
                edge: Edge::Right,
            },
        );
        let copy = model.grid.panes.last().ok_or("no pane")?;
        assert_eq!((copy.kind, model.grid.panes.len()), (Changes, 5));
        let placed = drawn(&model)?;
        let (old, new) = (find(&placed, changes)?, find(&placed, copy.id)?);
        let left = if flipped { 300.0 } else { 220.0 };
        assert_eq!(old, rect(left, 0.0, 257.0, 526.0), "flipped: {flipped}");
        assert_eq!(new, rect(left + 257.0, 0.0, 257.0, 526.0));
    }
    Ok(())
}

/// Plan default 4: closing a pane that is a window closes the window, forgets the pane and
/// leaves no slot for it; its neighbour keeps the room.
#[test]
fn closing_a_pulled_out_pane_closes_its_window_and_forgets_it() -> Check {
    let mut model = showing("everything");
    let web = pane(&model, Web)?;
    change(&mut model, GridMsg::Leave(web));
    let own = model.grid.window_of(web).ok_or("web has no window")?;
    assert_eq!(
        change(&mut model, GridMsg::Close(web)),
        [WindowOp::Close(own)]
    );
    assert_eq!(model.grid.kind(web), None);
    let main = model.grid.window(MAIN).ok_or("no main window")?;
    assert_eq!((main.layout.slot(web), main.layout.away.len()), (None, 0));
    let placed = drawn(&model)?;
    assert_eq!(placed.len(), 7);
    assert_eq!(
        find(&placed, pane(&model, Remote)?)?,
        rect(478.0, 0.0, 286.0, 416.0)
    );
    Ok(())
}

/// The app's window never ends up empty: with every other pane out as a window, the one
/// left can be neither pulled out nor closed.
#[test]
fn a_window_keeps_a_pane_while_others_are_out_of_it() -> Check {
    let mut model = showing("review");
    for kind in [Files, Changes, Terminal] {
        let pane = pane(&model, kind)?;
        assert_eq!(change(&mut model, GridMsg::Leave(pane)).len(), 1);
    }
    let chat = pane(&model, Chat)?;
    let before = model.grid.clone();
    assert_eq!(change(&mut model, GridMsg::Leave(chat)), []);
    assert_eq!(change(&mut model, GridMsg::Close(chat)), []);
    assert_eq!(model.grid, before);
    assert_eq!(drawn(&model)?, [(chat, rect(0.0, 0.0, 1034.0, 676.0))]);
    Ok(())
}

/// A grid holds 32 columns. The split that would make one more is refused whole: no
/// column, and no pane left over from the half of the change that did work.
#[test]
fn a_split_past_the_most_columns_changes_nothing() -> Check {
    let mut model = showing("just-chat");
    let chat = pane(&model, Chat)?;
    for made in 2..=MAX_TRACKS {
        change(
            &mut model,
            GridMsg::SplitAt {
                pane: chat,
                edge: Edge::Right,
            },
        );
        assert_eq!(model.grid.panes.len(), made, "the split that makes {made}");
    }
    let before = model.grid.clone();
    change(
        &mut model,
        GridMsg::SplitAt {
            pane: chat,
            edge: Edge::Right,
        },
    );
    assert_eq!(model.grid, before);
    assert_eq!(drawn(&model)?.len(), MAX_TRACKS);
    Ok(())
}

/// A drag names where the pointer is, as drawn: the line goes there, and with chat on the
/// left the same message moves the mirrored line.
#[test]
fn a_dragged_line_goes_to_the_pointer_on_either_chat_side() -> Check {
    let mut model = showing("everything");
    let (window, size) = (MAIN, FRAME_1);
    let line = Line {
        axis: Axis::Rows,
        after: 0,
    };
    let to = 256.0;
    change(
        &mut model,
        GridMsg::Resize {
            window,
            line,
            to,
            size,
        },
    );
    let placed = drawn(&model)?;
    assert_eq!(
        find(&placed, pane(&model, Web)?)?,
        rect(478.0, 0.0, 286.0, 256.0)
    );
    let shows = model.grid.window(MAIN).and_then(|window| window.saved);
    assert_eq!(shows, None, "a dragged layout is the saved one no longer");
    step(&mut model, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
    let at = Point { x: 270.0, y: 100.0 };
    let line = model.grid.line_at(MAIN, size, at).ok_or("no line there")?;
    let to = 300.0;
    change(
        &mut model,
        GridMsg::Resize {
            window,
            line,
            to,
            size,
        },
    );
    let placed = drawn(&model)?;
    assert_eq!(
        find(&placed, pane(&model, Code)?)?,
        rect(0.0, 0.0, 300.0, 256.0)
    );
    assert_eq!(
        find(&placed, pane(&model, Web)?)?,
        rect(300.0, 0.0, 256.0, 256.0)
    );
    change(&mut model, GridMsg::Even { window, line, size });
    let placed = drawn(&model)?;
    assert_eq!(find(&placed, pane(&model, Code)?)?.w, 278.0);
    assert_eq!(find(&placed, pane(&model, Report)?)?.w, 278.0);
    Ok(())
}

/// Record 2.8.2 and 2.8.5 as a file: frame 2's grid with web and the remote desktop out as
/// windows at the places the record gives.
pub(super) const FRAME_2: &str = r#"{
  "chat": "right",
  "panes": [
    {"id": 1, "kind": "files"}, {"id": 2, "kind": "web"}, {"id": 3, "kind": "changes"},
    {"id": 4, "kind": "terminal"}, {"id": 5, "kind": "remote"}, {"id": 6, "kind": "chat"}
  ],
  "windows": [
    {"id": 0, "layout": {
      "cols": [{"px": 184}, {"fr": 1}, {"px": 250}],
      "rows": [{"px": 214}, {"fr": 1}, {"px": 118}],
      "areas": [[1, [1, 1, 4, 2]], [2, [1, 2, 2, 3]], [3, [2, 2, 3, 3]],
                [4, [3, 2, 4, 3]], [5, [1, 3, 2, 4]], [6, [2, 3, 4, 4]]],
      "away": [{"pane": 2}, {"pane": 5}]}},
    {"id": 1, "frame": {"x": 722, "y": -16, "w": 312, "h": 296},
     "layout": {"cols": [{"fr": 1}], "rows": [{"fr": 1}], "areas": [[2, [1, 1, 2, 2]]]}},
    {"id": 2, "frame": {"x": 722, "y": 290, "w": 312, "h": 250},
     "layout": {"cols": [{"fr": 1}], "rows": [{"fr": 1}], "areas": [[5, [1, 1, 2, 2]]]}}
  ]
}"#;

/// A grid file written by hand from the record draws frame 2: the main grid closed up (its
/// 118 terminal at the plan's 120 minimum), and each window where the record puts it.
#[test]
fn a_grid_file_written_by_hand_draws_frame_2() -> Check {
    let grid: Windows = serde_json::from_str(FRAME_2)?;
    let size = Size { w: 704.0, h: 676.0 };
    let placed = grid.place(MAIN, size);
    tiled(&placed, size)?;
    let want = [
        (PaneId(1), rect(0.0, 0.0, 184.0, 676.0)),
        (PaneId(3), rect(184.0, 0.0, 270.0, 556.0)),
        (PaneId(4), rect(184.0, 556.0, 270.0, 120.0)),
        (PaneId(6), rect(454.0, 0.0, 250.0, 676.0)),
    ];
    assert_eq!(placed, want);
    let web = grid.window(WindowId(1)).ok_or("web has no window")?;
    let (x, y, w, h) = (722.0, -16.0, 312.0, 296.0);
    assert_eq!(web.frame, Some(Frame { x, y, w, h }));
    assert_eq!(grid.origin_of(PaneId(5)), Some(MAIN));
    Ok(())
}

/// A file the user broke is refused whole: overlapping slots, a pane drawn twice or out
/// and drawn nowhere, an unknown kind or pane, a negative track, a window listed twice.
#[test]
fn a_broken_grid_file_is_refused() -> Check {
    serde_json::from_str::<Windows>(FRAME_2)?;
    let broken = [
        ("[3, [2, 2, 3, 3]]", "[3, [1, 2, 3, 3]]"),
        (", {\"pane\": 5}]", "]"),
        (
            "\"away\": [{\"pane\": 2}, {\"pane\": 5}]",
            "\"away\": [{\"pane\": 2}, {\"pane\": 5}, {\"pane\": 4}]",
        ),
        ("\"kind\": \"web\"", "\"kind\": \"hologram\""),
        ("{\"px\": 184}", "{\"px\": -184}"),
        ("{\"id\": 2, \"frame\"", "{\"id\": 1, \"frame\""),
        ("{\"id\": 4, \"kind\": \"terminal\"}, ", ""),
    ];
    for (good, bad) in broken {
        let text = FRAME_2.replace(good, bad);
        assert_ne!(text, FRAME_2, "the file has no {good}");
        let read = serde_json::from_str::<Windows>(&text);
        assert!(read.is_err(), "{bad} was read: {read:?}");
    }
    let nowhere = FRAME_2.replace("\"w\": 312, \"h\": 296", "\"w\": 0, \"h\": 296");
    let grid: Windows = serde_json::from_str(&nowhere)?;
    let place = grid.window(WindowId(1)).and_then(|window| window.frame);
    assert_eq!(
        place, None,
        "a place of no width is forgotten, the file kept"
    );
    Ok(())
}

/// What a session did to the grid survives being saved: layouts dragged and swapped, a
/// pane out as a window, the last real place of that window, and chat's side.
#[test]
fn the_grid_reads_back_from_its_file_as_it_was() -> Check {
    let mut model = showing("everything");
    let (window, size) = (MAIN, FRAME_1);
    let (web, code, chat) = (pane(&model, Web)?, pane(&model, Code)?, pane(&model, Chat)?);
    change(&mut model, GridMsg::Leave(web));
    let own = model.grid.window_of(web).ok_or("web has no window")?;
    let (x, y, w, h) = (722.0, -16.0, 312.0, 296.0);
    let frame = Frame { x, y, w, h };
    change(&mut model, GridMsg::Moved { window: own, frame });
    let nowhere = Frame {
        w: f32::NAN,
        ..frame
    };
    let moved = GridMsg::Moved {
        window: own,
        frame: nowhere,
    };
    change(&mut model, moved);
    let line = Line {
        axis: Axis::Cols,
        after: 0,
    };
    let to = 250.5;
    change(
        &mut model,
        GridMsg::Resize {
            window,
            line,
            to,
            size,
        },
    );
    change(
        &mut model,
        GridMsg::Swap {
            window,
            a: code,
            b: chat,
        },
    );
    change(
        &mut model,
        GridMsg::SplitAt {
            pane: code,
            edge: Edge::Right,
        },
    );
    step(&mut model, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
    let moved = model.grid.window(own).and_then(|window| window.frame);
    assert_eq!(moved, Some(frame));
    let text = serde_json::to_string(&model.grid)?;
    let read: Windows = serde_json::from_str(&text)?;
    assert_eq!(read, model.grid, "{text}");
    Ok(())
}

/// A pane kind no saved layout holds joins the stack of its place as a tab in front, with
/// no new slot; asked again it is already there, and closed it comes back as the same pane.
#[test]
fn a_pane_of_a_kind_the_window_lacks_opens_as_a_tab_in_its_place() -> Check {
    let mut model = showing("everything");
    let before = drawn(&model)?;
    let terminal = pane(&model, Terminal)?;
    let shelf = find(&before, terminal)?;
    let (window, kind) = (MAIN, PaneKind::Trace);
    change(&mut model, GridMsg::Open { window, kind });
    let trace = pane(&model, PaneKind::Trace)?;
    let after = drawn(&model)?;
    assert_eq!(after.len(), before.len(), "no slot was cut for it");
    assert_eq!(
        find(&after, trace)?,
        shelf,
        "it is drawn where the terminal was"
    );
    assert_eq!(model.grid.tabs(MAIN, trace), [terminal, trace]);
    change(&mut model, GridMsg::Open { window, kind });
    assert_eq!(drawn(&model)?, after, "a second ask adds none");
    change(&mut model, GridMsg::Close(trace));
    assert_eq!(
        drawn(&model)?,
        before,
        "closed, the terminal has its slot again"
    );
    change(&mut model, GridMsg::Open { window, kind });
    assert_eq!(model.grid.window_of(trace), Some(MAIN), "the same pane");
    Ok(())
}
