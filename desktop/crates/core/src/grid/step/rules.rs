//! What a window opens by itself: a kind a rule names gets a quiet tab the first time its
//! session has something of it to show, and the tab goes when that ends.

use crate::grid::place::probe::Check;
use crate::grid::windows::Pane;
use crate::ids::{PaneId, ThreadId};
use crate::model::Model;
use crate::pane::PaneKind::{self, Agents, Changes, Chat, Code, Files, Web};

use super::GridMsg;
use super::tests::{MAIN, change, drawn, pane, record};

/// The chat alone in session `a`, told that `kinds` have something to show.
fn window(saved: &'static str, kinds: &[PaneKind]) -> Model {
    let mut model = record();
    let window = MAIN;
    change(&mut model, GridMsg::Show { window, saved });
    tell(&mut model, "a", kinds);
    model
}

fn tell(model: &mut Model, thread: &str, kinds: &[PaneKind]) {
    let (window, kinds) = (MAIN, kinds.to_vec());
    let thread = Some(ThreadId::new(thread));
    let told = GridMsg::Have {
        window,
        kinds,
        thread,
    };
    change(model, told);
}

fn open(model: &mut Model, kind: PaneKind) {
    let window = MAIN;
    change(model, GridMsg::Open { window, kind });
}

fn of(model: &Model, kind: PaneKind) -> Vec<&Pane> {
    let panes = model.grid.panes.iter();
    panes.filter(|pane| pane.kind == kind).collect()
}

fn tabs(model: &Model, of: PaneId) -> Result<Vec<PaneKind>, String> {
    let kind = |pane: &PaneId| model.grid.kind(*pane).ok_or("a tab of no kind".to_owned());
    model.grid.tabs(MAIN, of).iter().map(kind).collect()
}

/// The agent's first edit: Changes joins the work place as a tab behind the pane in front,
/// with a dot. The front tab stays; told the same again, nothing more opens.
#[test]
fn a_kind_with_something_new_opens_as_a_quiet_tab_behind_the_one_in_front() -> Check {
    let mut model = window("chat", &[Chat, Code]);
    open(&mut model, Code);
    let code = pane(&model, Code)?;
    let before = drawn(&model)?;
    tell(&mut model, "a", &[Chat, Code, Changes]);
    assert_eq!(drawn(&model)?, before, "what is drawn did not move");
    assert_eq!(tabs(&model, code)?, [Code, Changes]);
    let [changes] = of(&model, Changes)[..] else {
        return Err("one Changes pane".into());
    };
    assert!(
        changes.seated && changes.unseen,
        "the window's, not yet seen"
    );
    tell(&mut model, "a", &[Chat, Code, Changes, Files]);
    assert_eq!(of(&model, Changes).len(), 1, "told again, none is added");
    assert_eq!(of(&model, Files).len(), 0, "Files is no rule's");
    Ok(())
}

/// With no pane of its place open, the place opens beside the chat and the pane is drawn:
/// there is no dot on a pane in plain view.
#[test]
fn the_first_pane_of_a_place_opens_that_place() -> Check {
    let mut model = window("chat", &[Chat]);
    assert_eq!(drawn(&model)?.len(), 1);
    tell(&mut model, "a", &[Chat, Changes]);
    let changes = pane(&model, Changes)?;
    let placed = drawn(&model)?;
    assert_eq!(placed.len(), 2, "{placed:?}");
    assert!(placed.iter().any(|(pane, _)| *pane == changes));
    assert!(of(&model, Changes).iter().all(|pane| !pane.unseen));
    Ok(())
}

/// An untouched tab goes when its kind has nothing left, and is back, new again, when it has.
/// A tab the person brought in front is theirs: it has no dot and stays when its thing ends.
#[test]
fn an_untouched_tab_goes_when_its_thing_ends_and_a_touched_one_stays() -> Check {
    let mut model = window("chat", &[Chat, Code]);
    open(&mut model, Code);
    let code = pane(&model, Code)?;
    tell(&mut model, "a", &[Chat, Code, Changes]);
    tell(&mut model, "a", &[Chat, Code]);
    assert_eq!(tabs(&model, code)?, [Code], "nothing to show, no tab");
    tell(&mut model, "a", &[Chat, Code, Changes]);
    assert_eq!(tabs(&model, code)?, [Code, Changes]);
    let unseen = |model: &Model| of(model, Changes).iter().any(|pane| pane.unseen);
    assert_eq!(of(&model, Changes).len(), 1, "the same pane");
    assert!(unseen(&model), "and new again");
    let changes = pane(&model, Changes)?;
    change(&mut model, GridMsg::Front(changes));
    change(&mut model, GridMsg::Front(code));
    assert!(!unseen(&model), "seen, it carries no dot behind");
    tell(&mut model, "a", &[Chat, Code]);
    assert_eq!(tabs(&model, code)?, [Code, Changes], "touched, it stays");
    Ok(())
}

/// A tab the person closed stays closed while its kind goes on having something. The
/// person's own pane of the kind, or a seat a layout gave it, is enough: none is added.
#[test]
fn a_closed_tab_stays_closed_and_a_pane_already_here_is_enough() -> Check {
    let mut model = window("chat", &[Chat, Changes]);
    let changes = pane(&model, Changes)?;
    change(&mut model, GridMsg::Close(changes));
    tell(&mut model, "a", &[Chat, Changes, Code]);
    assert_eq!(of(&model, Changes).len(), 0, "closed, it stays closed");
    let mut model = window("chat", &[Chat]);
    open(&mut model, Web);
    tell(&mut model, "a", &[Chat, Web]);
    assert_eq!(of(&model, Web).len(), 1, "the person's own is enough");
    let mut model = window("review", &[Chat]);
    tell(&mut model, "a", &[Chat, Changes]);
    assert_eq!(of(&model, Changes).len(), 1, "the layout's seat is enough");
    assert_eq!(drawn(&model)?.len(), 2, "and it takes that seat");
    Ok(())
}

/// A pane that is out docks back beside the neighbour it remembers, and that neighbour may
/// have come back as a quiet tab behind another: the pane docks beside the slot it is in.
#[test]
fn a_pane_docks_beside_the_slot_its_old_neighbour_came_back_into() -> Check {
    let mut model = window("everything", &[Chat, Changes, Web]);
    let web = pane(&model, Web)?;
    change(&mut model, GridMsg::Leave(web));
    let (window, saved) = (MAIN, "chat");
    change(&mut model, GridMsg::Show { window, saved });
    tell(&mut model, "a", &[Chat, Web]);
    open(&mut model, Code);
    tell(&mut model, "a", &[Chat, Web, Code, Changes]);
    let (code, chat) = (pane(&model, Code)?, pane(&model, Chat)?);
    assert_eq!(
        tabs(&model, code)?,
        [Code, Changes],
        "Changes is behind Code"
    );
    let before = drawn(&model)?;
    change(&mut model, GridMsg::Rejoin(web));
    let after = drawn(&model)?;
    let of = |placed: &[(PaneId, _)], pane| crate::grid::place::probe::find(placed, pane);
    assert_eq!(
        of(&after, chat)?,
        of(&before, chat)?,
        "the chat kept its room"
    );
    let (old, half, docked) = (of(&before, code)?, of(&after, code)?, of(&after, web)?);
    assert!(
        (half.w + docked.w - old.w).abs() < 0.5,
        "{half:?} {docked:?}"
    );
    Ok(())
}

/// A pane out in a window of its own has that window to itself: nothing opens beside it.
#[test]
fn nothing_opens_in_a_window_that_is_not_the_sessions_home() -> Check {
    let mut model = window("chat", &[Chat]);
    open(&mut model, Code);
    let code = pane(&model, Code)?;
    change(&mut model, GridMsg::Leave(code));
    let window = model.grid.window_of(code).ok_or("it is in no window")?;
    assert_ne!(window, MAIN);
    let (kinds, thread) = (vec![Chat, Code, Changes], Some(ThreadId::new("a")));
    let told = GridMsg::Have {
        window,
        kinds,
        thread,
    };
    change(&mut model, told);
    assert_eq!(of(&model, Changes).len(), 0, "{:?}", model.grid.panes);
    assert_eq!(
        model
            .grid
            .place(window, crate::grid::place::probe::FRAME_1)
            .len(),
        1
    );
    Ok(())
}

/// Just chat is a page to read: nothing opens beside it. Another session with something of
/// the kind uses the window's one pane of it.
#[test]
fn just_chat_opens_nothing_and_sessions_share_the_windows_pane() -> Check {
    let mut model = window("just-chat", &[Chat]);
    tell(&mut model, "a", &[Chat, Changes, Agents, Web]);
    assert_eq!(model.grid.panes.len(), 1, "{:?}", model.grid.panes);
    let mut model = window("chat", &[Chat, Agents]);
    tell(&mut model, "b", &[Chat]);
    assert_eq!(drawn(&model)?.len(), 1, "b has no agents");
    tell(&mut model, "b", &[Chat, Agents]);
    assert_eq!(of(&model, Agents).len(), 1, "one pane for both sessions");
    assert_eq!(drawn(&model)?.len(), 2);
    Ok(())
}
