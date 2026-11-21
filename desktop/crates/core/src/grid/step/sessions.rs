//! Panes belong to a session: one the person opens shows in the sessions they opened it in,
//! the window keeps its lines, and a kind about this Mac shows in every session.

use crate::grid::layout::Track;
use crate::grid::place::probe::{Check, find};
use crate::grid::windows::{MAX_SESSIONS, Scope, Windows};
use crate::ids::ThreadId;
use crate::model::Model;
use crate::pane::PaneKind::{self, Chat, Containers, Trace, Web};

use super::GridMsg;
use super::tests::{FRAME_2, MAIN, change, drawn, pane, record};

/// The chat alone, in a window showing session `thread`.
fn window(thread: &str) -> Model {
    let mut model = record();
    let (window, saved) = (MAIN, "chat");
    change(&mut model, GridMsg::Show { window, saved });
    show(&mut model, thread);
    model
}

/// The window turns to session `thread`; an empty name is New chat, no session at all.
fn show(model: &mut Model, thread: &str) {
    let thread = (!thread.is_empty()).then(|| ThreadId::new(thread));
    let (window, kinds) = (MAIN, PaneKind::ALL.to_vec());
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

fn kinds(model: &Model) -> Result<Vec<PaneKind>, String> {
    let placed = drawn(model)?;
    let kind = |(pane, _): &(_, _)| model.grid.kind(*pane).ok_or("a pane of no kind");
    let mut kinds = placed.iter().map(kind).collect::<Result<Vec<_>, _>>()?;
    kinds.sort();
    Ok(kinds)
}

fn tracks(model: &Model) -> Option<(Vec<Track>, Vec<Track>)> {
    let layout = &model.grid.window(MAIN)?.layout;
    Some((layout.cols.clone(), layout.rows.clone()))
}

/// A pane opened in one session is out in the next and back, where it was, on the way
/// back; the window's tracks are the same in both, so no line moved.
#[test]
fn a_pane_shows_in_the_session_it_was_opened_in_and_the_lines_stay() -> Check {
    let mut model = window("a");
    open(&mut model, Trace);
    assert_eq!(kinds(&model)?, [Chat, Trace]);
    let (lines, was) = (tracks(&model), drawn(&model)?);
    show(&mut model, "b");
    assert_eq!(kinds(&model)?, [Chat], "session b never opened it");
    assert_eq!(tracks(&model), lines, "the lines stay with the window");
    show(&mut model, "");
    assert_eq!(kinds(&model)?, [Chat], "nor did New chat");
    show(&mut model, "a");
    assert_eq!(drawn(&model)?, was, "back as it was");
    Ok(())
}

/// A second session that asks for the same kind gets the same pane in the same slot, open
/// in both from then on, and no second pane.
#[test]
fn a_second_session_opens_the_same_pane_in_its_own_slot() -> Check {
    let mut model = window("a");
    open(&mut model, Web);
    let (web, was) = (pane(&model, Web)?, drawn(&model)?);
    show(&mut model, "b");
    open(&mut model, Web);
    assert_eq!(find(&drawn(&model)?, web)?, find(&was, web)?);
    assert_eq!(model.grid.panes.len(), 2, "no second web pane was made");
    show(&mut model, "a");
    assert_eq!(kinds(&model)?, [Chat, Web]);
    show(&mut model, "c");
    assert_eq!(kinds(&model)?, [Chat]);
    show(&mut model, "b");
    assert_eq!(kinds(&model)?, [Chat, Web], "b has it from when it asked");
    Ok(())
}

/// Closed in one session, a pane open in another stays there with its slot; closed in the
/// last session it is open in, it is gone for good.
#[test]
fn closing_a_pane_closes_it_in_this_session_only_while_another_has_it() -> Check {
    let mut model = window("a");
    open(&mut model, Web);
    show(&mut model, "b");
    open(&mut model, Web);
    let web = pane(&model, Web)?;
    change(&mut model, GridMsg::Close(web));
    assert_eq!(kinds(&model)?, [Chat], "closed in b");
    show(&mut model, "a");
    assert_eq!(kinds(&model)?, [Chat, Web], "a still has it");
    change(&mut model, GridMsg::Close(web));
    assert_eq!(
        model.grid.kind(web),
        None,
        "its last session closed it for good"
    );
    show(&mut model, "b");
    assert_eq!(kinds(&model)?, [Chat]);
    Ok(())
}

/// A kind about this Mac shows in every session, and any pane can be told to: kept across
/// sessions it stays through a switch, and told back it is this session's alone.
#[test]
fn a_mac_wide_kind_and_a_pane_kept_across_sessions_stay_through_a_switch() -> Check {
    let mut model = window("a");
    open(&mut model, Containers);
    open(&mut model, Trace);
    show(&mut model, "b");
    assert_eq!(kinds(&model)?, [Chat, Containers]);
    show(&mut model, "a");
    let trace = pane(&model, Trace)?;
    let (pane, on) = (trace, true);
    change(&mut model, GridMsg::Across { pane, on });
    show(&mut model, "b");
    assert_eq!(kinds(&model)?, [Chat, Trace, Containers]);
    let on = false;
    change(&mut model, GridMsg::Across { pane, on });
    show(&mut model, "a");
    assert_eq!(kinds(&model)?, [Chat, Containers], "it is session b's now");
    Ok(())
}

/// A file from before panes had sessions reads with every pane in every session; one this
/// build wrote reads back as it was; and a pane forgets its oldest session past the most.
#[test]
fn the_grid_file_keeps_each_panes_sessions() -> Check {
    let old: Windows = serde_json::from_str(FRAME_2)?;
    assert!(old.panes.iter().all(|pane| pane.scope == Scope::Window));
    let mut model = window("a");
    open(&mut model, Trace);
    for n in 0..MAX_SESSIONS {
        show(&mut model, &format!("s{n}"));
        open(&mut model, Trace);
    }
    let trace = model.grid.panes.iter().find(|pane| pane.kind == Trace);
    let Some(Scope::Sessions(list)) = trace.map(|pane| &pane.scope) else {
        return Err("the trace pane is no session's".into());
    };
    assert_eq!(list.len(), MAX_SESSIONS);
    assert!(
        !list.contains(&ThreadId::new("a")),
        "the oldest was forgotten"
    );
    let text = serde_json::to_string(&model.grid)?;
    let mut read: Windows = serde_json::from_str(&text)?;
    (read.have, read.shows) = (model.grid.have.clone(), model.grid.shows.clone());
    assert_eq!(read, model.grid);
    Ok(())
}

/// The window's own chat shows in every session, asked for or not; a second chat split
/// from it is the session's it was split in.
#[test]
fn the_windows_chat_stays_and_a_split_chat_is_its_sessions() -> Check {
    let mut model = window("a");
    open(&mut model, Chat);
    let first = pane(&model, Chat)?;
    change(&mut model, GridMsg::Split(first));
    assert_eq!(kinds(&model)?, [Chat, Chat]);
    show(&mut model, "b");
    let placed = drawn(&model)?;
    assert_eq!(placed.len(), 1, "the split chat is session a's: {placed:?}");
    assert_eq!(
        find(&placed, first).map(drop),
        Ok(()),
        "the first chat stays"
    );
    show(&mut model, "a");
    assert_eq!(kinds(&model)?, [Chat, Chat]);
    Ok(())
}
