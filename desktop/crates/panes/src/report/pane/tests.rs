//! What a person reading the agent's page would notice if it broke.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Runtime, col};
use yd_core::command::tools::web;
use yd_core::feed::SourceCmd;
use yd_core::ids::WindowId;
use yd_core::msg::Effect;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::{DEF, FEED, address, newest, scenes, size, zoom};

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

fn draw(setup: fn(&mut Hub), width: f32) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
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
        (width, 300.0),
        Env::new(true, 1.0),
    );
    for _ in 0..5 {
        rt.frame(0.016);
    }
    Ok((rt, sent))
}

fn asked(sent: &Sent) -> Vec<SourceCmd> {
    let all = sent.borrow();
    let asks = all.iter().filter_map(|effect| match effect {
        Effect::Source(feed, ask) if *feed == FEED => Some(ask.clone()),
        _ => None,
    });
    asks.collect()
}

#[test]
fn the_newest_html_the_thread_wrote_is_its_page_with_its_turn() -> Check {
    let (rt, _) = draw(scenes::SCENES[1].setup, 252.0)?;
    let hub = rt.state.hub.try_borrow()?;
    let thread = hub.model.wire.threads.values().next().ok_or("no thread")?;
    let written = newest(thread).ok_or("no page")?;
    assert_eq!(written.path, "reports/upload-limit-simulated.html");
    assert_eq!(written.turn, Some(2));
    assert_eq!(written.bytes.map(size).as_deref(), Some("3.1 KB"));
    Ok(())
}

#[test]
fn the_page_opens_on_the_second_browser_page_laid_out_600_wide() -> Check {
    let (_rt, sent) = draw(scenes::SCENES[1].setup, 300.0)?;
    let asks = asked(&sent);
    let opened: Vec<_> = asks
        .iter()
        .filter_map(|ask| match ask {
            SourceCmd::Run(run) if run.command == web::OPEN => run.text.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(
        opened,
        ["file:///Users/me/atlas-api/reports/upload-limit-simulated.html"]
    );
    let wide = asks
        .iter()
        .any(|ask| matches!(ask, SourceCmd::Resize { cols: 600, .. }));
    assert!(wide, "the page is laid out at its design width: {asks:?}");
    Ok(())
}

#[test]
fn a_thread_that_wrote_no_page_says_so() -> Check {
    let (rt, sent) = draw(|_| {}, 300.0)?;
    let all = rt.elements();
    let empty = all
        .iter()
        .any(|e| e.key.as_deref() == Some("report-0.state"));
    assert!(empty, "the empty state shows");
    assert!(asked(&sent).is_empty(), "nothing is opened");
    Ok(())
}

#[test]
fn the_page_scales_with_its_room_within_bounds() -> Check {
    assert_eq!(zoom(252.0), 0.42);
    assert_eq!(zoom(60.0), 0.2);
    assert_eq!(zoom(1_200.0), 1.1);
    let odd = address("/w", "out/a b#1.html");
    assert_eq!(odd, "file:///w/out/a%20b%231.html");
    Ok(())
}
