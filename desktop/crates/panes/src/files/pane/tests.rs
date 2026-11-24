use std::error::Error;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::feed::FeedEvent;
use yd_core::ids::WindowId;
use yd_core::msg::{Msg, step};
use yd_core::row::{Row, Rows, Value};
use yd_ui::pane::{PaneCx, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::marks::{Mark, marks};
use super::scenes::sample;
use super::{CHANGES, FEED};

#[test]
fn the_threads_reads_and_edits_mark_the_files_they_named() -> Result<(), Box<dyn Error>> {
    let shared = Hub::new(&[], Vec::new());
    let mut hub = shared.try_borrow_mut()?;
    sample(&mut hub, None);
    let thread = hub
        .model
        .wire
        .listed()
        .into_iter()
        .next()
        .ok_or("no thread")?;
    let marks = marks(thread, yd_ui::shown::works_in(&hub, thread));
    let mark = |path: &str| marks.get(path).copied().unwrap_or_default();
    assert_eq!(
        mark("tests/upload.rs"),
        Mark {
            reads: 2,
            edited: false,
            writing: false
        }
    );
    assert_eq!(
        mark("src/routes/upload.rs"),
        Mark {
            reads: 1,
            edited: true,
            writing: false
        }
    );
    assert_eq!(
        mark("src/middleware/rate_limit.rs"),
        Mark {
            reads: 0,
            edited: true,
            writing: true
        }
    );
    assert_eq!(
        mark("tests/upload.rs").word().as_deref(),
        Some("read twice")
    );
    assert_eq!(mark("src/routes/upload.rs").word(), None);
    Ok(())
}

/// The files pane alone in a window, with the scene's sample.
fn window(search: Option<&str>) -> Result<Runtime<Win>, Box<dyn Error>> {
    let shared = Hub::new(&[], Vec::new());
    sample(&mut *shared.try_borrow_mut()?, search);
    let mut rt = Runtime::new(
        Win::new(shared, WindowId(1)),
        alone,
        (345.0, 700.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    Ok(rt)
}

fn alone(win: &Win, cx: &mut Cx<Win>) -> El {
    match win.hub.try_borrow() {
        Ok(hub) => (super::DEF.view)(
            &PaneCx {
                hub: &hub,
                pane: SCENE_PANE,
                focused: true,
                edits: None,
            },
            cx,
        ),
        Err(_) => col(),
    }
}

fn find(rt: &Runtime<Win>, key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let all = rt.elements();
    Ok(all
        .iter()
        .find(|e| e.key.as_deref() == Some(key))
        .ok_or(format!("no {key}"))?
        .rect)
}

fn texts(rt: &Runtime<Win>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.text)
        .collect()
}

fn press(rt: &mut Runtime<Win>, key: &str) -> Result<(), Box<dyn Error>> {
    let [x, y, w, h] = find(rt, key)?;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    Ok(())
}

#[test]
fn the_search_box_is_shut_until_its_button_is_pressed() -> Result<(), Box<dyn Error>> {
    let mut rt = window(None)?;
    assert_eq!(
        find(&rt, "files-0.search-box")?[3],
        0.0,
        "the box shows at rest"
    );
    press(&mut rt, "files-0.search")?;
    assert_eq!(find(&rt, "files-0.search-box")?[3], 22.0);
    rt.key(KeyEvent {
        key: Key::Text("rate".into()),
        mods: Mods::default(),
    });
    rt.frame(0.0);
    let shown = texts(&rt);
    assert!(shown.iter().any(|t| t == "1 match"), "{shown:?}");
    assert!(
        !shown.iter().any(|t| t == "auth.rs"),
        "the tree still shows while typing"
    );
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(find(&rt, "files-0.search-box")?[3], 0.0);
    Ok(())
}

#[test]
fn untouched_files_fade_and_touched_ones_carry_their_marks() -> Result<(), Box<dyn Error>> {
    let rt = window(None)?;
    let opacity = |key: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .map(|e| e.opacity)
    };
    assert_eq!(opacity("files-0.row.src/middleware/auth.rs"), Some(0.4));
    assert_eq!(opacity("files-0.row.src/main.rs"), Some(1.0));
    assert_eq!(opacity("files-0.row..github"), Some(0.4));
    let shown = texts(&rt);
    for words in [
        "writing now",
        "read twice",
        "Touched · 5",
        "Edited · 3",
        "+48",
    ] {
        assert!(
            shown.iter().any(|t| t == words),
            "{words} missing from {shown:?}"
        );
    }
    Ok(())
}

#[test]
fn a_tree_the_walk_cut_short_says_so_under_the_list() -> Result<(), Box<dyn Error>> {
    let shared = Hub::new(&[], Vec::new());
    {
        let mut hub = shared.try_borrow_mut()?;
        sample(&mut hub, None);
        let cut = Row::new("tree")
            .cell("truncated", Value::Flag(true))
            .text("cut", "4 files shown: the walk stops at 20000 entries");
        step(
            &mut hub.model,
            Msg::Feed(super::FEED, FeedEvent::Upsert(cut)),
        );
    }
    let mut rt = Runtime::new(
        Win::new(shared, WindowId(1)),
        alone,
        (345.0, 700.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let said = texts(&rt);
    assert!(
        said.iter()
            .any(|t| t == "[…] 4 files shown: the walk stops at 20000 entries"),
        "{said:?}"
    );
    Ok(())
}

#[test]
fn pressing_a_file_opens_it_in_the_code_viewer() -> Result<(), Box<dyn Error>> {
    let mut rt = window(None)?;
    let row = rt.elements().into_iter().find_map(|e| {
        let key = e
            .key
            .filter(|key| key.contains(".row.") && key.ends_with("upload.rs"))?;
        (!e.exiting).then_some(key)
    });
    press(&mut rt, &row.ok_or("upload.rs has no row")?)?;
    let hub = rt.state.hub.try_borrow()?;
    let open = hub.model.tables.get(yd_core::feed::OPEN_FILES);
    let tab =
        open.and_then(|open| open.find(&yd_core::ids::RowId::new("file:src/routes/upload.rs")));
    assert!(
        tab.is_some(),
        "a pressed file has no tab in the code viewer"
    );
    Ok(())
}

/// The viewer's open files as `(file name, is the preview)`, in tab order.
fn open_files(rt: &Runtime<Win>) -> Result<Vec<(String, bool)>, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    let Some(table) = hub.model.tables.get(yd_core::feed::OPEN_FILES) else {
        return Ok(Vec::new());
    };
    let tabs = (0..table.len()).filter_map(|at| table.get(at));
    let file = |tab: &Row| {
        let path = tab.words("path").unwrap_or_default();
        let name = path.rsplit('/').next().unwrap_or_default().to_owned();
        (name, yd_core::command::run::previewed(tab))
    };
    Ok(tabs.map(file).collect())
}

fn row_of(rt: &Runtime<Win>, name: &str) -> Result<String, Box<dyn Error>> {
    let row = rt.elements().into_iter().find_map(|e| {
        let key = e
            .key
            .filter(|key| key.contains(".row.") && key.ends_with(name))?;
        (!e.exiting).then_some(key)
    });
    Ok(row.ok_or(format!("{name} has no row"))?)
}

/// One press on a file looks at it: the next press on another takes its tab. A second press
/// on the same row keeps the file, and the next look opens beside it.
#[test]
fn a_press_looks_at_a_file_and_a_double_press_keeps_it() -> Result<(), Box<dyn Error>> {
    let mut rt = window(None)?;
    let (upload, limit) = (row_of(&rt, "upload.rs")?, row_of(&rt, "rate_limit.rs")?);
    press(&mut rt, &upload)?;
    press(&mut rt, &limit)?;
    assert_eq!(open_files(&rt)?, [("rate_limit.rs".to_owned(), true)]);
    let [x, y, w, h] = find(&rt, &limit)?;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    for _ in 0..2 {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(open_files(&rt)?, [("rate_limit.rs".to_owned(), false)]);
    press(&mut rt, &upload)?;
    let both = [
        ("rate_limit.rs".to_owned(), false),
        ("upload.rs".to_owned(), true),
    ];
    assert_eq!(open_files(&rt)?, both);
    Ok(())
}

#[test]
fn a_file_row_is_found_as_its_file_in_the_tree_and_under_changed() -> Result<(), Box<dyn Error>> {
    let hub = Hub::new(&[], Vec::new());
    let hub = hub.try_borrow()?;
    let find = |key: &str| super::thing_at(&hub, SCENE_PANE, key).map(|t| t.address.to_string());
    let want = Some("yi://file/src/routes/upload.rs");
    assert_eq!(find("files-2.row.src/routes/upload.rs").as_deref(), want);
    assert_eq!(
        find("files-2.row.change:src/routes/upload.rs").as_deref(),
        want
    );
    assert_eq!(find("files-2.search-box"), None);
    Ok(())
}

const ASK: &str = include_str!("../../../../../fixtures/wire/approval-allow.jsonl");
const LANE: &str = "/Users/someone/.yi/lanes/5e1f0c2a/0";

/// A pane of `def` in a window that shows the recorded session as a second client hears it,
/// with git's row for the session's own checkout, and what the window sent.
fn checkout(def: &'static yd_ui::pane::PaneDef) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    let shared = Hub::new(&[], Vec::new());
    let sent = Sent::default();
    {
        let mut hub = shared.try_borrow_mut()?;
        let heard: Vec<&str> = ASK.lines().filter(|l| !l.contains("_yi/workdir")).collect();
        yd_core::wire::replay::play(&mut hub.model, &heard.join("\n"))?;
        let id = hub
            .model
            .wire
            .listed()
            .first()
            .ok_or("no thread")?
            .id
            .clone();
        let lane = Row::new(format!("worktree:yi/{}", id.as_str())).text("path", LANE);
        let repo = Row::new("repo").text("branch", "main").text("base", "main");
        for row in [lane.cell("changed", Value::Flag(true)), repo] {
            step(&mut hub.model, Msg::Feed(CHANGES, FeedEvent::Upsert(row)));
        }
        let log = sent.clone();
        hub.set_sink(move |effect| log.borrow_mut().push(effect));
    }
    let mut win = Win::new(shared, WindowId(0));
    yd_ui::shown::sync(&mut win);
    let view = match def.kind {
        yd_core::pane::PaneKind::Code => alone_code,
        yd_core::pane::PaneKind::Changes => alone_changes,
        _ => alone,
    };
    let mut rt = Runtime::new(win, view, (520.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    Ok((rt, sent))
}

type Sent = std::rc::Rc<std::cell::RefCell<Vec<yd_core::msg::Effect>>>;

fn drawn(def: &yd_ui::pane::PaneDef, win: &Win, cx: &mut Cx<Win>) -> El {
    match win.hub.try_borrow() {
        Ok(hub) => (def.view)(
            &PaneCx {
                hub: &hub,
                pane: SCENE_PANE,
                focused: true,
                edits: None,
            },
            cx,
        ),
        Err(_) => col(),
    }
}

fn alone_code(win: &Win, cx: &mut Cx<Win>) -> El {
    drawn(&crate::code_view::DEF, win, cx)
}

fn alone_changes(win: &Win, cx: &mut Cx<Win>) -> El {
    drawn(&crate::changes::DEF, win, cx)
}

fn line(rt: &Runtime<Win>, key: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let [x, y, w, h] = find(rt, key)?;
    let inside =
        |r: &[f32; 4]| r[1] >= y && r[1] + r[3] <= y + h + 0.5 && r[0] >= x && r[0] <= x + w;
    let words = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && inside(&e.rect));
    Ok(words
        .filter_map(|e| e.text)
        .filter(|t| !t.is_empty())
        .collect())
}

fn roots(sent: &Sent) -> Vec<String> {
    let sent = sent.borrow();
    let roots = sent.iter().filter_map(|effect| match effect {
        yd_core::msg::Effect::Source(FEED, yd_core::feed::SourceCmd::Root(folder)) => {
            Some(folder.display().to_string())
        }
        _ => None,
    });
    roots.collect()
}

#[test]
fn panes_on_the_agents_checkout_say_so_and_can_show_the_project_folder()
-> Result<(), Box<dyn Error>> {
    for def in [&super::DEF, &crate::code_view::DEF, &crate::changes::DEF] {
        let (rt, _) = checkout(def)?;
        let key = format!("{}-0.copy", def.kind.slug());
        let said = line(&rt, &key)?;
        let first = said.first().ok_or("the line says nothing")?;
        assert!(
            first.starts_with("Agent's copy · yi/") && first.ends_with(" · from $PROJECT/main"),
            "{}: {said:?}",
            def.kind.slug()
        );
    }
    let (mut rt, sent) = checkout(&super::DEF)?;
    assert_eq!(roots(&sent), [LANE]);
    press(&mut rt, "files-0.copy.show")?;
    let project = {
        let hub = rt.state.hub.try_borrow()?;
        hub.model
            .wire
            .listed()
            .first()
            .ok_or("no thread")?
            .cwd
            .clone()
    };
    assert_eq!(roots(&sent), [LANE.to_owned(), project.clone()]);
    let said = line(&rt, "files-0.copy")?;
    assert_eq!(
        said.first().map(String::as_str),
        Some("Project folder · $PROJECT")
    );
    press(&mut rt, "files-0.copy.show")?;
    assert_eq!(roots(&sent).last(), Some(&LANE.to_owned()), "and back");
    Ok(())
}

#[test]
fn panes_on_the_project_folder_itself_say_nothing_of_a_copy() -> Result<(), Box<dyn Error>> {
    let rt = window(None)?;
    assert!(find(&rt, "files-0.copy").is_err());
    Ok(())
}
