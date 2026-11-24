//! Picking and attaching in the window the app runs: every pane boxed, a pane with no finder of
//! its own attached as what it shows, which is what reaches the model.

use yd_core::address::attach::blocks;
use yd_core::feed::{FeedEvent, WEB};
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_ui::overlay::boxes::{PANE_BOX, PREVIEW};
use yd_ui::term::view::state::{Shells, Tab};
use yd_ui::win::dispatch;

use super::*;

const SERVED: &str = "Serving HTTP on port 8765";

/// Everything, with a server's line on the terminal's screen; the terminal pane's key and the
/// key of its screen.
fn served() -> Result<(Runtime<Win>, String, String), Box<dyn Error>> {
    let mut runtime = runtime("shell/mockup")?;
    let terminal = {
        let mut hub = runtime.state.hub.try_borrow_mut()?;
        let pane = hub
            .model
            .grid
            .panes
            .iter()
            .find(|p| p.kind == PaneKind::Terminal);
        let id = pane.ok_or("no terminal pane")?.id;
        let shells = hub.panes.get_mut::<Shells>(Some(id)).ok_or("no shells")?;
        let Tab::Shell(feed) = shells.active else {
            return Err("the agent tab is in front".into());
        };
        let shell = shells.shell(feed).ok_or("no shell")?;
        let mut parser = shell.surface.parser.lock().map_err(|_| "poisoned")?;
        parser.process(format!("% python3 -m http.server 8765\r\n{SERVED}\r\n").as_bytes());
        format!("terminal-{}", id.0)
    };
    rest(&mut runtime);
    let shown = runtime.elements();
    let screen = keys(&shown)
        .into_iter()
        .find(|key| key.starts_with(&format!("{terminal}.screen.")))
        .ok_or("no terminal screen")?
        .to_owned();
    Ok((runtime, terminal, screen))
}

/// What the attached things send the model, as one text.
fn sent_words(runtime: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let sent = blocks(&hub.model.attached.things);
    let said = sent.iter().filter_map(|b| b["text"].as_str());
    Ok(said.collect())
}

#[test]
fn pick_mode_boxes_every_pane_and_a_press_on_the_terminal_attaches_its_screen() -> Check {
    let (mut runtime, terminal, screen) = served()?;
    let alt_cmd = Mods { alt: true, ..cmd() };
    strike(&mut runtime, Key::Text("p".into()), alt_cmd);
    let shown = runtime.elements();
    let boxed = keys(&shown)
        .into_iter()
        .filter(|key| key.starts_with(PANE_BOX));
    assert!(boxed.count() >= 7, "a box round every pane of Everything");
    assert!(live(&runtime, &[PANE_BOX, &terminal].concat()));
    tap(&mut runtime, &screen)?;
    let count = runtime.state.hub.try_borrow()?.model.attached.things.len();
    assert_eq!(count, 1, "the press attached one thing");
    let text = sent_words(&runtime)?;
    assert!(text.contains(SERVED), "the screen's words: {text}");
    assert!(!text.contains("yi://"), "no app address: {text}");
    Ok(())
}

#[test]
fn attach_to_chat_from_the_terminals_right_click_menu_sends_its_screen() -> Check {
    let (mut runtime, _, screen) = served()?;
    let [x, y, w, h] = by_key(&runtime.elements(), &screen)
        .ok_or("no screen")?
        .rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    runtime.pointer_move(x, y);
    runtime.secondary_down(x, y);
    runtime.secondary_up(x, y);
    rest(&mut runtime);
    tap(&mut runtime, "window.context-menu.row.thing.attach")?;
    let text = sent_words(&runtime)?;
    assert!(text.contains("Attached: pane · Terminal"), "{text}");
    assert!(text.contains(SERVED), "the screen's words: {text}");
    Ok(())
}

#[test]
fn picking_the_web_page_says_what_it_can_pick_and_sends_the_pages_address() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    let page = Row::new("page:0").text("url", "http://localhost:8773/");
    runtime.update(move |win, _| dispatch(win, Msg::Feed(WEB, FeedEvent::Upsert(page.clone()))));
    rest(&mut runtime);
    let web = keys(&runtime.elements())
        .into_iter()
        .find(|key| key.starts_with("web-") && key.ends_with(".page"))
        .ok_or("no web page surface")?
        .to_owned();
    let alt_cmd = Mods { alt: true, ..cmd() };
    strike(&mut runtime, Key::Text("p".into()), alt_cmd);
    let (x, y) = centre(&runtime, &web)?;
    runtime.pointer_move(x, y);
    rest(&mut runtime);
    let says = row_text(&runtime.elements(), &format!("{PREVIEW}.tag"))?;
    assert!(says.contains("its elements cannot be picked"), "{says}");
    tap(&mut runtime, &web)?;
    let text = sent_words(&runtime)?;
    assert!(text.contains("Attached: pane · Web app"), "{text}");
    assert!(text.contains("a picture"), "{text}");
    let open = "agent-browser open http://localhost:8773/";
    assert!(text.contains(open), "the agent is told how to look: {text}");
    Ok(())
}

/// Dies with the web pane's tab attached as its title alone: the agent read the environment
/// block that follows as the page.
#[test]
fn attaching_the_web_tab_from_its_right_click_menu_sends_the_pages_address() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    let page = Row::new("page:0").text("url", "http://localhost:8774/");
    runtime.update(move |win, _| dispatch(win, Msg::Feed(WEB, FeedEvent::Upsert(page.clone()))));
    rest(&mut runtime);
    let tab = keys(&runtime.elements())
        .into_iter()
        .find(|key| key.starts_with("web-") && key.ends_with(".pane-tab"))
        .ok_or("no web tab")?
        .to_owned();
    let (x, y) = centre(&runtime, &tab)?;
    runtime.pointer_move(x, y);
    runtime.secondary_down(x, y);
    runtime.secondary_up(x, y);
    rest(&mut runtime);
    tap(&mut runtime, "window.context-menu.row.thing.attach")?;
    let text = sent_words(&runtime)?;
    let open = "agent-browser open http://localhost:8774/";
    assert!(text.contains(open), "the page's address: {text}");
    Ok(())
}

/// Dies with a Code pane attached as only its name: the model is told nothing of the file.
#[test]
fn an_attached_code_pane_sends_its_files_path_and_the_lines_in_view() -> Check {
    use yd_core::command::table::{OPEN_FILE, Route};
    use yd_core::feed::FILES;
    use yd_core::row::rows_from_json;
    let mut runtime = runtime("shell/everything")?;
    runtime.update(|win, _| {
        let read = include_str!("../../../../../panes/src/code_view/sample/fs.rows.json");
        for file in rows_from_json(read).unwrap_or_default() {
            dispatch(win, Msg::Feed(FILES, FeedEvent::Upsert(file)));
        }
        let file = Address::new(Kind::File, "src/middleware/rate_limit.rs");
        if let Ok(file) = file {
            let run = Run::new(OPEN_FILE).on(Target::Thing(file));
            dispatch(win, Msg::Command(Route::Model, run));
        }
    });
    rest(&mut runtime);
    let crumb = keys(&runtime.elements())
        .into_iter()
        .find(|key| key.starts_with("code-") && key.ends_with(".breadcrumb"))
        .ok_or("no code pane showing a file")?
        .to_owned();
    let alt_cmd = Mods { alt: true, ..cmd() };
    strike(&mut runtime, Key::Text("p".into()), alt_cmd);
    tap(&mut runtime, &crumb)?;
    let text = sent_words(&runtime)?;
    assert!(text.contains("Attached: pane ·"), "{text}");
    assert!(text.contains("src/middleware/rate_limit.rs"), "{text}");
    assert!(text.contains("lines 1-"), "which lines: {text}");
    assert!(text.contains("const LIMIT: u32 = 20;"), "the code: {text}");
    assert!(!text.contains("Only this pane's name"), "{text}");
    Ok(())
}

#[test]
fn a_right_click_on_a_code_line_opens_the_menu_with_attach() -> Check {
    let mut runtime = runtime("code/mockup")?;
    rest(&mut runtime);
    let shown = runtime.elements();
    let line = keys(&shown)
        .into_iter()
        .find(|key| key.starts_with("code-") && key.ends_with(".L3"))
        .ok_or("no third line in the code viewer")?
        .to_owned();
    let [x, y, w, h] = by_key(&shown, &line).ok_or("no line")?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    runtime.pointer_move(x, y);
    runtime.secondary_down(x, y);
    runtime.secondary_up(x, y);
    rest(&mut runtime);
    assert!(live(&runtime, "window.context-menu"), "the menu opened");
    let head = row_text(&runtime.elements(), "window.context-menu-head")?;
    assert!(head.starts_with("rate_limit.rs line 3"), "{head}");
    assert!(
        live(&runtime, "window.context-menu.row.thing.attach"),
        "it offers Attach"
    );
    Ok(())
}
