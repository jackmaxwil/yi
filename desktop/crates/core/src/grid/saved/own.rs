//! Layouts of the person's own: kept from a window, numbered, shown, renamed, forgotten, and
//! read back from the grid's file.

use crate::grid::layout::{ChatSide, GridError, Rect, Size};
use crate::grid::place::place;
use crate::grid::place::probe::{Check, FRAME_1, near};
use crate::grid::step::GridMsg;
use crate::grid::windows::Windows;
use crate::ids::{PaneId, WindowId};
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::pane::PaneKind;

use super::{OwnId, SAVED, Shape};

const MAIN: WindowId = WindowId(0);

fn showing(saved: &'static str) -> Model {
    let mut model = Model::default();
    step(
        &mut model,
        Msg::Grid(GridMsg::Show {
            window: MAIN,
            saved,
        }),
    );
    model
}

fn keep(model: &mut Model, name: &str) {
    let name = name.to_owned();
    step(model, Msg::Grid(GridMsg::Keep { window: MAIN, name }));
}

fn numbers(model: &Model) -> Vec<u8> {
    model.grid.own.iter().map(|own| own.id.0).collect()
}

fn kinds(model: &Model) -> Vec<Option<PaneKind>> {
    let Some(window) = model.grid.window(MAIN) else {
        return Vec::new();
    };
    let docked = window.layout.docked();
    docked.map(|(pane, _)| model.grid.kind(pane)).collect()
}

/// A layout kept from Review is shown again over Everything, marked as the one shown, and
/// the grid written to a file and read back still holds it.
#[test]
fn a_kept_layout_shows_again_and_round_trips_with_the_grid() -> Check {
    let mut model = showing("review");
    let review = model.grid.place(MAIN, FRAME_1);
    keep(&mut model, "  Reading  ");
    let kept = model.grid.own.first().ok_or("nothing kept")?;
    assert_eq!((kept.id, kept.name.as_str()), (OwnId(6), "Reading"));
    let window = model.grid.window(MAIN).ok_or("no window")?;
    assert_eq!((window.saved, window.own), (None, Some(OwnId(6))));
    step(
        &mut model,
        Msg::Grid(GridMsg::Show {
            window: MAIN,
            saved: "everything",
        }),
    );
    assert_eq!(model.grid.window(MAIN).and_then(|w| w.own), None);
    let wear = GridMsg::Wear {
        window: MAIN,
        own: OwnId(6),
    };
    step(&mut model, Msg::Grid(wear));
    assert_eq!(
        model.grid.place(MAIN, FRAME_1),
        review,
        "Review's arrangement"
    );
    assert_eq!(model.grid.window(MAIN).and_then(|w| w.own), Some(OwnId(6)));
    let text = serde_json::to_string(&model.grid)?;
    let read: Windows = serde_json::from_str(&text)?;
    assert_eq!(read, model.grid);
    Ok(())
}

/// A rearranged window is no longer the layout it was kept as.
#[test]
fn rearranging_a_window_unmarks_its_kept_layout() -> Check {
    let mut model = showing("review");
    keep(&mut model, "Mine");
    let files = model.grid.panes.iter().find(|p| p.kind == PaneKind::Files);
    let files = files.map(|pane| pane.id).ok_or("no files")?;
    step(&mut model, Msg::Grid(GridMsg::Close(files)));
    assert_eq!(model.grid.window(MAIN).and_then(|w| w.own), None);
    Ok(())
}

/// Numbers go lowest free first: 6 to 9 carry a key, the fifth gets 10, a forgotten number
/// is the next one given, and an eighth kept layout is the last.
#[test]
fn kept_layouts_take_the_lowest_free_number_up_to_eight() -> Check {
    let mut model = showing("code");
    for name in ["a", "b", "c", "d", "e"] {
        keep(&mut model, name);
    }
    assert_eq!(numbers(&model), [6, 7, 8, 9, 10]);
    step(&mut model, Msg::Grid(GridMsg::Forget(OwnId(7))));
    keep(&mut model, "f");
    assert_eq!(numbers(&model), [6, 8, 9, 10, 7]);
    for name in ["g", "h", "i"] {
        keep(&mut model, name);
    }
    assert_eq!(numbers(&model), [6, 8, 9, 10, 7, 11, 12, 13]);
    let before = model.grid.clone();
    let mut grid = model.grid.clone();
    let refused = crate::grid::step::step(
        &mut grid,
        GridMsg::Keep {
            window: MAIN,
            name: "j".into(),
        },
    );
    assert!(refused.is_empty());
    assert_eq!(grid, before, "a ninth is refused whole");
    Ok(())
}

/// Forgetting the layout a window shows keeps the window as it is and only unmarks it; a
/// rename keeps the number, and a name of nothing is refused.
#[test]
fn forgetting_the_layout_shown_keeps_the_arrangement() -> Check {
    let mut model = showing("screens");
    keep(&mut model, "Watch");
    let rename = GridMsg::Rename {
        own: OwnId(6),
        name: "Look".into(),
    };
    step(&mut model, Msg::Grid(rename));
    step(
        &mut model,
        Msg::Grid(GridMsg::Rename {
            own: OwnId(6),
            name: "   ".into(),
        }),
    );
    let names: Vec<&str> = model.grid.own.iter().map(|own| own.name.as_str()).collect();
    assert_eq!(names, ["Look"]);
    let before = kinds(&model);
    step(&mut model, Msg::Grid(GridMsg::Forget(OwnId(6))));
    assert!(model.grid.own.is_empty());
    assert_eq!(kinds(&model), before);
    assert_eq!(model.grid.window(MAIN).and_then(|w| w.own), None);
    Ok(())
}

/// A pane that is out is not part of the layout kept: the shape is what is drawn.
#[test]
fn a_kept_layout_is_the_window_as_drawn() -> Check {
    let mut model = showing("review");
    let found = model
        .grid
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Terminal);
    let terminal = found.map(|pane| pane.id).ok_or("no terminal")?;
    step(&mut model, Msg::Grid(GridMsg::Leave(terminal)));
    keep(&mut model, "Three");
    let kept = model.grid.own.first().ok_or("nothing kept")?;
    let shown: Vec<PaneKind> = kept.shape.areas.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(shown, [PaneKind::Files, PaneKind::Changes, PaneKind::Chat]);
    Ok(())
}

/// Each picture is the real arrangement: `place` at the window's size, scaled to its width,
/// for every saved layout and a kept one, either side.
#[test]
fn a_layouts_picture_is_where_place_puts_each_pane() -> Check {
    let room = Size {
        w: 1200.0,
        h: 750.0,
    };
    let mut model = showing("everything");
    keep(&mut model, "All");
    let kept = model.grid.own.first().map(|own| own.shape.clone());
    let shapes = SAVED.iter().map(|saved| saved.shape()).chain(kept);
    for shape in shapes {
        for side in [ChatSide::Left, ChatSide::Right] {
            let picture = shape.picture(room, side, 48.0);
            let layout = shape.seat((1..).map(PaneId));
            let placed = place(&layout, room, side);
            assert_eq!(picture.len(), placed.len());
            for ((kind, drawn), (pane, at)) in picture.iter().zip(&placed) {
                let index = usize::try_from(pane.0 - 1)?;
                assert_eq!(shape.areas.get(index).map(|(k, _)| *k), Some(*kind));
                let k = 48.0 / room.w;
                let want = Rect {
                    x: at.x * k,
                    y: at.y * k,
                    w: at.w * k,
                    h: at.h * k,
                };
                assert!(near(*drawn, want), "{kind:?}: {drawn:?} is not {want:?}");
            }
        }
    }
    Ok(())
}

/// A file whose kept layout does not tile, or names nobody, reads with that layout gone and
/// a window that showed it unmarked; the rest of the grid stands.
#[test]
fn a_spoilt_kept_layout_in_a_file_reads_as_gone() -> Check {
    let mut model = showing("review");
    keep(&mut model, "Fine");
    let code = GridMsg::Show {
        window: MAIN,
        saved: "code",
    };
    step(&mut model, Msg::Grid(code));
    keep(&mut model, "Spoilt");
    let mut text = serde_json::to_string(&model.grid)?;
    let spoilt = model.grid.own.get(1).ok_or("no second")?;
    let shape = serde_json::to_string(&spoilt.shape)?;
    let broken = Shape {
        areas: spoilt.shape.areas.iter().take(1).copied().collect(),
        ..spoilt.shape.clone()
    };
    text = text.replace(&shape, &serde_json::to_string(&broken)?);
    let read: Windows = serde_json::from_str(&text)?;
    let names: Vec<&str> = read.own.iter().map(|own| own.name.as_str()).collect();
    assert_eq!(names, ["Fine"]);
    assert_eq!(
        read.window(MAIN).and_then(|w| w.own),
        None,
        "it showed the spoilt one"
    );
    assert!(matches!(broken.check(), Err(GridError::NotTiled)));
    Ok(())
}
