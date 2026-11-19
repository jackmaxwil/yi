//! What a person using the browser bar would notice if it broke, drawn in process.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::command::tools::web;
use yd_core::feed::{FeedEvent, SourceCmd};
use yd_core::ids::WindowId;
use yd_core::msg::{Effect, Msg, step};
use yd_core::row::Row;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::{DEF, FEED};

type Check = Result<(), Box<dyn Error>>;
type Sent = Rc<RefCell<Vec<Effect>>>;

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    (DEF.view)(&pane, cx)
}

fn draw(setup: fn(&mut Hub)) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    let sent: Sent = Rc::default();
    let into = sent.clone();
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.set_sink(move |effect| into.borrow_mut().push(effect));
        setup(&mut hub);
    }
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (420.0, 300.0),
        Env::new(true, 1.0),
    );
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok((rt, sent))
}

fn tap(rt: &mut Runtime<Win>, key: &str) -> Check {
    let all = rt.elements();
    let hit = all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key));
    let [x, y, w, h] = hit.ok_or(format!("no {key}"))?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..3 {
        rt.frame(0.016);
    }
    Ok(())
}

fn press(rt: &mut Runtime<Win>, key: Key, cmd: bool) {
    let mods = Mods {
        cmd,
        logo: cmd,
        ..Mods::default()
    };
    rt.key(KeyEvent { key, mods });
    rt.frame(0.016);
}

fn type_address(rt: &mut Runtime<Win>, words: &str) -> Check {
    tap(rt, "web-0.address.input")?;
    press(rt, Key::Command('a'), true);
    press(rt, Key::Text(words.to_owned()), false);
    press(rt, Key::Enter, false);
    rt.frame(0.016);
    Ok(())
}

fn words(rt: &Runtime<Win>, key: &str) -> Option<String> {
    let all = rt.elements();
    let rect = all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))?
        .rect;
    let inside = |r: [f32; 4]| r[1] >= rect[1] && r[1] + r[3] <= rect[1] + rect[3] + 0.5;
    let said: Vec<String> = all
        .iter()
        .filter(|e| !e.exiting && inside(e.rect))
        .filter_map(|e| e.text.clone())
        .collect();
    Some(said.join(" "))
}

fn asked(sent: &Sent) -> Vec<SourceCmd> {
    let all = sent.borrow();
    let asks = all.iter().filter_map(|effect| match effect {
        Effect::Source(feed, ask) if *feed == FEED => Some(ask.clone()),
        _ => None,
    });
    asks.collect()
}

fn opened(sent: &Sent) -> Vec<String> {
    let asks = asked(sent).into_iter().filter_map(|ask| match ask {
        SourceCmd::Run(run) if run.command == web::OPEN => run.text,
        _ => None,
    });
    asks.collect()
}

#[test]
fn a_typed_address_that_is_not_a_web_address_is_refused_with_a_line() -> Check {
    let (mut rt, sent) = draw(|_| {})?;
    type_address(&mut rt, "javascript:alert(1)")?;
    let note = words(&rt, "web-0.note").unwrap_or_default();
    assert!(
        note.contains("not an http, https or file address"),
        "{note}"
    );
    assert_eq!(opened(&sent), Vec::<String>::new(), "nothing is opened");
    type_address(&mut rt, "localhost:5173/upload")?;
    assert_eq!(opened(&sent), ["http://localhost:5173/upload"]);
    assert_eq!(
        words(&rt, "web-0.note"),
        None,
        "the refusal goes once an address opens"
    );
    Ok(())
}

fn live(hub: &mut Hub) {
    let row = Row::new("page:0")
        .text("url", "http://localhost:5173/upload")
        .text("title", "localhost:5173")
        .text("can_back", "yes");
    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(row)));
}

#[test]
fn back_and_reload_reach_the_browser_source() -> Check {
    let (mut rt, sent) = draw(live)?;
    tap(&mut rt, "web-0.back")?;
    tap(&mut rt, "web-0.reload")?;
    let runs: Vec<_> = asked(&sent)
        .into_iter()
        .filter_map(|ask| match ask {
            SourceCmd::Run(run) => Some(run.command),
            _ => None,
        })
        .collect();
    assert_eq!(runs, [web::BACK, web::RELOAD]);
    Ok(())
}

#[test]
fn the_page_is_told_the_size_of_its_box_once() -> Check {
    let (mut rt, sent) = draw(live)?;
    for _ in 0..4 {
        rt.frame(0.016);
    }
    let sizes: Vec<_> = asked(&sent)
        .into_iter()
        .filter(|ask| matches!(ask, SourceCmd::Resize { .. }))
        .collect();
    let all = rt.elements();
    let page = all
        .iter()
        .find(|e| e.key.as_deref() == Some("web-0.page"))
        .ok_or("no page")?;
    let [_, _, w, h] = page.rect;
    let told = SourceCmd::Resize {
        cols: w.round() as u16,
        rows: h.round() as u16,
    };
    assert_eq!(sizes, [told]);
    Ok(())
}
