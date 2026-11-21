//! A pane about one thing: a child agent's chat is its own pane, a tab of the work place.

use crate::address::uri::Address;
use crate::grid::place::probe::{Check, find};
use crate::grid::windows::Windows;
use crate::ids::PaneId;
use crate::kind::Kind;
use crate::model::Model;
use crate::pane::PaneKind::{Changes, Chat};

use super::GridMsg;
use super::stacks::{open, show, window};
use super::tests::{MAIN, change, drawn, pane};

fn child(name: &str) -> Result<Address, String> {
    Address::new(Kind::Agent, format!("a/{name}")).map_err(|error| error.to_string())
}

/// Asks for the chat about `thing` and gives back the pane that is about it.
fn about(model: &mut Model, thing: &Address) -> Result<PaneId, String> {
    let (window, kind, thing) = (MAIN, Chat, thing.clone());
    let asked = GridMsg::About {
        window,
        kind,
        thing: thing.clone(),
    };
    change(model, asked);
    let found = model
        .grid
        .panes
        .iter()
        .find(|pane| pane.thing == Some(thing.clone()));
    found
        .map(|pane| pane.id)
        .ok_or("no pane is about it".to_owned())
}

/// A child's chat opens as a tab of the work place, not a slot of its own; asked for again it
/// is the same pane; another child is another tab; and Chat alone is still the session's.
#[test]
fn a_chat_about_a_child_is_a_tab_of_work_and_is_found_again() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    let work = find(&drawn(&model)?, pane(&model, Changes)?)?;
    let (one, two) = (child("one")?, child("two")?);
    let first = about(&mut model, &one)?;
    assert_eq!(drawn(&model)?.len(), 2, "no slot was cut for it");
    assert_eq!(find(&drawn(&model)?, first)?, work, "in front, in work");
    assert_eq!(
        model.grid.tabs(MAIN, first),
        [pane(&model, Changes)?, first]
    );
    let second = about(&mut model, &two)?;
    assert_ne!(second, first, "another child is another pane");
    assert_eq!(model.grid.tabs(MAIN, second).len(), 3);
    assert_eq!(about(&mut model, &one)?, first);
    assert_eq!(model.grid.panes.len(), 4, "asked for again, none is made");
    assert_eq!(find(&drawn(&model)?, first)?, work, "and it is in front");
    let before = model.grid.windows.clone();
    open(&mut model, Chat);
    assert_eq!(model.grid.windows, before, "the session's chat was in view");
    assert_eq!(model.grid.panes.len(), 4);
    Ok(())
}

/// A child's chat is its session's: another session does not show it, and back in its own
/// it is in front where it was.
#[test]
fn a_childs_chat_shows_in_its_own_session_only() -> Check {
    let mut model = window("a");
    let first = about(&mut model, &child("one")?)?;
    let slot = find(&drawn(&model)?, first)?;
    show(&mut model, "b");
    assert_eq!(drawn(&model)?.len(), 1, "b has the chat alone");
    show(&mut model, "a");
    assert_eq!(find(&drawn(&model)?, first)?, slot);
    Ok(())
}

/// A layout seats its chat with a chat about nothing, never with a child's; a child's chat
/// a layout hid comes back as the pane it was, and is not handed to another child.
#[test]
fn a_layout_seats_no_pane_that_is_about_a_thing() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    let (one, two) = (child("one")?, child("two")?);
    let first = about(&mut model, &one)?;
    let own = pane(&model, Chat)?;
    assert_ne!(own, first);
    change(&mut model, GridMsg::Close(own));
    assert_eq!(model.grid.kind(own), None, "the session's chat is closed");
    let (window, saved) = (MAIN, "chat");
    change(&mut model, GridMsg::Show { window, saved });
    let seated = drawn(&model)?;
    let [(seat, _)] = seated.as_slice() else {
        return Err(format!("the chat layout draws {seated:?}").into());
    };
    assert_ne!(*seat, first, "the layout's chat is a chat about nothing");
    assert_eq!(model.grid.thing(*seat), None);
    let second = about(&mut model, &two)?;
    assert_ne!(second, first, "the hidden pane stays the first child's");
    assert_eq!(about(&mut model, &one)?, first, "and comes back as it was");
    assert_eq!(drawn(&model)?.len(), 2, "a tab of the slot a child opened");
    Ok(())
}

/// A child's chat pulled out into a window of its own is where that child shows: asked for
/// again, no second pane about it is made.
#[test]
fn a_childs_chat_out_in_its_own_window_is_not_opened_twice() -> Check {
    let mut model = window("a");
    let one = child("one")?;
    let first = about(&mut model, &one)?;
    change(&mut model, GridMsg::Leave(first));
    assert_ne!(model.grid.window_of(first), Some(MAIN));
    let before = model.grid.clone();
    assert_eq!(about(&mut model, &one)?, first);
    assert_eq!(model.grid, before);
    Ok(())
}

/// The grid file keeps what a pane is about, and refuses a pane about no address at all.
#[test]
fn the_grid_file_keeps_what_a_pane_is_about() -> Check {
    let mut model = window("a");
    let first = about(&mut model, &child("one")?)?;
    let text = serde_json::to_string(&model.grid)?;
    assert!(
        text.contains("yi://agent/a%2Fone") || text.contains("yi://agent/a/one"),
        "{text}"
    );
    let read: Windows = serde_json::from_str(&text)?;
    assert_eq!(read.thing(first), model.grid.thing(first));
    let broken = text.replace("yi://agent/", "nowhere/");
    assert!(serde_json::from_str::<Windows>(&broken).is_err());
    Ok(())
}
