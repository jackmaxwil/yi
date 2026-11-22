//! A layout is places, not panes: a pane it seated is out of the grid while its kind has
//! nothing to show, and takes its own slot once it has.

use crate::grid::place::probe::{Check, FRAME_1, find};
use crate::grid::saved::SAVED;
use crate::grid::windows::Windows;
use crate::ids::PaneId;
use crate::model::Model;
use crate::pane::PaneKind::{self, Changes, Chat, Files, Terminal};

use super::GridMsg;
use super::tests::{FRAME_2, MAIN, REVIEW_PLACED, change, drawn, expect, pane, record};

/// A window that was told its kinds before a layout was picked, as the running app's is.
fn told(kinds: &[PaneKind], saved: &'static str) -> Model {
    let mut model = record();
    let window = MAIN;
    change(
        &mut model,
        GridMsg::Show {
            window,
            saved: "chat",
        },
    );
    have(&mut model, kinds);
    change(&mut model, GridMsg::Show { window, saved });
    model
}

fn have(model: &mut Model, kinds: &[PaneKind]) {
    let (window, kinds, thread) = (MAIN, kinds.to_vec(), None);
    let told = GridMsg::Have {
        window,
        kinds,
        thread,
    };
    change(model, told);
}

fn kinds_drawn(model: &Model) -> Result<Vec<PaneKind>, String> {
    let placed = drawn(model)?;
    let kind = |(pane, _): &(PaneId, _)| model.grid.kind(*pane).ok_or("a pane of no kind");
    let mut kinds = placed.iter().map(kind).collect::<Result<Vec<_>, _>>()?;
    kinds.sort();
    Ok(kinds)
}

/// A session with nothing in it: every saved layout with a chat slot draws the chat alone,
/// filling the grid, and no resting pane counts as pulled out into a window.
#[test]
fn a_layout_seats_only_the_kinds_with_something_to_show() -> Check {
    for saved in SAVED {
        let model = told(&[Chat], saved.slug);
        let chat = saved.areas.iter().any(|(kind, _)| *kind == Chat);
        let kinds = kinds_drawn(&model)?;
        if chat {
            assert_eq!(kinds, [Chat], "{}", saved.slug);
        } else {
            assert_eq!(kinds.len(), 1, "{}: the grid keeps one pane in", saved.slug);
        }
        for seat in &model.grid.panes {
            assert_eq!(model.grid.origin_of(seat.id), None, "{}", saved.slug);
        }
        assert_eq!(model.grid.windows.len(), 1, "{}", saved.slug);
        let shown = model.grid.window(MAIN).and_then(|window| window.saved);
        assert_eq!(
            shown,
            Some(saved.slug),
            "resting leaves it the layout shown"
        );
    }
    Ok(())
}

/// Review with nothing but chat, then changes: Changes comes back where Review draws it
/// with Files and Terminal closed, and with every kind the grid is the record's Review.
#[test]
fn a_resting_pane_takes_its_own_slot_when_its_kind_has_something() -> Check {
    let mut model = told(&[Chat], "review");
    have(&mut model, &[Chat, Changes]);
    assert_eq!(kinds_drawn(&model)?, [Chat, Changes]);
    let mut closed = record();
    let (window, saved) = (MAIN, "review");
    change(&mut closed, GridMsg::Show { window, saved });
    for kind in [Files, Terminal] {
        let shut = pane(&closed, kind)?;
        change(&mut closed, GridMsg::Close(shut));
    }
    let (resting, without) = (drawn(&model)?, drawn(&closed)?);
    for kind in [Changes, Chat] {
        let (ours, theirs) = (pane(&model, kind)?, pane(&closed, kind)?);
        assert_eq!(find(&resting, ours)?, find(&without, theirs)?, "{kind:?}");
    }
    have(&mut model, &[Chat, Changes, Files, Terminal]);
    expect(&model, &REVIEW_PLACED)?;
    have(&mut model, &[Chat]);
    assert_eq!(
        kinds_drawn(&model)?,
        [Chat],
        "a kind that runs dry rests again"
    );
    Ok(())
}

/// A pane the person asks for shows in its own slot whatever its kind has, and is theirs
/// from then on: it does not rest when the kinds are told again.
#[test]
fn asking_for_a_resting_pane_shows_it_and_keeps_it() -> Check {
    let mut model = told(&[Chat], "review");
    let (window, kind) = (MAIN, Terminal);
    change(&mut model, GridMsg::Open { window, kind });
    assert_eq!(kinds_drawn(&model)?, [Chat, Terminal]);
    let terminal = pane(&model, Terminal)?;
    let slot = model
        .grid
        .window(MAIN)
        .and_then(|w| w.layout.slot(terminal));
    let review = crate::grid::saved::REVIEW.areas.iter();
    let seat = review.clone().find(|(kind, _)| *kind == Terminal);
    assert_eq!(
        slot,
        seat.map(|(_, at)| *at),
        "its Review slot, not half a pane"
    );
    have(&mut model, &[Chat]);
    assert_eq!(kinds_drawn(&model)?, [Chat, Terminal]);
    assert_eq!(
        model.grid.panes.len(),
        review.len(),
        "no second terminal was made"
    );
    Ok(())
}

/// A layout kept while panes rest keeps their places, and the grid's file keeps who rests.
#[test]
fn a_kept_layout_and_the_grid_file_keep_resting_panes() -> Check {
    let mut model = told(&[Chat], "review");
    let (window, name) = (MAIN, "Mine".to_owned());
    change(&mut model, GridMsg::Keep { window, name });
    let kept = model.grid.own.first().ok_or("no layout was kept")?;
    let mut kinds: Vec<PaneKind> = kept.shape.areas.iter().map(|(kind, _)| *kind).collect();
    kinds.sort();
    assert_eq!(kinds, [Chat, Files, Changes, Terminal]);
    let text = serde_json::to_string(&model.grid)?;
    let mut read: Windows = serde_json::from_str(&text)?;
    read.fronts = model.grid.fronts.clone();
    assert_eq!(read.place(MAIN, FRAME_1), model.grid.place(MAIN, FRAME_1));
    assert_eq!(read, model.grid);
    Ok(())
}

/// A file from before panes rested reads with every pane the person's and none resting; a
/// file whose resting pane is also drawn is refused.
#[test]
fn an_older_grid_file_rests_nothing_and_a_pane_resting_and_drawn_is_refused() -> Check {
    let grid: Windows = serde_json::from_str(FRAME_2)?;
    assert!(grid.panes.iter().all(|pane| !pane.seated));
    let holes = grid.windows.iter().flat_map(|window| &window.layout.away);
    assert!(holes.clone().all(|hole| !hole.resting));
    let good = "{\"pane\": 5}";
    let resting = FRAME_2.replace(good, "{\"pane\": 5, \"resting\": true}");
    assert_ne!(resting, FRAME_2, "the file has no {good}");
    let read = serde_json::from_str::<Windows>(&resting);
    assert!(read.is_err(), "a pane resting and drawn was read: {read:?}");
    Ok(())
}

/// A pane closed while others rest goes for good, and the rest still close up.
#[test]
fn a_pane_closes_while_others_rest() -> Check {
    let mut model = told(&[Chat, Files], "everything");
    assert_eq!(kinds_drawn(&model)?, [Chat, Files]);
    let files = pane(&model, Files)?;
    change(&mut model, GridMsg::Close(files));
    assert_eq!(kinds_drawn(&model)?, [Chat]);
    have(&mut model, &[Chat, Changes]);
    assert_eq!(kinds_drawn(&model)?, [Chat, Changes]);
    Ok(())
}
