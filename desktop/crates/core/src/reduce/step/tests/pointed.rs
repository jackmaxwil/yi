//! The model's `show`: where the file lands and what the model is told.

use serde_json::{Value, json};

use super::{Check, address, feed, run, sent};
use crate::address::uri::Address;
use crate::feed::{SourceCmd, TERMINAL};
use crate::grid::step::GridMsg;
use crate::grid::windows::Frame;
use crate::ids::{PaneId, WindowId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::pane::PaneKind;
use crate::reduce::pointed::{IDLE, target};
use crate::reduce::step::commands::PIN;

const THREAD: &str = "pointing";
const MAIN: WindowId = WindowId(0);

/// A window in the layout `saved` showing the thread, the person idle since time zero.
fn window(saved: &'static str) -> Model {
    let mut model = Model::default();
    let window = MAIN;
    step(&mut model, Msg::Grid(GridMsg::Show { window, saved }));
    let thread = Some(crate::ids::ThreadId::new(THREAD));
    let (kinds, told) = (vec![PaneKind::Code], Msg::Grid);
    step(
        &mut model,
        told(GridMsg::Have {
            window,
            kinds,
            thread,
        }),
    );
    model.acted = false;
    model
}

/// Asks to show `address` at time `at`; gives the one answer sent back.
fn ask(model: &mut Model, at: u64, address: &str, strength: &str) -> Result<Value, String> {
    let params = json!({"sessionId": THREAD, "address": address, "cwd": "/work/app",
        "strength": strength});
    let frame = json!({"jsonrpc": "2.0", "id": "show_pointing_1", "method": "_yi/show",
        "params": params});
    let frames = sent(&feed(model, at, &frame)).map_err(|error| error.to_string())?;
    let [answer] = frames.as_slice() else {
        return Err(format!("{} frames answered one ask", frames.len()));
    };
    if answer["id"] != "show_pointing_1" {
        return Err(format!("answered under another id: {answer}"));
    }
    Ok(answer["result"].clone())
}

fn about(model: &Model, path: &str) -> Result<PaneId, String> {
    let file = Address::new(Kind::File, path).map_err(|error| error.to_string())?;
    let is = |pane: &&crate::grid::windows::Pane| {
        pane.kind == PaneKind::Code && pane.thing.as_ref() == Some(&file)
    };
    let mut found = model.grid.panes.iter().filter(is).map(|pane| pane.id);
    match (found.next(), found.next()) {
        (Some(pane), None) => Ok(pane),
        (None, _) => Err(format!("no pane is about {path}")),
        (Some(_), Some(_)) => Err(format!("two panes are about {path}")),
    }
}

fn drawn(model: &Model, pane: PaneId) -> bool {
    model.grid.window_of(pane) == Some(MAIN)
}

fn dotted(model: &Model, pane: PaneId) -> bool {
    let mut panes = model.grid.panes.iter();
    panes.any(|other| other.id == pane && other.unseen)
}

/// The code pane the layout drew: the one a quiet show must leave in front.
fn viewer(model: &Model) -> Result<PaneId, String> {
    let code = |pane: &&crate::grid::windows::Pane| pane.kind == PaneKind::Code;
    let viewer = model.grid.panes.iter().find(code).map(|pane| pane.id);
    viewer
        .filter(|pane| drawn(model, *pane))
        .ok_or_else(|| "no viewer is drawn".to_owned())
}

#[test]
fn front_comes_forward_for_a_person_who_is_idle_and_waits_as_a_tab_for_one_who_is_not() -> Check {
    let mut model = window("code");
    let viewer = viewer(&model)?;
    let said = ask(&mut model, IDLE.0, "/work/app/src/a.rs:12", "front")?;
    assert_eq!(
        said,
        json!({"outcome": "front"}),
        "idle for exactly the wait"
    );
    let a = about(&model, "src/a.rs")?;
    assert!(
        drawn(&model, a) && !drawn(&model, viewer),
        "it took the viewer's tab"
    );
    assert_eq!(
        model.reveals,
        [(a, 12)],
        "its line waits for the file to be read"
    );

    // The person brings their viewer back in front: an act, dated by the wire's next word.
    step(&mut model, Msg::Grid(GridMsg::Front(viewer)));
    let busy = ask(&mut model, 50_000, "src/b.rs", "front")?;
    let note = "The user is busy; it waits as a tab.";
    assert_eq!(busy, json!({"outcome": "tab", "note": note}));
    let b = about(&model, "src/b.rs")?;
    assert!(
        drawn(&model, viewer) && !drawn(&model, b),
        "what they had in front stays"
    );
    assert!(dotted(&model, b), "the tab says it has not been seen");

    let early = ask(&mut model, 50_000 + IDLE.0 - 1, "src/b.rs", "front")?;
    assert_eq!(early["outcome"], "tab", "a millisecond short of the wait");
    let late = ask(&mut model, 50_000 + IDLE.0, "src/b.rs", "front")?;
    assert_eq!(late, json!({"outcome": "front"}));
    let b = about(&model, "src/b.rs")?;
    assert!(
        drawn(&model, b) && !dotted(&model, b),
        "one pane, now in front and seen"
    );
    Ok(())
}

#[test]
fn a_tab_or_a_peek_never_comes_forward_however_idle_the_person_is() -> Check {
    for strength in ["tab", "peek"] {
        let mut model = window("code");
        let viewer = viewer(&model)?;
        let before = model.grid.panes.len();
        let said = ask(&mut model, 60_000, "yi://file/src/a.rs#L40", strength)?;
        assert_eq!(said, json!({"outcome": "tab"}), "{strength}");
        let a = about(&model, "src/a.rs")?;
        assert!(drawn(&model, viewer) && !drawn(&model, a) && dotted(&model, a));
        assert_eq!(model.reveals, [(a, 40)]);
        ask(&mut model, 61_000, "src/a.rs", strength)?;
        assert_eq!(model.grid.panes.len(), before + 1, "a thing is shown once");
    }
    Ok(())
}

#[test]
fn a_pane_with_no_tabs_to_join_takes_a_slot_and_says_it_is_in_front() -> Check {
    let mut model = window("chat");
    let said = ask(&mut model, 60_000, "src/a.rs", "tab")?;
    assert_eq!(
        said,
        json!({"outcome": "front"}),
        "it is drawn, so the model is told so"
    );
    assert!(drawn(&model, about(&model, "src/a.rs")?));
    Ok(())
}

#[test]
fn what_cannot_be_shown_is_refused_in_words_and_opens_nothing() -> Check {
    let mut model = window("code");
    let before = model.grid.clone();
    let agent = ask(&mut model, 60_000, "yi://agent/reviewer", "front")?;
    let not_file = "The app shows files; this address is not one.";
    assert_eq!(agent, json!({"outcome": "refused", "note": not_file}));
    assert_eq!(ask(&mut model, 60_000, "  ", "tab")?["outcome"], "refused");

    assert_eq!(model.grid, before);
    // The window turns to another session: the one asked about is on no screen.
    let (window, kinds) = (MAIN, vec![PaneKind::Code]);
    let thread = Some(crate::ids::ThreadId::new("another"));
    let have = GridMsg::Have {
        window,
        kinds,
        thread,
    };
    step(&mut model, Msg::Grid(have));
    let turned = model.grid.clone();
    let off_screen = ask(&mut model, 60_000, "src/a.rs", "front")?;
    let note = "That session is not on screen.";
    assert_eq!(off_screen, json!({"outcome": "refused", "note": note}));
    assert_eq!(model.grid, turned);
    Ok(())
}

#[test]
fn what_the_person_does_makes_them_busy_and_what_the_app_does_alone_does_not() -> Check {
    let window = MAIN;
    let frame = Frame {
        x: 0.0,
        y: 0.0,
        w: 900.0,
        h: 700.0,
    };
    let told = |model: &Model| GridMsg::Have {
        window,
        kinds: vec![PaneKind::Code, PaneKind::Files],
        thread: model.grid.shows.get(&window).cloned(),
    };
    type Does = fn(&mut Model);
    let theirs: [(&str, Does); 3] = [
        ("a command", |model| {
            let Ok(thread) = address(Kind::Thread, THREAD) else {
                return;
            };
            run(model, PIN, thread, None);
        }),
        ("keys in a shell", |model| {
            step(
                model,
                Msg::Source(TERMINAL, SourceCmd::Input(vec![b'l', b's'])),
            );
        }),
        ("a tab brought in front", |model| {
            if let Ok(viewer) = viewer(model) {
                step(model, Msg::Grid(GridMsg::Front(viewer)));
            }
        }),
    ];
    for (what, does) in theirs {
        let mut model = self::window("code");
        does(&mut model);
        let said = ask(&mut model, 60_000, "src/a.rs", "front")?;
        assert_eq!(said["outcome"], "tab", "{what} is the person's");
    }
    let mut model = self::window("code");
    let have = told(&model);
    step(&mut model, Msg::Grid(have));
    step(&mut model, Msg::Grid(GridMsg::Moved { window, frame }));
    let (cols, rows) = (80, 24);
    step(
        &mut model,
        Msg::Source(TERMINAL, SourceCmd::Resize { cols, rows }),
    );
    let said = ask(&mut model, 60_000, "src/a.rs", "front")?;
    assert_eq!(
        said["outcome"], "front",
        "none of these is the person's doing"
    );
    Ok(())
}

#[test]
fn a_filled_window_and_a_pane_out_in_its_own_are_told_as_they_are() -> Check {
    let mut model = window("code");
    let kinds = vec![PaneKind::Code, PaneKind::Files];
    let thread = model.grid.shows.get(&MAIN).cloned();
    let have = GridMsg::Have {
        window: MAIN,
        kinds,
        thread,
    };
    step(&mut model, Msg::Grid(have));
    let other = |pane: &&crate::grid::windows::Pane| pane.kind != PaneKind::Code;
    let mut others = model.grid.panes.iter().filter(other).map(|pane| pane.id);
    let chat = others.find(|pane| drawn(&model, *pane));
    let chat = chat.ok_or("the layout draws nothing but code")?;
    step(&mut model, Msg::Grid(GridMsg::Fill(chat)));
    model.acted = false;
    let said = ask(&mut model, 60_000, "src/a.rs", "front")?;
    let note = "Another pane fills the window; it waits as a tab.";
    assert_eq!(said, json!({"outcome": "tab", "note": note}));
    let fills = model.grid.fills(MAIN);
    assert_eq!(fills, Some(chat), "the fill is the person's to end");

    let mut model = window("code");
    ask(&mut model, 60_000, "src/a.rs", "front")?;
    let a = about(&model, "src/a.rs")?;
    step(&mut model, Msg::Grid(GridMsg::Leave(a)));
    assert_eq!(
        model.grid.origin_of(a),
        Some(MAIN),
        "out in a window of its own"
    );
    let again = ask(&mut model, 90_000, "src/a.rs", "tab")?;
    assert_eq!(
        again,
        json!({"outcome": "front"}),
        "where it is, it is seen"
    );
    assert_eq!(about(&model, "src/a.rs")?, a, "and no second pane is made");
    assert_eq!(
        model.grid.origin_of(a),
        Some(MAIN),
        "nor is it brought back"
    );
    Ok(())
}

#[test]
fn an_ask_this_app_cannot_read_is_refused_as_a_method_it_lacks() -> Check {
    let mut model = window("code");
    let frame = json!({"jsonrpc": "2.0", "id": 7, "method": "_yi/show",
        "params": {"sessionId": THREAD}});
    let frames = sent(&feed(&mut model, 1, &frame))?;
    let [answer] = frames.as_slice() else {
        return Err(format!("{} frames", frames.len()).into());
    };
    assert_eq!(answer["id"], 7);
    assert_eq!(
        answer["error"]["code"], -32601,
        "the server does not wait: {answer}"
    );
    Ok(())
}

#[test]
fn an_address_is_a_file_under_the_sessions_folder_and_maybe_a_line() {
    let at = |address: &str| target(address, "/work/app");
    let named = |path: &str, line| Some((path.to_owned(), line));
    assert_eq!(at("src/a.rs"), named("src/a.rs", None));
    assert_eq!(at("src/a.rs:12"), named("src/a.rs", Some(12)));
    assert_eq!(at("/work/app/src/a.rs:12"), named("src/a.rs", Some(12)));
    assert_eq!(
        at("/etc/hosts"),
        named("/etc/hosts", None),
        "outside it, as written"
    );
    assert_eq!(
        at("/work/application/a.rs"),
        named("/work/application/a.rs", None)
    );
    assert_eq!(at("yi://file/src/a.rs#L7"), named("src/a.rs", Some(7)));
    assert_eq!(at("yi://lines/src/a.rs#L7-L9"), named("src/a.rs", Some(7)));
    assert_eq!(at("yi://file/src/a.rs"), named("src/a.rs", None));
    assert_eq!(at("src/día.rs:3"), named("src/día.rs", Some(3)));
    assert_eq!(at("src/a.rs:twelve"), named("src/a.rs:twelve", None));
    assert_eq!(at("yi://thread/abc"), None);
    assert_eq!(
        at(":12"),
        named(":12", None),
        "a colon with no path before it is a name"
    );
    assert_eq!(at(""), None);
}

#[test]
fn a_pane_waits_on_one_line_and_the_oldest_wait_is_let_go_at_the_cap() {
    let mut model = Model::default();
    model.reveal(PaneId(1), 5);
    model.reveal(PaneId(1), 9);
    assert_eq!(model.reveals, [(PaneId(1), 9)], "the line last pointed at");
    for pane in 2..=9 {
        model.reveal(PaneId(pane), 1);
    }
    let kept: Vec<u32> = model.reveals.iter().map(|(pane, _)| pane.0).collect();
    assert_eq!(
        kept,
        [2, 3, 4, 5, 6, 7, 8, 9],
        "eight at most, the oldest gone"
    );
}
