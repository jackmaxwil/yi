//! A pane about one thing shown beside: a slot of its own cut from the slot its place's tabs
//! are in, or from a pane the person named.

use crate::address::uri::Address;
use crate::command::table::{FLIP_CHAT, Route, Run};
use crate::grid::layout::{Edge, Rect};
use crate::grid::place::probe::Check;
use crate::ids::{PaneId, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::pane::PaneKind::{self, Changes, Chat, Code, Debugger, Terminal};
use crate::pane::Place;

use super::GridMsg;
use super::tests::{MAIN, change, drawn, pane, record};

fn showing(saved: &'static str) -> Model {
    let mut model = record();
    let window = MAIN;
    change(&mut model, GridMsg::Show { window, saved });
    let kinds = vec![Chat, Code, Changes, Terminal];
    let thread = Some(ThreadId::new("a"));
    change(
        &mut model,
        GridMsg::Have {
            window,
            kinds,
            thread,
        },
    );
    model
}

fn file(path: &str) -> Result<Address, String> {
    Address::new(Kind::File, path).map_err(|error| error.to_string())
}

/// Asks for the code pane about `path` beside, and gives back the pane that is about it.
fn beside(
    model: &mut Model,
    path: &str,
    pane: Option<PaneId>,
    edge: Edge,
) -> Result<PaneId, String> {
    let (window, kind, thing) = (MAIN, Code, file(path)?);
    let asked = GridMsg::Beside {
        window,
        kind,
        thing: thing.clone(),
        pane,
        edge,
    };
    change(model, asked);
    let about = |held: &&crate::grid::windows::Pane| held.thing.as_ref() == Some(&thing);
    let found = model.grid.panes.iter().find(about);
    found
        .map(|pane| pane.id)
        .ok_or(format!("no pane is about {path}"))
}

fn rect(model: &Model, pane: PaneId) -> Result<Rect, String> {
    let found = drawn(model)?.into_iter().find(|(id, _)| *id == pane);
    found
        .map(|(_, rect)| rect)
        .ok_or(format!("{pane:?} is not drawn"))
}

fn count(model: &Model, kind: PaneKind) -> usize {
    let of_kind = |pane: &&crate::grid::windows::Pane| pane.kind == kind;
    model.grid.panes.iter().filter(of_kind).count()
}

/// A file opened beside is a code pane about it in a slot cut from the right of one slot of
/// the work place; asked for again it is the same pane, and another file is another pane.
#[test]
fn a_file_shown_beside_takes_a_slot_right_of_the_work_tabs_and_is_found_again() -> Check {
    let mut model = showing("everything");
    let before = drawn(&model)?;
    let one = beside(&mut model, "src/a.rs", None, Edge::Right)?;
    assert_eq!(drawn(&model)?.len(), before.len() + 1, "a slot of its own");
    let moved = |(pane, was): &(PaneId, Rect)| Some((*pane, *was, rect(&model, *pane).ok()?));
    let cut: Vec<_> = before.iter().filter_map(moved).collect();
    let cut: Vec<_> = cut.into_iter().filter(|(_, was, now)| was != now).collect();
    let [(host, was, left)] = cut[..] else {
        return Err(format!("one slot gives room: {cut:?}").into());
    };
    assert_eq!(model.grid.slot_place(host), Some(Place::Work));
    let right = rect(&model, one)?;
    assert_eq!((left.x, left.y, left.h), (was.x, was.y, was.h));
    assert_eq!((right.y, right.h), (was.y, was.h), "the same row");
    assert!(right.x > left.x, "right of it: {left:?} {right:?}");
    assert_eq!(model.grid.tabs(MAIN, one), [one], "a tab of no stack");
    assert_eq!(model.grid.kept(one), Some(false), "the person's, here");
    let before = model.grid.clone();
    assert_eq!(beside(&mut model, "src/a.rs", None, Edge::Right)?, one);
    assert_eq!(model.grid, before, "shown once, nothing moves");
    let two = beside(&mut model, "src/b.rs", None, Edge::Right)?;
    assert_ne!(two, one, "another file is another pane");
    assert_eq!(count(&model, Code), 3);
    Ok(())
}

/// Named a pane and an edge, the slot is cut from that edge of that pane's slot as drawn,
/// with the chat on either side; a tab behind names the slot it waits in.
#[test]
fn beside_a_named_pane_the_slot_is_cut_from_the_edge_named_as_drawn() -> Check {
    for flipped in [false, true] {
        let mut model = showing("everything");
        if flipped {
            step(&mut model, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
        }
        let terminal = pane(&model, Terminal)?;
        let was = rect(&model, terminal)?;
        let left = beside(&mut model, "src/a.rs", Some(terminal), Edge::Left)?;
        let (made, kept) = (rect(&model, left)?, rect(&model, terminal)?);
        assert_eq!((made.x, made.y, made.h), (was.x, was.y, was.h));
        assert!(kept.x > made.x, "drawn left of it, flipped {flipped}");
        let under = beside(&mut model, "src/b.rs", Some(left), Edge::Bottom)?;
        let (top, bottom) = (rect(&model, left)?, rect(&model, under)?);
        assert_eq!((bottom.x, bottom.w), (top.x, top.w), "under it");
        assert!(bottom.y > top.y);
    }
    let mut model = showing("everything");
    let (window, kind) = (MAIN, Debugger);
    change(&mut model, GridMsg::Open { window, kind });
    let stack = model.grid.tabs(MAIN, pane(&model, Debugger)?);
    let behind = stack
        .iter()
        .copied()
        .find(|tab| rect(&model, *tab).is_err());
    let behind = behind.ok_or("no tab behind")?;
    let front = stack.iter().copied().find(|tab| *tab != behind);
    let was = rect(&model, front.ok_or("no tab in front")?)?;
    let made = beside(&mut model, "src/c.rs", Some(behind), Edge::Right)?;
    let now = rect(&model, made)?;
    assert_eq!(
        (now.y, now.h),
        (was.y, was.h),
        "cut from the slot it waits in"
    );
    assert!(now.x > was.x && now.x < was.x + was.w);
    Ok(())
}

/// Where no slot of its place is open, beside is the same as shown: the place opens. And the
/// pane is its session's, as any pane the person opens.
#[test]
fn beside_opens_the_place_where_none_is_and_the_pane_is_its_sessions() -> Check {
    let mut model = showing("just-chat");
    assert_eq!(drawn(&model)?.len(), 1, "the chat alone");
    let one = beside(&mut model, "src/a.rs", None, Edge::Right)?;
    assert_eq!(drawn(&model)?.len(), 2);
    let slot = rect(&model, one)?;
    let (window, kinds) = (MAIN, vec![Chat]);
    let thread = Some(ThreadId::new("b"));
    change(
        &mut model,
        GridMsg::Have {
            window,
            kinds,
            thread,
        },
    );
    assert!(rect(&model, one).is_err(), "another session has none of it");
    let (kinds, thread) = (vec![Chat], Some(ThreadId::new("a")));
    change(
        &mut model,
        GridMsg::Have {
            window,
            kinds,
            thread,
        },
    );
    assert_eq!(rect(&model, one)?, slot, "back where it was");
    Ok(())
}
