//! What a reader of the code viewer would notice if it broke, on Rune's in-process runtime.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, ElementInfo, Env, Runtime, col};
use yd_core::address::uri::Address;
use yd_core::command::table::{OPEN_FILE, Route, Run, Target};
use yd_core::feed::{FILES, FeedEvent, OPEN_FILES, SourceCmd};
use yd_core::ids::{RowId, WindowId};
use yd_core::kind::Kind;
use yd_core::model::Model;
use yd_core::msg::{Effect, Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Value};
use yd_ui::grid_view::view::alone;
use yd_ui::pane::{PaneDef, SCENE_PANE, Stage};
use yd_ui::win::{Hub, Win};

use super::{DEF, current, path, scenes, thing_at};

const TABS: yd_core::ids::FeedId = OPEN_FILES;
static DEFS: &[&PaneDef] = &[&DEF];

type Check = Result<(), Box<dyn Error>>;
type Window = Runtime<Win>;
/// The effects a window's hub handed on.
type Sent = Rc<RefCell<Vec<Effect>>>;

const UPLOAD: &str = "src/routes/upload.rs";

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    alone(win, &hub, cx, PaneKind::Code)
}

/// The scene `name` in a window of 600 by 420 points, and the effects its hub hands on.
fn scene(name: &str) -> Result<(Window, Sent), Box<dyn Error>> {
    let scene = scenes::SCENES
        .iter()
        .find(|scene| scene.name == name)
        .ok_or_else(|| format!("no scene {name}"))?;
    let hub = Hub::new(DEFS, Vec::new());
    let effects = Sent::default();
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = Stage::Pane(PaneKind::Code);
        (scene.setup)(&mut hub);
        let sink = effects.clone();
        hub.set_sink(move |effect| sink.borrow_mut().push(effect));
    }
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (600.0, 420.0),
        Env::new(true, 1.0),
    );
    run(&mut rt, 0.3);
    Ok((rt, effects))
}

fn run(rt: &mut Window, seconds: f64) {
    let mut left = seconds;
    while left > 0.0 {
        rt.frame(1.0 / 60.0);
        left -= 1.0 / 60.0;
    }
}

fn element(rt: &Window, key: &str) -> Result<ElementInfo, String> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .ok_or_else(|| format!("no element keyed {key}"))
}

/// The words drawn inside the keyed element, left to right.
fn words(rt: &Window, key: &str) -> Result<Vec<String>, String> {
    let [x, y, w, h] = element(rt, key)?.rect;
    let inside = |r: &[f32; 4]| {
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        cx >= x && cx <= x + w && cy >= y && cy <= y + h
    };
    let mut found: Vec<(f32, String)> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && inside(&e.rect))
        .filter_map(|e| Some((e.rect[0], e.text?)))
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    // The bar's meta is drawn even when a pane has none to say.
    found.retain(|(_, word)| !word.is_empty());
    Ok(found.into_iter().map(|(_, word)| word).collect())
}

fn shown_file(rt: &Window) -> Result<String, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    let table = hub.model.tables.get(TABS).ok_or("no tabs")?;
    Ok(path(current(table).ok_or("no tab shows")?).to_owned())
}

#[test]
fn the_open_file_shows_with_its_tabs_its_path_and_its_facts() -> Check {
    let (rt, _) = scene("code/mockup")?;
    let bar = element(&rt, "code-0.pane-tab-bar")?.rect;
    assert_eq!((bar[2], bar[3]), (600.0, 20.0));
    assert_eq!(
        words(&rt, "code-0.pane-tab-bar")?,
        ["rate_limit.rs", "upload.rs", "Rust · 23 lines · new"]
    );
    let on = element(&rt, "code-0.tab-1.pane-tab")?;
    let off = element(&rt, "code-0.tab-2.pane-tab")?;
    let bars = rt.elements().into_iter().filter(|e| {
        let key = e.key.as_deref().unwrap_or_default();
        !e.exiting && key.ends_with(".pane-tab-bar")
    });
    assert_eq!(bars.count(), 1, "the pane draws no bar of its own");
    assert_eq!((on.rect[3], off.rect[3]), (19.0, 19.0));
    assert!(
        on.fill.a > 0.0 && off.fill.a == 0.0,
        "only the file that shows has a lit tab"
    );
    assert_eq!(
        words(&rt, "code-0.breadcrumb")?,
        ["src / middleware / ", "rate_limit.rs"]
    );
    assert_eq!(
        words(&rt, "code-0.L7")?,
        ["7", "+", "pub struct Bucket {"],
        "a line of a file new in this thread: its number, its sign, its text"
    );
    assert_eq!(element(&rt, "code-0.L7")?.rect[3], 16.0);
    assert_eq!(
        element(&rt, "code-0.L7")?.fill.a,
        0.0,
        "a new line has a bar, not a fill"
    );
    assert!(element(&rt, "code-0.L23").is_ok() && element(&rt, "code-0.L24").is_err());
    Ok(())
}

#[test]
fn a_pane_with_no_file_open_keeps_its_name_on_its_tab() -> Check {
    let (rt, _) = scene("code/empty")?;
    assert_eq!(words(&rt, "code-0.pane-tab-bar")?, ["Code"]);
    assert_eq!(element(&rt, "code-0.tab-0.pane-tab")?.rect[3], 19.0);
    assert!(element(&rt, "code-0.breadcrumb").is_err());
    let (rt, _) = scene("code/mockup")?;
    assert!(
        element(&rt, "code-0.tab-0.pane-tab").is_err(),
        "with a file open the tabs are the files"
    );
    Ok(())
}

#[test]
fn pressing_a_tab_shows_that_file_and_asks_the_files_source_for_its_text() -> Check {
    let (mut rt, effects) = scene("code/mockup")?;
    let tab = element(&rt, "code-0.tab-2.pane-tab")?.rect;
    let at = (tab[0] + tab[2] / 2.0, tab[1] + tab[3] / 2.0);
    rt.pointer_move(at.0, at.1);
    rt.pointer_down(at.0, at.1);
    rt.pointer_up(at.0, at.1);
    run(&mut rt, 0.2);

    assert_eq!(shown_file(&rt)?, UPLOAD);
    let asked = effects.borrow().clone();
    let of_files = |effect: &Effect| matches!(effect, Effect::Source(FILES, _));
    let reads: Vec<Effect> = asked.into_iter().filter(of_files).collect();
    assert_eq!(
        reads,
        [Effect::Source(FILES, SourceCmd::Read(UPLOAD.into()))],
        "one read is asked for"
    );
    assert_eq!(
        words(&rt, "code-0.breadcrumb")?,
        ["src / routes / ", "upload.rs"]
    );
    assert!(
        element(&rt, "code-0.L1").is_err(),
        "no line shows until the file is read"
    );
    assert_eq!(
        words(&rt, "code-0.pane-tab-bar")?
            .last()
            .map(String::as_str),
        Some("Rust")
    );

    // The files source answers with the row `file:<path>`.
    let read = Row::new(format!("file:{UPLOAD}"))
        .text("path", UPLOAD)
        .text(
            "text",
            "pub fn routes() -> Router {\n    Router::new()\n}\n",
        )
        .text("lines", "3");
    rt.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            step(&mut hub.model, Msg::Feed(FILES, FeedEvent::Upsert(read)));
        }
    });
    run(&mut rt, 0.2);
    assert_eq!(words(&rt, "code-0.L2")?, ["2", "    Router::new()"]);
    assert_eq!(
        words(&rt, "code-0.pane-tab-bar")?
            .last()
            .map(String::as_str),
        Some("Rust · 3 lines")
    );
    Ok(())
}

#[test]
fn opening_a_file_adds_its_tab_once_and_shows_it() -> Check {
    let mut model = Model::default();
    let open = |model: &mut Model, address: &Address| {
        let run = Run::new(OPEN_FILE).on(Target::Thing(address.clone()));
        step(model, Msg::Command(Route::Model, run))
    };
    let apply = |model: &mut Model, address: &Address| -> Check {
        let asked = open(model, address);
        assert_eq!(asked.len(), 1, "the file's text is asked for");
        Ok(())
    };
    let (a, b) = (
        Address::new(Kind::File, "src/a.rs")?,
        Address::new(Kind::Lines, "src/b.rs")?.part("L3-L9"),
    );
    apply(&mut model, &a)?;
    apply(&mut model, &b)?;
    apply(&mut model, &a)?;
    let table = model.tables.get(TABS).ok_or("no tabs")?;
    let ids: Vec<&str> = super::tabs(table).map(|tab| tab.id.as_str()).collect();
    assert_eq!(
        ids,
        ["file:src/a.rs", "file:src/b.rs"],
        "a file opened again keeps its one tab, in its place"
    );
    assert_eq!(current(table).map(path), Some("src/a.rs"));

    // A fact the opener gave the tab, such as being new in this thread, survives a reopen.
    let fresh = Row::new("file:src/b.rs")
        .text("path", "src/b.rs")
        .cell("new", Value::Flag(true));
    step(&mut model, Msg::Feed(TABS, FeedEvent::Upsert(fresh)));
    apply(&mut model, &b)?;
    let table = model.tables.get(TABS).ok_or("no tabs")?;
    let tab = table.find(&RowId::new("file:src/b.rs")).ok_or("no tab")?;
    assert_eq!(tab.get("new"), Some(&Value::Flag(true)));
    assert_eq!(current(table).map(path), Some("src/b.rs"));
    assert!(open(&mut model, &Address::new(Kind::Thread, "t1")?).is_empty());
    Ok(())
}

#[test]
fn a_line_stands_for_its_file_and_number_and_a_selected_line_for_the_selection() -> Check {
    let (mut rt, _) = scene("code/mockup")?;
    let thing = |rt: &Window, key: &str| -> Result<Option<String>, Box<dyn Error>> {
        let hub = rt.state.hub.try_borrow()?;
        Ok(thing_at(&hub, SCENE_PANE, key).map(|thing| thing.address.to_string()))
    };
    let file = "yi://lines/src/middleware/rate_limit.rs";
    assert_eq!(thing(&rt, "code-0.L7")?, Some(format!("{file}#L7")));
    assert_eq!(thing(&rt, "code-0.row.L7")?, Some(format!("{file}#L7")));
    assert_eq!(thing(&rt, "code-0.L99")?, None);
    assert_eq!(thing(&rt, "code-0.breadcrumb")?, None);

    // A drag from line 3 to line 4 selects both: either now stands for the two lines.
    let (from, to) = (
        element(&rt, "code-0.row.L3")?.rect,
        element(&rt, "code-0.row.L4")?.rect,
    );
    rt.pointer_move(from[0] + 2.0, from[1] + 8.0);
    rt.pointer_down(from[0] + 2.0, from[1] + 8.0);
    run(&mut rt, 0.05);
    rt.pointer_move(to[0] + 60.0, to[1] + 8.0);
    run(&mut rt, 0.05);
    rt.pointer_up(to[0] + 60.0, to[1] + 8.0);
    run(&mut rt, 0.05);
    assert_eq!(thing(&rt, "code-0.L4")?, Some(format!("{file}#L3-L4")));
    assert_eq!(thing(&rt, "code-0.L7")?, Some(format!("{file}#L7")));
    Ok(())
}

#[test]
fn marks_and_a_panel_others_put_on_the_file_draw_on_its_lines() -> Check {
    let (rt, _) = scene("code/marks")?;
    assert!(element(&rt, "code-0.L14.breakpoint").is_ok());
    assert!(element(&rt, "code-0.L20.fix").is_ok());
    assert_eq!(
        words(&rt, "code-0.L13.value")?,
        ["now = 1731.42s"],
        "the value the debugger shows after line 13"
    );
    assert!(element(&rt, "code-0.L18")?.fill.a > 0.1);
    assert_eq!(element(&rt, "code-0.L10.sign")?.text.as_deref(), Some("-"));
    assert_eq!(element(&rt, "code-0.L9.sign")?.text.as_deref(), Some("+"));

    let (rt, _) = scene("code/slot-open")?;
    let (panel, above, below) = (
        element(&rt, "code-0.slot.references")?.rect,
        element(&rt, "code-0.L13")?.rect,
        element(&rt, "code-0.L14")?.rect,
    );
    assert_eq!(panel[1], above[1] + above[3]);
    assert_eq!(below[1], panel[1] + panel[3]);
    assert!(
        panel[3] > 100.0,
        "the panel is as tall as its six references: {}",
        panel[3]
    );
    Ok(())
}

#[test]
fn a_language_server_that_is_not_running_says_why_beside_the_path() -> Check {
    let (mut rt, _) = scene("code/mockup")?;
    let why = "error · exited with code 1: error: Unknown binary 'rust-analyzer' in official toolchain 'stable-aarch64-apple-darwin'.";
    let server = Row::new("server:rust-analyzer")
        .cell("state", Value::Dot(yd_core::row::Dot::Fail))
        .text("server", "rust-analyzer")
        .text("language", "Rust")
        .text("status", why);
    rt.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let feed = Msg::Feed(yd_core::feed::LSP, FeedEvent::Upsert(server.clone()));
            step(&mut hub.model, feed);
        }
    });
    run(&mut rt, 0.2);
    let crumb = words(&rt, "code-0.breadcrumb")?;
    assert_eq!(crumb.last().map(String::as_str), Some(why), "{crumb:?}");
    let dot = element(&rt, "code-0.breadcrumb.server")?;
    assert_eq!(dot.help.as_deref(), Some(why), "the dot's tooltip");
    Ok(())
}

/// The `lsp.open` asks the pane sent, by the path they name.
fn told(effects: &Sent) -> Vec<String> {
    let asks = effects.borrow();
    let opens = asks.iter().filter_map(|effect| match effect {
        Effect::Source(yd_core::feed::LSP, SourceCmd::Run(run))
            if run.command == yd_core::command::tools::lsp::OPEN =>
        {
            match &run.target {
                Target::Thing(address) => Some(address.id().to_owned()),
                Target::App | Target::Pane(_) => None,
            }
        }
        _ => None,
    });
    opens.collect()
}

#[test]
fn a_file_changed_on_disk_shows_its_new_error_with_a_count_in_the_bar() -> Check {
    let (mut rt, effects) = scene("code/mockup")?;
    let file = "src/middleware/rate_limit.rs";
    assert_eq!(
        told(&effects),
        [file],
        "the server hears of the open file once"
    );
    let text = {
        let hub = rt.state.hub.try_borrow()?;
        let files = hub.model.tables.get(FILES).ok_or("no files")?;
        let row = files
            .find(&RowId::new(format!("file:{file}")))
            .ok_or("unread")?;
        row.words("text").ok_or("no text")?.to_owned()
    };
    let changed = Row::new(format!("file:{file}"))
        .text("path", file)
        .text(
            "text",
            format!("{text}pub fn broken() -> u32 {{ \"x\" }}\n"),
        )
        .text("lines", "24");
    let error = Row::new(format!("problem:{file}:24:0"))
        .cell(
            "severity",
            Value::Badge("Error".into(), yd_core::row::Tone::Bad),
        )
        .text("message", "mismatched types: expected `u32`, found `&str`")
        .text("col", "26")
        .text("end_col", "29");
    rt.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            step(&mut hub.model, Msg::Feed(FILES, FeedEvent::Upsert(changed)));
        }
    });
    run(&mut rt, 0.2);
    assert_eq!(
        told(&effects),
        [file, file],
        "the changed text is told again"
    );
    rt.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let feed = Msg::Feed(yd_core::feed::LSP, FeedEvent::Upsert(error));
            step(&mut hub.model, feed);
        }
    });
    run(&mut rt, 0.2);
    assert_eq!(
        words(&rt, "code-0.pane-tab-bar")?
            .last()
            .map(String::as_str),
        Some("Rust · 24 lines · new · 1 error")
    );
    Ok(())
}
