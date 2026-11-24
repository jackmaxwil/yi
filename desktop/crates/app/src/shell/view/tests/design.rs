//! Design mode joined to the running window: the pane shown by its command, a pick and a
//! handle drag carried to the page's source, a scripted page's answers drawn in the pane.

use std::sync::mpsc::channel;

use yd_core::command::tools::design::START;
use yd_core::command::tools::web;
use yd_core::feed::{FeedEvent, SourceCmd, WEB};
use yd_core::ids::FeedId;
use yd_core::inbox::inbox;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_sources::browser::{self, Missing, Page};
use yd_sources::cdp::{Element, Scripted};
use yd_ui::grid_view::picker;
use yd_ui::menu::command_box;
use yd_ui::win::dispatch;

use super::*;

/// What a window asked of its sources, in order.
type Asked = Rc<RefCell<Vec<(FeedId, SourceCmd)>>>;

fn asked(runtime: &Runtime<Win>) -> Result<Asked, Box<dyn Error>> {
    let asks = Asked::default();
    let log = Rc::clone(&asks);
    runtime.state.hub.try_borrow_mut()?.set_sink(move |effect| {
        if let Effect::Source(feed, ask) = effect {
            log.borrow_mut().push((feed, ask));
        }
    });
    Ok(asks)
}

fn run(runtime: &mut Runtime<Win>, run: Run) {
    runtime.update(move |win, tx| {
        command_box::run(win, tx, run.clone());
    });
    rest(runtime);
}

fn key_of(runtime: &Runtime<Win>, kind: PaneKind) -> Result<String, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let window = grid.window(root::MAIN).ok_or("no window")?;
    let mut docked = window.layout.docked().map(|(pane, _)| pane);
    let pane = docked.find(|pane| grid.kind(*pane) == Some(kind));
    let pane = pane.ok_or_else(|| format!("the window draws no {kind:?} pane"))?;
    Ok(format!("{}-{}", kind.slug(), pane.0))
}

/// The words drawn by the descendants of the element keyed `key`, left to right.
fn words(runtime: &Runtime<Win>, key: &str) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    by_key(&shown, key).ok_or_else(|| format!("no {key}"))?;
    let under = format!("/{key}/");
    let live = shown
        .iter()
        .filter(|e| !e.exiting && e.path.contains(&under));
    let mut said: Vec<(f32, String)> = live
        .filter_map(|e| Some((e.rect[0], e.text.clone()?)))
        .collect();
    said.sort_by(|a, b| a.0.total_cmp(&b.0));
    let said: Vec<String> = said.into_iter().map(|(_, text)| text).collect();
    Ok(said.join(" "))
}

/// The mockup's alert before any change (record 4.9.1), as the page that answers.
fn alert() -> Scripted {
    Scripted::of(Element {
        name: "div.alert".to_owned(),
        source: "src/pages/Upload.tsx:41".to_owned(),
        at: (16.0, 40.0),
        size: (602.0, 30.0),
        padding: (7.0, 10.0),
        fill: "rgb(253, 236, 236)".to_owned(),
        ink: "rgb(156, 36, 36)".to_owned(),
        radius: 6.0,
        says: "Too many uploads. Try again in 41 seconds.".to_owned(),
        gap: 10.0,
        inset: 16.0,
    })
}

/// Hands every ask the window has made of the page to the browser source over a fresh page
/// `made`, and folds what the source sends into the window's model.
fn answered<P: Page>(runtime: &mut Runtime<Win>, asks: &Asked, made: fn() -> P) -> Check {
    let (tx, rx) = channel();
    let mut open = Run::new(web::OPEN);
    open.text = Some("localhost:5173/upload".to_owned());
    tx.send(SourceCmd::Run(open))?;
    for (feed, ask) in asks.borrow().iter() {
        if *feed == WEB {
            tx.send(ask.clone())?;
        }
    }
    drop(tx);
    let (sender, inbox) = inbox(|| {});
    let launch = |url: &str, _: (u32, u32), _: u32| {
        let mut page = made();
        page.navigate(url);
        Ok::<_, Missing>(page)
    };
    browser::serve(launch, |_| Ok(()), sender.feed(WEB), rx)?;
    runtime.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            inbox.drain(&mut hub.model);
        }
    });
    rest(runtime);
    Ok(())
}

/// Opens the web pane's page, which has sent one frame of 572 by 368 pixels; its pane's key.
fn opened(runtime: &mut Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let web = key_of(runtime, PaneKind::Web)?;
    let page = Row::new("page:0").text("url", "http://localhost:5173/upload");
    runtime.update(move |win, _| dispatch(win, Msg::Feed(WEB, FeedEvent::Upsert(page.clone()))));
    rest(runtime);
    {
        let hub = runtime.state.hub.try_borrow()?;
        let panes = hub.model.grid.panes.iter();
        let id = panes
            .filter(|pane| pane.kind == PaneKind::Web)
            .map(|pane| pane.id);
        let state = id.filter_map(|id| hub.panes.get::<yd_panes::web::pane::State>(Some(id)));
        let screen = state.map(|state| state.screen.clone()).next();
        let screen = screen.ok_or("the pane keeps no screen")?;
        screen.push(572, 368, vec![255; 572 * 368 * 4])?;
    }
    rest(runtime);
    Ok(web)
}

#[test]
fn a_handle_drag_changes_the_size_the_page_reports_and_the_size_row() -> Check {
    let mut runtime = unframed("shell/everything")?;
    rest(&mut runtime);
    let asks = asked(&runtime)?;
    let show = picker::show(PaneKind::Design).ok_or("Design has no Show command")?;
    run(&mut runtime, Run::new(show));
    let design = key_of(&runtime, PaneKind::Design)?;
    assert!(
        live(&runtime, &format!("{design}.state")),
        "nothing is picked"
    );

    run(&mut runtime, Run::new(START));
    let web = opened(&mut runtime)?;
    let shown = runtime.elements();
    let [x, y, w, h] = by_key(&shown, &format!("{web}.page"))
        .ok_or("no page")?
        .rect;
    let points = (w / 572.0).min(h / 368.0);
    let at = (x + 120.5 * points, y + 50.5 * points);
    runtime.pointer_move(at.0, at.1);
    runtime.pointer_down(at.0, at.1);
    runtime.pointer_up(at.0, at.1);
    rest(&mut runtime);
    let picked = asks.borrow().iter().any(|(feed, ask)| {
        let run = match ask {
            SourceCmd::Run(run) if *feed == WEB => run,
            _ => return false,
        };
        run.command.0 == "design.at" && run.text.as_deref() == Some("120,50")
    });
    assert!(
        picked,
        "the press asks the page what is at its pixel: {:?}",
        asks.borrow()
    );

    answered(&mut runtime, &asks, alert)?;
    let header = words(&runtime, &format!("{design}.design-header"))?;
    assert_eq!(header, "div.alert src/pages/Upload.tsx:41 Pick another");
    let size = words(&runtime, &format!("{design}.row.size"))?;
    assert_eq!(size, "Size 602 × 30");
    assert!(!live(&runtime, &format!("{design}.changes")));

    let handle = format!("{web}.design.handle.s");
    let (hx, hy) = centre(&runtime, &handle)?;
    // Twelve pixels of the page down, whatever the picture is fitted to.
    let down = 12.0 * points;
    runtime.pointer_move(hx, hy);
    runtime.pointer_down(hx, hy);
    runtime.pointer_move(hx + 20.0, hy + down);
    runtime.pointer_up(hx + 20.0, hy + down);
    rest(&mut runtime);
    answered(&mut runtime, &asks, alert)?;
    let size = words(&runtime, &format!("{design}.row.size"))?;
    assert_eq!(size, "Size 602 × 42 602 × 30");
    let badge = words(&runtime, &format!("{web}.design.size"))?;
    assert_eq!(badge, "602 × 42");
    let line = words(&runtime, &format!("{design}.change.0.size"))?;
    assert_eq!(line, "size 602 × 30 → 602 × 42");
    Ok(())
}

#[test]
fn pressing_an_element_of_a_page_yis_link_cannot_ask_says_why_and_what_to_do() -> Check {
    let mut runtime = unframed("shell/everything")?;
    rest(&mut runtime);
    let asks = asked(&runtime)?;
    let show = picker::show(PaneKind::Design).ok_or("Design has no Show command")?;
    run(&mut runtime, Run::new(show));
    let design = key_of(&runtime, PaneKind::Design)?;
    run(&mut runtime, Run::new(START));
    let web = opened(&mut runtime)?;
    tap(&mut runtime, &format!("{web}.page"))?;
    answered(&mut runtime, &asks, Scripted::unlinked)?;
    let said = words(&runtime, &format!("{design}.state"))?;
    assert!(said.contains("Yi's link sends no Runtime call"), "{said}");
    assert!(
        said.contains("Attach to Chat"),
        "what to do instead: {said}"
    );
    assert!(!said.contains("Press an element"), "{said}");
    Ok(())
}
