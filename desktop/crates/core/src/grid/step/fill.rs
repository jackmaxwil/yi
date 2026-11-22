//! A pane given its window's whole grid: it is drawn alone, the grid under it is kept, and
//! the fill ends with any change that arranges the window.

use crate::grid::layout::{Edge, Point, Rect, Size};
use crate::grid::place::probe::{Check, FRAME_1};
use crate::ids::ThreadId;
use crate::pane::PaneKind::{Changes, Chat, Code, Debugger, Files, Terminal};

use super::GridMsg;
use super::tests::{MAIN, change, drawn, pane, record};
use crate::model::Model;

fn everything() -> Model {
    let mut model = record();
    let (window, saved) = (MAIN, "everything");
    change(&mut model, GridMsg::Show { window, saved });
    model
}

fn whole() -> Rect {
    let (x, y, Size { w, h }) = (0.0, 0.0, FRAME_1);
    Rect { x, y, w, h }
}

/// Filled, the pane is the one thing drawn, over the whole grid; said again, every pane is
/// back where it was. No line can be dragged and nothing can be dropped while it lasts.
#[test]
fn a_filled_pane_is_drawn_alone_and_the_grid_comes_back_as_it_was() -> Check {
    let mut model = everything();
    let (code, files) = (pane(&model, Code)?, pane(&model, Files)?);
    let before = drawn(&model)?;
    let middle = Point { x: 600.0, y: 300.0 };
    let line = (0..1200_u16).find_map(|x| {
        let at = Point {
            x: f32::from(x),
            y: 300.0,
        };
        model.grid.line_at(MAIN, FRAME_1, at).map(|_| at)
    });
    let line = line.ok_or("Everything has no line to drag")?;
    assert!(model.grid.zone_at(MAIN, FRAME_1, files, middle).is_some());
    change(&mut model, GridMsg::Fill(code));
    assert_eq!(drawn(&model)?, [(code, whole())]);
    assert!(model.grid.line_at(MAIN, FRAME_1, line).is_none(), "no line");
    assert!(model.grid.zone_at(MAIN, FRAME_1, files, middle).is_none());
    change(&mut model, GridMsg::Fill(code));
    assert_eq!(drawn(&model)?, before);
    Ok(())
}

/// A tab of the filled stack may come in front and has the grid then; said of that tab, the
/// fill ends. Filling another pane moves the fill to it.
#[test]
fn the_fill_follows_the_tab_in_front_of_its_stack() -> Check {
    let mut model = everything();
    let window = MAIN;
    change(
        &mut model,
        GridMsg::Open {
            window,
            kind: Debugger,
        },
    );
    let (changes, debugger) = (pane(&model, Changes)?, pane(&model, Debugger)?);
    assert_eq!(model.grid.tabs(MAIN, debugger), [changes, debugger]);
    change(&mut model, GridMsg::Fill(debugger));
    change(&mut model, GridMsg::Front(changes));
    assert_eq!(drawn(&model)?, [(changes, whole())], "the tab has the grid");
    let files = pane(&model, Files)?;
    change(&mut model, GridMsg::Fill(files));
    assert_eq!(drawn(&model)?, [(files, whole())], "the fill moved");
    change(&mut model, GridMsg::Fill(files));
    assert!(drawn(&model)?.len() > 1, "and ended");
    change(&mut model, GridMsg::Fill(debugger));
    change(&mut model, GridMsg::Fill(changes));
    assert!(drawn(&model)?.len() > 1, "a tab of the stack ends it too");
    Ok(())
}

/// A change that ends a fill, made of the grid it will be said to.
type End = fn(&Model) -> Result<GridMsg, String>;

/// The grid a person arranges is the one they see: opening, splitting, closing or bringing
/// a pane of another slot in front ends the fill. A telling of what the session has does not.
#[test]
fn a_change_that_arranges_the_window_ends_the_fill() -> Check {
    let window = MAIN;
    let (kinds, thread) = (vec![Chat, Code, Files, Terminal], Some(ThreadId::new("a")));
    let told = GridMsg::Have {
        window,
        kinds,
        thread,
    };
    let ends: [End; 4] = [
        |_| {
            Ok(GridMsg::Open {
                window: MAIN,
                kind: Debugger,
            })
        },
        |model| {
            Ok(GridMsg::SplitAt {
                pane: pane(model, Terminal)?,
                edge: Edge::Right,
            })
        },
        |model| Ok(GridMsg::Close(pane(model, Terminal)?)),
        |model| Ok(GridMsg::Front(pane(model, Files)?)),
    ];
    for end in ends {
        let mut model = everything();
        let code = pane(&model, Code)?;
        change(&mut model, GridMsg::Fill(code));
        change(&mut model, told.clone());
        assert_eq!(drawn(&model)?, [(code, whole())], "told, it stays");
        let end = end(&model)?;
        change(&mut model, end.clone());
        assert!(drawn(&model)?.len() > 1, "{end:?} ends it");
    }
    Ok(())
}

/// A pane filled in one session does not show in another, which draws its own grid; back in
/// the first, the pane has the grid again.
#[test]
fn a_fill_waits_while_its_pane_does_not_show() -> Check {
    let mut model = everything();
    let window = MAIN;
    let tell = |model: &mut Model, thread: &str| {
        let (kinds, thread) = (vec![Chat, Code, Files], Some(ThreadId::new(thread)));
        change(
            model,
            GridMsg::Have {
                window,
                kinds,
                thread,
            },
        );
    };
    tell(&mut model, "a");
    change(
        &mut model,
        GridMsg::Open {
            window,
            kind: Debugger,
        },
    );
    let debugger = pane(&model, Debugger)?;
    change(&mut model, GridMsg::Fill(debugger));
    assert_eq!(drawn(&model)?, [(debugger, whole())]);
    tell(&mut model, "b");
    let other = drawn(&model)?;
    assert!(other.iter().all(|(pane, _)| *pane != debugger), "{other:?}");
    assert!(other.len() > 1, "b draws its own grid");
    let line = (0..1200_u16).find_map(|x| {
        let at = Point {
            x: f32::from(x),
            y: 300.0,
        };
        model.grid.line_at(MAIN, FRAME_1, at)
    });
    assert!(line.is_some(), "and its lines can be dragged");
    tell(&mut model, "a");
    assert_eq!(drawn(&model)?, [(debugger, whole())]);
    Ok(())
}
