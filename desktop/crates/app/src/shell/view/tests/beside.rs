//! A file opened beside, in the real window: a ⌘-click gives it a code pane of its own, an
//! ⌥⌘-click a window of its own, and each code pane goes on showing its own file.

use rune::PhysicalInput;
use yd_core::command::tools::lsp::REFERENCES;
use yd_core::feed::{FILES, FeedEvent, LSP, OPEN_FILES, SourceCmd};
use yd_core::grid::step::GridMsg;
use yd_core::ids::PaneId;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows};
use yd_panes::code_view::pane::thing_at;
use yd_panes::code_view::refs;
use yd_ui::code::doc::{Cols, LineNo};
use yd_ui::code::pointer::{CodeEvent, Token};
use yd_ui::grid_view::beside;
use yd_ui::grid_view::view::Grips;
use yd_ui::win::dispatch;

use super::bar::{of, one};
use super::opens::menu;
use super::*;

const MAIN_RS: &str = "files-1.row.src/main.rs";
const SESSIONS: &str = "files-1.row.src/routes/sessions.rs";
const SPLIT_RIGHT: &str = "window.context-menu.row.tab.split-right";
const SPLIT_DOWN: &str = "window.context-menu.row.tab.split-down";

/// A press on `key` with `held` down, let go after.
fn tap_with(runtime: &mut Runtime<Win>, key: &str, held: Mods) -> Check {
    runtime.physical_input(PhysicalInput::Modifiers(held));
    tap(runtime, key)?;
    runtime.physical_input(PhysicalInput::Modifiers(Mods::default()));
    rest(runtime);
    Ok(())
}

/// The code pane about `path`, when the grid holds one.
fn about(runtime: &Runtime<Win>, path: &str) -> Result<Option<PaneId>, Box<dyn Error>> {
    let file = Address::new(Kind::File, path)?;
    let hub = runtime.state.hub.try_borrow()?;
    let is = |pane: &&yd_core::grid::windows::Pane| pane.thing.as_ref() == Some(&file);
    Ok(hub.model.grid.panes.iter().find(is).map(|pane| pane.id))
}

/// The path a code pane's breadcrumb starts with, its folders and name run together. A
/// narrow pane clips the name, so every word that starts inside the row counts.
fn crumb(runtime: &Runtime<Win>, pane: PaneId) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let key = format!("code-{}.breadcrumb", pane.0);
    let [x, y, w, h] = by_key(&shown, &key).ok_or(format!("no {key}"))?.rect;
    let starts = |e: &&ElementInfo| {
        let [ex, ey, ..] = e.rect;
        e.text.is_some() && ex >= x && ex < x + w && ey >= y && ey < y + h
    };
    let mut words: Vec<&ElementInfo> = shown.iter().filter(starts).collect();
    words.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    let path: String = words
        .iter()
        .take(2)
        .filter_map(|e| e.text.as_deref())
        .collect();
    Ok(path.replace(' ', ""))
}

fn frame(runtime: &Runtime<Win>, pane: PaneId) -> Result<[f32; 4], Box<dyn Error>> {
    let key = format!("code-{}.pane-frame", pane.0);
    let found = by_key(&runtime.elements(), &key).map(|frame| frame.rect);
    Ok(found.ok_or(format!("no {key}"))?)
}

/// The files that have a tab in the viewer.
fn tabs(runtime: &Runtime<Win>) -> Result<Vec<String>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let open = hub.model.tables.get(OPEN_FILES).ok_or("no open files")?;
    let paths = (0..open.len()).filter_map(|at| open.get(at)?.words("path"));
    Ok(paths.map(str::to_owned).collect())
}

type Asked = Rc<RefCell<Vec<Effect>>>;

/// Everything the window asks of the world from here on.
fn effects(runtime: &Runtime<Win>) -> Result<Asked, Box<dyn Error>> {
    let asked = Asked::default();
    let log = Rc::clone(&asked);
    let mut hub = runtime.state.hub.try_borrow_mut()?;
    hub.set_sink(move |effect| log.borrow_mut().push(effect));
    Ok(asked)
}

/// The files the files source was asked to read, in order.
fn reads(asked: &Asked) -> Vec<String> {
    let asked = asked.borrow();
    let read = asked.iter().filter_map(|effect| match effect {
        Effect::Source(FILES, SourceCmd::Read(path)) => path.to_str().map(str::to_owned),
        _ => None,
    });
    read.collect()
}

/// The pane with the keys in the main window.
fn focus(runtime: &Runtime<Win>) -> Result<Option<PaneId>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grip = hub
        .panes
        .get::<Grips>(None)
        .map(|grips| grips.of(root::MAIN));
    Ok(grip.and_then(|grip| grip.focus))
}

/// The file the first line of `pane` stands for when picked.
fn picked(runtime: &Runtime<Win>, pane: PaneId) -> Result<Option<String>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let thing = thing_at(&hub, pane, "L1");
    Ok(thing.map(|thing| thing.address.id().to_owned()))
}

/// A ⌘-click on a file opens it right of the viewer in a pane about it, which asks for its
/// text and has the keys. The viewer keeps its file, and a plain click changes it alone.
#[test]
fn a_command_click_opens_the_file_beside_and_each_pane_shows_its_own() -> Check {
    let mut runtime = settled("shell/everything")?;
    let asked = effects(&runtime)?;
    let viewer = one(&runtime, PaneKind::Code)?;
    let (was, open) = (crumb(&runtime, viewer)?, tabs(&runtime)?);
    let [x, y, w, _] = frame(&runtime, viewer)?;
    tap_with(&mut runtime, MAIN_RS, cmd())?;
    let pinned = about(&runtime, "src/main.rs")?.ok_or("no pane about main.rs")?;
    assert_eq!(of(&runtime, PaneKind::Code)?, [viewer, pinned]);
    assert_eq!(tabs(&runtime)?, open, "the viewer gains no tab");
    assert_eq!(crumb(&runtime, viewer)?, was, "and keeps its file");
    let ([left, ..], [right, top, ..]) = (frame(&runtime, viewer)?, frame(&runtime, pinned)?);
    assert!((left - x).abs() < 0.5 && (top - y).abs() < 0.5, "the row");
    assert!(right > x && right < x + w, "cut from the viewer's slot");
    assert_eq!(reads(&asked), ["src/main.rs"], "it asks for its text");
    let tab = format!("code-{}.tab-0.pane-tab", pinned.0);
    assert_eq!(row_text(&runtime.elements(), &tab)?, "main.rs");
    assert_eq!(focus(&runtime)?, Some(pinned), "it has the keys");
    runtime.update(|win, _| {
        let text = Row::new("file:src/main.rs").text("text", "fn main() {}\n");
        dispatch(win, Msg::Feed(FILES, FeedEvent::Upsert(text)));
    });
    rest(&mut runtime);
    assert_eq!(crumb(&runtime, pinned)?, "src/main.rs");
    assert!(live(&runtime, &format!("code-{}.code", pinned.0)), "read");
    assert_eq!(picked(&runtime, pinned)?.as_deref(), Some("src/main.rs"));
    assert_eq!(picked(&runtime, viewer)?, Some(was.clone()), "each its own");
    // A ⌘-click on a name in the pane beside asks about that pane's file.
    runtime.update(move |win, tx| {
        let (line, cols) = (LineNo::from_index(0), Cols::new(3, 7));
        let (token, follows) = (CodeEvent::CommandPress, false);
        refs::heard(
            win,
            tx,
            pinned,
            token(Token {
                line,
                cols,
                follows,
            }),
        );
    });
    let of_file = |effect: &Effect| match effect {
        Effect::Source(LSP, SourceCmd::Run(run)) if run.command == REFERENCES => {
            match &run.target {
                Target::Thing(address) => Some(address.id().to_owned()),
                Target::App | Target::Pane(_) => None,
            }
        }
        _ => None,
    };
    let about_file: Vec<String> = asked.borrow().iter().filter_map(of_file).collect();
    assert_eq!(
        about_file,
        ["src/main.rs"],
        "the references of its own file"
    );
    tap(&mut runtime, SESSIONS)?;
    assert_eq!(crumb(&runtime, viewer)?, "src/routes/sessions.rs");
    assert_eq!(crumb(&runtime, pinned)?, "src/main.rs", "its own file");
    let all = ["src/main.rs", "src/routes/sessions.rs"];
    assert_eq!(reads(&asked), all, "a file with a tab is asked for once");
    tap_with(&mut runtime, MAIN_RS, cmd())?;
    assert_eq!(
        of(&runtime, PaneKind::Code)?,
        [viewer, pinned],
        "shown once"
    );
    assert_eq!(crumb(&runtime, viewer)?, "src/routes/sessions.rs");
    let open = tabs(&runtime)?;
    tap(
        &mut runtime,
        &format!("code-{}.tab-0.pane-tab-close", pinned.0),
    )?;
    assert_eq!(of(&runtime, PaneKind::Code)?, [viewer], "its tab closes it");
    assert_eq!(tabs(&runtime)?, open, "and none of the viewer's");
    // Lines of a file shown beside are a pane about the file: the next lines find it.
    for part in ["L3", "L9"] {
        let lines = Address::new(Kind::Lines, "src/main.rs")?.part(part);
        runtime.update(move |win, tx| beside::show(win, tx, &lines, None, false));
        rest(&mut runtime);
        assert!(about(&runtime, "src/main.rs")?.is_some(), "{part}");
        assert_eq!(of(&runtime, PaneKind::Code)?.len(), 2, "{part}");
    }
    Ok(())
}

/// The words on the tab of the `kind` pane about `thing` once it waits behind another.
fn behind(kind: PaneKind, thing: Address) -> Result<String, Box<dyn Error>> {
    let mut runtime = settled("shell/everything")?;
    let about = thing.clone();
    runtime.update(move |win, _| {
        let (window, thing) = (root::MAIN, about);
        let about = GridMsg::About {
            window,
            kind,
            thing,
        };
        dispatch(win, Msg::Grid(about));
    });
    rest(&mut runtime);
    let (pane, mate) = {
        let hub = runtime.state.hub.try_borrow()?;
        let grid = &hub.model.grid;
        let is = |pane: &&yd_core::grid::windows::Pane| pane.thing.as_ref() == Some(&thing);
        let pane = grid.panes.iter().find(is).map(|pane| pane.id);
        let pane = pane.ok_or("no pane about it")?;
        (pane, grid.mates(pane).first().copied())
    };
    let mate = mate.ok_or("it joined no stack")?;
    runtime.update(move |win, _| dispatch(win, Msg::Grid(GridMsg::Front(mate))));
    rest(&mut runtime);
    let shown = runtime.elements();
    let tab = format!(".tab-pane-{}.pane-tab", pane.0);
    let key = keys(&shown).into_iter().find(|key| key.ends_with(&tab));
    row_text(&shown, key.ok_or("no tab for it")?)
}

/// A pane about a file that waits behind another is a tab named by its file, not its kind;
/// one about a thing that is no file, a child's chat, keeps its kind's name.
#[test]
fn a_file_pane_behind_another_is_a_tab_named_by_its_file() -> Check {
    let file = Address::new(Kind::File, "src/main.rs")?;
    assert_eq!(behind(PaneKind::Code, file)?, "main.rs");
    let child = Address::new(Kind::Agent, "a/one")?;
    assert_eq!(behind(PaneKind::Chat, child)?, "Chat");
    Ok(())
}

/// An ⌥⌘-click opens the file in a window of its own and leaves the main window as it was;
/// again, it stays out. A ⌘-click on a file out in its window opens no second pane.
#[test]
fn an_option_command_click_opens_the_file_in_a_window_of_its_own() -> Check {
    let mut runtime = settled("shell/everything")?;
    let viewer = one(&runtime, PaneKind::Code)?;
    let (slot, open) = (frame(&runtime, viewer)?, tabs(&runtime)?);
    let both = Mods { alt: true, ..cmd() };
    let out = |runtime: &Runtime<Win>, pane: PaneId| -> Result<_, Box<dyn Error>> {
        Ok(runtime.state.hub.try_borrow()?.model.grid.origin_of(pane))
    };
    for held in [both, both, cmd()] {
        tap_with(&mut runtime, MAIN_RS, held)?;
        let pinned = about(&runtime, "src/main.rs")?.ok_or("no pane about main.rs")?;
        assert_eq!(of(&runtime, PaneKind::Code)?, [viewer, pinned]);
        assert_eq!(out(&runtime, pinned)?, Some(root::MAIN), "out of the main");
        assert_eq!(frame(&runtime, viewer)?, slot, "which is as it was");
        assert_eq!(tabs(&runtime)?, open);
        assert_ne!(focus(&runtime)?, Some(pinned), "no pane out has the keys");
    }
    Ok(())
}

/// A split of the viewer puts the file it shows beside it, on the edge asked, in a pane
/// about that file: the viewer moves on and the pane beside keeps the file it was split for.
#[test]
fn a_split_of_the_viewer_puts_its_file_beside_and_the_two_part_ways() -> Check {
    let mut runtime = settled("shell/everything")?;
    let viewer = one(&runtime, PaneKind::Code)?;
    let was = crumb(&runtime, viewer)?;
    menu(&mut runtime, "code-", ".tab-")?;
    tap(&mut runtime, SPLIT_RIGHT)?;
    let pinned = about(&runtime, &was)?.ok_or("no pane about the viewer's file")?;
    let ([x, ..], [right, ..]) = (frame(&runtime, viewer)?, frame(&runtime, pinned)?);
    assert!(right > x, "right of the viewer");
    assert_eq!(crumb(&runtime, pinned)?, was);
    tap(&mut runtime, SESSIONS)?;
    assert_eq!(crumb(&runtime, viewer)?, "src/routes/sessions.rs");
    assert_eq!(crumb(&runtime, pinned)?, was, "the file it was split for");
    menu(&mut runtime, &format!("code-{}.", viewer.0), ".tab-")?;
    tap(&mut runtime, SPLIT_DOWN)?;
    let under = about(&runtime, "src/routes/sessions.rs")?.ok_or("no pane under")?;
    let ([x, y, ..], [below_x, below_y, ..]) = (frame(&runtime, viewer)?, frame(&runtime, under)?);
    assert!((below_x - x).abs() < 0.5 && below_y > y, "under the viewer");
    Ok(())
}
