//! The model's `show` in the real window: a file pointed at while the person is busy waits as
//! a tab with a dot, and "Show me" on its step in chat brings it in front.

use yd_core::feed::{FILES, FeedEvent};
use yd_core::ids::{Millis, PaneId};
use yd_core::msg::{Msg, step};
use yd_core::row::Row;
use yd_core::wire::event::WireEvent;
use yd_ui::grid_view::view::Grips;
use yd_ui::win::dispatch;

use super::*;

const POINTED: &str = "src/routes/sessions.rs";

fn about(runtime: &Runtime<Win>, path: &str) -> Result<Option<PaneId>, Box<dyn Error>> {
    let file = Address::new(Kind::File, path)?;
    let hub = runtime.state.hub.try_borrow()?;
    let is = |pane: &&yd_core::grid::windows::Pane| pane.thing.as_ref() == Some(&file);
    Ok(hub.model.grid.panes.iter().find(is).map(|pane| pane.id))
}

fn focus(runtime: &Runtime<Win>) -> Result<Option<PaneId>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grips = hub.panes.get::<Grips>(None);
    Ok(grips.and_then(|grips| grips.of(root::MAIN).focus))
}

/// The tab in front of the slot `pane` is a tab of.
fn front(runtime: &Runtime<Win>, pane: PaneId) -> Result<PaneId, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let tabs = grid.tabs(root::MAIN, pane);
    let front = tabs
        .iter()
        .find(|tab| grid.window_of(**tab) == Some(root::MAIN));
    Ok(*front.ok_or("its slot draws no tab")?)
}

fn ending(runtime: &Runtime<Win>, end: &str) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let key = keys(&shown).into_iter().find(|key| key.ends_with(end));
    Ok(key.ok_or(format!("nothing drawn ends {end}"))?.to_owned())
}

#[test]
fn a_file_pointed_at_while_the_person_is_busy_waits_as_a_tab_until_show_me() -> Check {
    let keys_at = focus(&settled("shell/everything")?)?;
    let mut runtime = settled("shell/pointed")?;
    let pane = about(&runtime, POINTED)?.ok_or("no pane about the file")?;
    let had = front(&runtime, pane)?;
    assert_ne!(
        had, pane,
        "what the person had in front of that slot still is"
    );
    assert_eq!(
        focus(&runtime)?,
        keys_at,
        "and the keys are where they were"
    );
    let tab = ending(&runtime, &format!(".tab-pane-{}.pane-tab", pane.0))?;
    assert_eq!(row_text(&runtime.elements(), &tab)?, "sessions.rs");
    ending(&runtime, &format!(".tab-pane-{}.pane-tab-dot", pane.0))?;
    {
        let hub = runtime.state.hub.try_borrow()?;
        let sent: Vec<&str> = hub.model.reveals.iter().map(|_| "line").collect();
        assert_eq!(sent, ["line"], "its line waits until the tab is in front");
    }

    let shown = runtime.elements();
    let step = ending(&runtime, ".step.show-1")?;
    assert_eq!(row_text(&shown, &format!("{step}.verb"))?, "Showed");
    let row = row_text(&shown, &step)?;
    assert!(
        row.contains("Show me") && row.ends_with("Added as a tab"),
        "{row}"
    );

    let show_me = ending(&runtime, ".step.show-1.show")?;
    tap(&mut runtime, &show_me)?;
    rest(&mut runtime);
    assert_eq!(front(&runtime, pane)?, pane, "in front of its slot");
    let frame = format!("code-{}.pane-frame", pane.0);
    assert!(by_key(&runtime.elements(), &frame).is_some(), "and drawn");
    assert_eq!(
        focus(&runtime)?,
        Some(pane),
        "asked for by the person, so with the keys"
    );
    let waiting = |runtime: &Runtime<Win>| -> Result<Vec<(PaneId, u32)>, Box<dyn Error>> {
        Ok(runtime.state.hub.try_borrow()?.model.reveals.clone())
    };
    let unread = waiting(&runtime)?;
    assert_eq!(
        unread,
        [(pane, 12)],
        "unread, the file cannot be scrolled yet"
    );
    runtime.update(|win, _| {
        let lines: Vec<String> = (1..=200).map(|n| format!("// line {n}")).collect();
        let text = Row::new(format!("file:{POINTED}")).text("text", lines.join("\n"));
        dispatch(win, Msg::Feed(FILES, FeedEvent::Upsert(text)));
    });
    rest(&mut runtime);
    assert_eq!(
        waiting(&runtime)?,
        [],
        "read and in front, it is at its line"
    );
    let shown = runtime.elements();
    let top = |key: &str| by_key(&shown, key).map(|found| found.rect[1]);
    let code = top(&format!("code-{}.code", pane.0)).ok_or("no code drawn")?;
    let twelfth = top(&format!("code-{}.L12", pane.0)).ok_or("line 12 is not drawn")?;
    assert!(
        (twelfth - code).abs() < 2.0,
        "line 12 at the top: {twelfth} in {code}"
    );
    // Scrolled away, a second "Show me" goes back to the line: it keeps the line each time.
    let at_top = |runtime: &Runtime<Win>| {
        let shown = runtime.elements();
        let top = |key: &str| by_key(&shown, key).map(|found| found.rect[1]);
        let code = top(&format!("code-{}.code", pane.0));
        let twelfth = top(&format!("code-{}.L12", pane.0));
        matches!((code, twelfth), (Some(code), Some(line)) if (line - code).abs() < 2.0)
    };
    runtime.update(move |_, tx| {
        if let Ok(far) = yd_ui::code::doc::LineNo::try_from(150) {
            yd_ui::code::view::reveal(tx, &format!("code-{}", pane.0), far);
        }
    });
    rest(&mut runtime);
    assert!(!at_top(&runtime), "scrolled away");
    tap(&mut runtime, &show_me)?;
    rest(&mut runtime);
    assert!(at_top(&runtime), "and back at line 12");
    let gone = format!(".tab-pane-{}.pane-tab-dot", pane.0);
    let shown = runtime.elements();
    assert!(!keys(&shown).iter().any(|key| key.ends_with(&gone)));
    Ok(())
}

/// Typing in the composer is the person at work, though it sends the model no message: a
/// `front` that comes while they type waits as a tab, and one that finds them idle does not.
#[test]
fn a_front_waits_for_a_person_typing_in_the_composer() -> Check {
    for typing in [false, true] {
        let mut runtime = settled("shell/everything")?;
        let input = ending(&runtime, ".composer.input")?;
        tap(&mut runtime, &input)?;
        // The press itself is an act; only what follows it is under test.
        runtime.update(|win, _| {
            if let Ok(mut hub) = win.hub.try_borrow_mut() {
                (hub.model.acted, hub.model.touched) = (false, Millis(0));
            }
        });
        if typing {
            let key = Key::Text("a".to_owned());
            runtime.key(KeyEvent {
                key,
                mods: Mods::default(),
            });
            rest(&mut runtime);
        }
        runtime.update(|win, _| {
            let Ok(mut hub) = win.hub.try_borrow_mut() else {
                return;
            };
            let thread = yd_ui::shown::thread(&hub, root::MAIN).map(|thread| thread.id.clone());
            let Some(thread) = thread else {
                return;
            };
            let line = format!(
                r#"{{"jsonrpc":"2.0","id":"show_2","method":"_yi/show","params":{{"sessionId":"{}","address":"{POINTED}","cwd":"","strength":"front"}}}}"#,
                thread.as_str()
            );
            step(&mut hub.model, Msg::Wire(WireEvent::Line(line)));
        });
        rest(&mut runtime);
        let pane = about(&runtime, POINTED)?.ok_or("no pane about the file")?;
        let came = front(&runtime, pane)? == pane;
        assert_eq!(came, !typing, "typing: {typing}");
    }
    Ok(())
}
