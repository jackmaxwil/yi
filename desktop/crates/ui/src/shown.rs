//! The thread each window shows: picked in the sidebar, read by every pane of the window,
//! and kept open on the wire and on disk for as long as it shows.

use std::collections::{BTreeMap, BTreeSet};

use rune::TextEdit;
use std::path::PathBuf;

use yd_core::address::uri::Address;
use yd_core::command::table::{Route, Run, Target};
use yd_core::feed::{CHANGES, FILES, FeedEvent, LSP, OPEN_FILES, SourceCmd};
use yd_core::grid::saved::{BLANK, JUST_CHAT};
use yd_core::grid::step::GridMsg;
use yd_core::ids::{PaneId, RowId, ThreadId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::state::{Link, Wire};
use yd_core::reduce::step::commands::{CLOSE, OPEN};
use yd_core::reduce::threads::Thread;
use yd_core::row::{Rows, Value};
use yd_core::wire::request::RequestId;

use crate::term::view::state::Shells;
use crate::win::{Hub, Win, dispatch, field};

/// The composer's field in a pane: New chat puts refused words back into it.
pub const DRAFT: &str = "draft";

/// A window's view of a thread as the wire and the sources last heard of it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Open {
    thread: Option<ThreadId>,
    /// The folder the `fs` and `git` sources were pointed at.
    folder: String,
}

/// A window showing New chat in place of a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fresh {
    /// Nothing is sent yet.
    Open,
    /// Its first words went to the server as the request `ticket`; the thread born of that
    /// request shows, and a refusal puts `words` back in `pane`'s field.
    Sent {
        ticket: RequestId,
        pane: PaneId,
        words: String,
    },
}

/// Each window's pick and what it has open. It lives in the hub (`hub.panes`, under no
/// pane) so every pane reads it and a scene can set it.
#[derive(Debug, Default)]
pub struct Shown {
    picked: BTreeMap<WindowId, ThreadId>,
    open: BTreeMap<WindowId, Open>,
    fresh: BTreeMap<WindowId, Fresh>,
    /// The folder each window's New chat was pointed at, while it shows.
    chosen: BTreeMap<WindowId, String>,
    /// The checkout git named for a thread the server never said worked in one. Kept, since
    /// the git source drops its rows each time it is pointed at another folder.
    lanes: BTreeMap<ThreadId, String>,
    /// Threads whose Files, Changes and Code panes were asked to show the project folder.
    project: BTreeSet<ThreadId>,
    /// The thread (none for New chat) whose shells each window's Terminal panes hold now.
    owner: BTreeMap<WindowId, Option<ThreadId>>,
    /// Invariant: the shells of every other thread a pane has seen, running; one thread's
    /// shells are here or in the pane, never both.
    parked: BTreeMap<(PaneId, Option<ThreadId>), Shells>,
}

/// Whether `window` shows New chat: it was asked for, or the server's thread list came and
/// had no thread in it. Before the first list, no thread known is not yet no thread at all.
pub fn fresh(hub: &Hub, window: WindowId) -> bool {
    let shown = hub.panes.get::<Shown>(None);
    let asked = shown.is_some_and(|shown| shown.fresh.contains_key(&window));
    let wire = &hub.model.wire;
    let none = matches!(wire.link, Link::Up { .. }) && wire.listed && wire.threads.is_empty();
    asked || none
}

fn set_fresh(win: &mut Win, to: impl FnOnce(&Wire) -> Option<Fresh>) -> bool {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    let to = to(&hub.model.wire);
    let Some(shown) = hub.panes.get_mut::<Shown>(None) else {
        return false;
    };
    match to {
        Some(fresh) => shown.fresh.insert(window, fresh).is_some(),
        None => {
            shown.chosen.remove(&window);
            shown.fresh.remove(&window).is_some()
        }
    }
}

/// Shows New chat in this window: what `thread.new` with no words does.
pub fn start(win: &mut Win) {
    set_fresh(win, |_| Some(Fresh::Open));
}

/// Shows New chat in this window, its thread to start in `folder`.
pub fn choose(win: &mut Win, folder: String) {
    start(win);
    hand_over(win, None);
    restart_shells(win, Some(&folder));
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(shown) = hub.panes.get_mut::<Shown>(None)
    {
        shown.chosen.insert(win.window, folder);
    }
}

/// The folder New chat in `window` was pointed at, if one was chosen.
pub fn chosen(hub: &Hub, window: WindowId) -> Option<&str> {
    let shown = hub.panes.get::<Shown>(None)?;
    shown.chosen.get(&window).map(String::as_str)
}

/// New chat's first words are on their way: the thread they make shows once it exists.
/// Call it just before the run that sends `session/new`, so the ticket is that request's.
pub fn sent(win: &mut Win, pane: PaneId, words: String) {
    set_fresh(win, |wire| {
        let ticket = wire.ticket();
        Some(Fresh::Sent {
            ticket,
            pane,
            words,
        })
    });
}

/// Leaves New chat with nothing sent; false when it was not showing.
pub fn leave(win: &mut Win) -> bool {
    set_fresh(win, |_| None)
}

/// The thread New chat's words made, once the server has: it is picked, New chat goes and the
/// window shows its chat alone. Refused, New chat stays with the words back in its field.
fn made(win: &mut Win) {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let Hub { model, panes, .. } = &mut *hub;
    let Some(shown) = panes.get_mut::<Shown>(None) else {
        return;
    };
    let Some(Fresh::Sent { ticket, .. }) = shown.fresh.get(&window) else {
        return;
    };
    if let Some(born) = model.wire.born(ticket) {
        shown.picked.insert(window, born.id.clone());
        shown.fresh.remove(&window);
        shown.chosen.remove(&window);
        let saved = model.grid.window(window).and_then(|window| window.saved);
        drop(hub);
        // Just chat is already the chat alone, as a page of its own.
        if saved != Some(JUST_CHAT.slug) {
            dispatch(
                win,
                Msg::Grid(GridMsg::Show {
                    window,
                    saved: BLANK.slug,
                }),
            );
        }
    } else if model.wire.failed(ticket).is_some()
        && let Some(Fresh::Sent { pane, words, .. }) = shown.fresh.insert(window, Fresh::Open)
    {
        drop(hub);
        win.edits.insert(field(pane, DRAFT), TextEdit::new(&words));
    }
}

/// The listed threads that hang under no other listed thread, in list order.
pub fn roots(wire: &Wire) -> Vec<&Thread> {
    let top = |thread: &&Thread| {
        let parent = thread.forked_from.as_ref();
        parent.is_none_or(|id| !wire.threads.contains_key(id))
    };
    wire.listed().into_iter().filter(top).collect()
}

/// The thread `window` shows: the one picked there, else the first the list shows.
pub fn thread(hub: &Hub, window: WindowId) -> Option<&Thread> {
    let wire = &hub.model.wire;
    let shown = hub.panes.get::<Shown>(None);
    let picked = shown.and_then(|shown| shown.picked.get(&window));
    picked
        .and_then(|id| wire.threads.get(id))
        .or_else(|| roots(wire).into_iter().next())
}

/// The thread `pane` shows: its window's, or the first window's for a pane drawn alone.
pub fn of_pane(hub: &Hub, pane: PaneId) -> Option<&Thread> {
    let window = hub.model.grid.window_of(pane);
    thread(hub, window.unwrap_or(WindowId(0)))
}

/// Picks the thread `window` shows. The wire and the sources hear of it on the next [`sync`];
/// a handler calls [`show`], which does both.
pub fn pick(hub: &mut Hub, window: WindowId, id: ThreadId) {
    if let Some(shown) = hub.panes.get_mut::<Shown>(None) {
        shown.picked.insert(window, id);
    }
}

/// Drops what `window` picked and had open, as it closes, and gives the thread it had open
/// on the wire for the caller to close. The next window with the id starts with neither.
pub fn forget(hub: &mut Hub, window: WindowId) -> Option<ThreadId> {
    let shown = hub.panes.get_mut::<Shown>(None)?;
    shown.picked.remove(&window);
    shown.owner.remove(&window);
    shown.open.remove(&window)?.thread
}

/// Shows `id` in this window, from a handler; New chat, if it was showing, goes.
pub fn show(win: &mut Win, id: ThreadId) {
    leave(win);
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        pick(&mut hub, win.window, id);
    }
    sync(win);
}

/// The folder a thread works in as the server said: its own worktree, else where it was made.
pub fn folder(thread: &Thread) -> &str {
    let lane = thread.workdir.as_ref().map(|workdir| workdir.cwd.as_str());
    lane.unwrap_or(thread.cwd.as_str())
}

/// The folder a thread really works in: [`folder`], or the checkout git names for it.
pub fn works_in<'a>(hub: &'a Hub, thread: &'a Thread) -> &'a str {
    let lanes = hub.panes.get::<Shown>(None).map(|shown| &shown.lanes);
    let lane = lanes.and_then(|lanes| lanes.get(&thread.id));
    match (&thread.workdir, lane) {
        (None, Some(lane)) => lane,
        _ => folder(thread),
    }
}

/// Whether the panes of `thread` were asked to show its project folder, not its checkout.
pub fn in_project(hub: &Hub, thread: &ThreadId) -> bool {
    let shown = hub.panes.get::<Shown>(None);
    shown.is_some_and(|shown| shown.project.contains(thread))
}

/// The folder the Files, Changes and Code panes read for `thread`.
fn panes_in<'a>(hub: &'a Hub, thread: &'a Thread) -> &'a str {
    match in_project(hub, &thread.id) {
        true => thread.cwd.as_str(),
        false => works_in(hub, thread),
    }
}

/// Points the panes of the thread this window shows at its project folder, or back at the
/// agent's checkout, from a handler.
pub fn flip_project(win: &mut Win) {
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        let id = thread(&hub, win.window).map(|thread| thread.id.clone());
        if let (Some(id), Some(shown)) = (id, hub.panes.get_mut::<Shown>(None))
            && !shown.project.remove(&id)
        {
            shown.project.insert(id);
        }
    }
    sync(win);
}

/// Each thread's own checkout, as the server said it or git named it: what a restart keeps.
pub fn checkouts(hub: &Hub) -> Vec<(String, String)> {
    let said = hub.model.wire.threads.values().filter_map(|thread| {
        let workdir = thread.workdir.as_ref().filter(|w| w.lane.is_some())?;
        Some((thread.id.as_str().to_owned(), workdir.cwd.clone()))
    });
    let lanes = hub.panes.get::<Shown>(None).into_iter();
    let named = lanes.flat_map(|shown| shown.lanes.iter());
    let named = named.map(|(id, path)| (id.as_str().to_owned(), path.clone()));
    said.chain(named).collect()
}

/// Takes `path` as the checkout `thread` works in, as a restart remembers it.
pub fn remember(hub: &mut Hub, thread: ThreadId, path: String) {
    if let Some(shown) = hub.panes.get_mut::<Shown>(None) {
        shown.lanes.insert(thread, path);
    }
}

/// Incident: Yi tells only the client attached as a session attaches that it works in a
/// checkout of its own (`_yi/workdir`), so a second client reads it off git.
fn learn(hub: &mut Hub, window: WindowId) {
    let Hub { model, panes, .. } = hub;
    let picked = panes.get::<Shown>(None).and_then(|s| s.picked.get(&window));
    let thread = picked.and_then(|id| model.wire.threads.get(id));
    let Some(thread) = thread.or_else(|| roots(&model.wire).into_iter().next()) else {
        return;
    };
    let row = RowId::new(["worktree:", &thread.branch()].concat());
    let found = model.tables.get(CHANGES).and_then(|table| table.find(&row));
    let Some(path) = found.and_then(|row| row.words("path")) else {
        return;
    };
    // Invariant: Yi may hold a checkout for a session whose steps run in the project, so a
    // checkout is followed once it holds work: it differs from where it came from.
    if found.and_then(|row| row.get("changed")) != Some(&Value::Flag(true)) {
        return;
    }
    if let Some(shown) = panes.get_mut::<Shown>(None) {
        shown.lanes.insert(thread.id.clone(), path.to_owned());
    }
}

/// Gives this window's Terminal panes the shells of `to` and parks the ones they hold, still
/// running; a thread new to a pane takes the shells made before any thread, else gets one.
fn hand_over(win: &mut Win, to: Option<ThreadId>) -> bool {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    let Hub { model, panes, .. } = &mut *hub;
    let held = panes.get::<Shown>(None).and_then(|s| s.owner.get(&window));
    let held = held.cloned().unwrap_or_default();
    if held == to {
        return false;
    }
    let mine = |pane: &&yd_core::grid::windows::Pane| {
        pane.kind == PaneKind::Terminal && model.grid.window_of(pane.id) == Some(window)
    };
    let mut took = false;
    for pane in model.grid.panes.iter().filter(mine) {
        let Some(shown) = panes.get_mut::<Shown>(None) else {
            return false;
        };
        let next = shown.parked.remove(&(pane.id, to.clone()));
        if next.is_none() && held.is_none() {
            took = true;
            continue;
        }
        let Some(shells) = panes.get_mut::<Shells>(Some(pane.id)) else {
            continue;
        };
        let old = std::mem::replace(shells, next.unwrap_or_default());
        if let Some(shown) = panes.get_mut::<Shown>(None) {
            shown.parked.insert((pane.id, held.clone()), old);
        }
    }
    if let Some(shown) = panes.get_mut::<Shown>(None) {
        shown.owner.insert(window, to);
    }
    took
}

/// Hangs up the shells of this window's Terminal panes that started outside `folder` (every
/// one when none), and gives each pane a new shell in the folder the window now works in.
pub fn restart_shells(win: &mut Win, folder: Option<&str>) {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let Hub { model, panes, .. } = &mut *hub;
    let mine = |pane: &&yd_core::grid::windows::Pane| {
        pane.kind == PaneKind::Terminal && model.grid.window_of(pane.id) == Some(window)
    };
    let mut hung = Vec::new();
    for pane in model.grid.panes.iter().filter(mine) {
        let Some(shells) = panes.get_mut::<Shells>(Some(pane.id)) else {
            continue;
        };
        let elsewhere = |cwd: &str| folder.is_none_or(|folder| !cwd.is_empty() && cwd != folder);
        if shells.tabs.iter().any(|shell| elsewhere(&shell.cwd)) {
            let old = std::mem::take(shells);
            hung.extend(old.tabs.into_iter().map(|shell| shell.feed));
        }
    }
    drop(hub);
    for feed in hung {
        dispatch(win, Msg::Source(feed, SourceCmd::Close));
    }
}

/// Closes the thread this window showed, opens the one it shows now and points the `fs` and
/// `git` sources at its folder; true when anything was sent. The window's poll calls it.
pub fn sync(win: &mut Win) -> bool {
    made(win);
    let window = win.window;
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        learn(&mut hub, window);
    }
    let (was, now) = {
        let Ok(hub) = win.hub.try_borrow() else {
            return false;
        };
        let shown = hub.panes.get::<Shown>(None);
        let was = shown.and_then(|shown| shown.open.get(&window)).cloned();
        let was = was.unwrap_or_default();
        let thread = thread(&hub, window);
        // A thread just picked that the list never named, such as a child agent's session:
        // opening it by id is what makes it known.
        let picked = shown.and_then(|shown| shown.picked.get(&window));
        let known = |id: &&ThreadId| hub.model.wire.threads.contains_key(*id);
        let new = picked.filter(|id| !known(id) && was.thread.as_ref() != Some(*id));
        let now = Open {
            thread: new.cloned().or(thread.map(|thread| thread.id.clone())),
            // New chat's `@` lists the project it will start in, not the thread behind it.
            folder: chosen(&hub, window)
                .filter(|_| fresh(&hub, window))
                .or(thread.map(|thread| panes_in(&hub, thread)))
                .unwrap_or_default()
                .to_owned(),
        };
        (was, now)
    };
    let mine = {
        let Ok(hub) = win.hub.try_borrow() else {
            return false;
        };
        (!fresh(&hub, window)).then(|| now.thread.clone()).flatten()
    };
    if hand_over(win, mine) {
        restart_shells(win, Some(&now.folder));
    }
    if was == now {
        return false;
    }
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(shown) = hub.panes.get_mut::<Shown>(None)
    {
        shown.open.insert(window, now.clone());
    }
    if was.thread != now.thread {
        for (command, id) in [(CLOSE, &was.thread), (OPEN, &now.thread)] {
            let thread = id
                .as_ref()
                .and_then(|id| Address::new(Kind::Thread, id.as_str()).ok());
            if let Some(thread) = thread {
                let run = Run::new(command).on(Target::Thing(thread));
                dispatch(win, Msg::Command(Route::Wire, run));
            }
        }
        // Incident: a second thread kept the first one's file tabs. They belong to a thread;
        // the first look at one has nothing of another's to drop.
        if was.thread.is_some() {
            dispatch(win, Msg::Feed(OPEN_FILES, FeedEvent::Reset(Vec::new())));
        }
    }
    if was.folder != now.folder && !now.folder.is_empty() {
        for feed in [FILES, CHANGES, LSP] {
            let root = SourceCmd::Root(PathBuf::from(&now.folder));
            dispatch(win, Msg::Source(feed, root));
        }
        // The source drops what it read when it is pointed elsewhere: a tab kept through the
        // move, as when a checkout is found for the thread, is asked for again.
        let open = win.hub.try_borrow().ok().and_then(|hub| {
            let table = hub.model.tables.get(OPEN_FILES)?;
            let paths = (0..table.len()).filter_map(|at| table.get(at)?.words("path"));
            Some(paths.map(PathBuf::from).collect::<Vec<_>>())
        });
        for path in open.unwrap_or_default() {
            dispatch(win, Msg::Source(FILES, SourceCmd::Read(path)));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::error::Error;
    use std::rc::Rc;

    use yd_core::feed::FeedEvent;
    use yd_core::msg::{Effect, step};
    use yd_core::reduce::step::commands::NEW;
    use yd_core::row::Row;
    use yd_core::wire::replay::{play, play_line};

    use crate::pane::SCENE_PANE;

    use super::*;

    /// A real server's list of six threads; five are known only from the list.
    const LISTED: &str = include_str!("../../../fixtures/wire/lists.jsonl");
    const WINDOW: WindowId = WindowId(0);

    type Asked = Rc<RefCell<Vec<Effect>>>;

    #[test]
    fn new_chat_waits_for_the_first_thread_list_before_it_shows() -> Result<(), Box<dyn Error>> {
        let hub = Hub::new(&[], Vec::new());
        let mut hub = hub.try_borrow_mut()?;
        let up: Vec<&str> = LISTED.lines().take(2).collect();
        play(&mut hub.model, &up.join("\n"))?;
        assert!(matches!(hub.model.wire.link, Link::Up { .. }));
        assert!(
            !fresh(&hub, WINDOW),
            "up, and no list yet: no thread is not yet known"
        );
        Ok(())
    }

    fn window() -> Result<(Win, Asked), Box<dyn Error>> {
        let hub = Hub::new(&[], Vec::new());
        let asked = Asked::default();
        {
            let mut hub = hub.try_borrow_mut()?;
            let listed: Vec<&str> = LISTED.lines().take(29).collect();
            play(&mut hub.model, &listed.join("\n"))?;
            let log = Rc::clone(&asked);
            hub.set_sink(move |effect| log.borrow_mut().push(effect));
        }
        Ok((Win::new(hub, WINDOW), asked))
    }

    fn views(win: &Win, id: &ThreadId) -> Result<u32, Box<dyn Error>> {
        let hub = win.hub.try_borrow()?;
        Ok(hub.model.wire.threads.get(id).ok_or("no thread")?.views)
    }

    fn roots_sent(asked: &Asked) -> Vec<(yd_core::ids::FeedId, PathBuf)> {
        let asked = asked.borrow();
        let roots = asked.iter().filter_map(|effect| match effect {
            Effect::Source(feed, SourceCmd::Root(folder)) => Some((*feed, folder.clone())),
            _ => None,
        });
        roots.collect()
    }

    #[test]
    fn the_thread_a_window_shows_is_opened_once_and_the_sources_get_its_folder()
    -> Result<(), Box<dyn Error>> {
        let (mut win, asked) = window()?;
        let (first, folder) = {
            let hub = win.hub.try_borrow()?;
            let shown = thread(&hub, WINDOW).ok_or("the list shows no thread")?;
            (shown.id.clone(), PathBuf::from(works_in(&hub, shown)))
        };
        assert_eq!(
            views(&win, &first)?,
            0,
            "nothing shows it before the window does"
        );
        assert!(sync(&mut win), "the first look opens it");
        assert_eq!(views(&win, &first)?, 1);
        assert_eq!(
            roots_sent(&asked),
            [
                (FILES, folder.clone()),
                (CHANGES, folder.clone()),
                (LSP, folder)
            ]
        );
        assert!(!sync(&mut win), "a second look sends nothing");
        assert_eq!(views(&win, &first)?, 1, "and opens nothing twice");
        Ok(())
    }

    const ASK: &str = include_str!("../../../fixtures/wire/approval-allow.jsonl");
    const LANE: &str = "/Users/someone/.yi/lanes/5e1f0c2a/0";

    /// The recorded session as a second client hears it, which is never `_yi/workdir`, then
    /// git's row for its checkout, which has the session's work in it or not.
    fn checkout(changed: bool) -> Result<(Win, Asked), Box<dyn Error>> {
        let shared = Hub::new(&[], Vec::new());
        let asked = Asked::default();
        let heard: Vec<&str> = ASK.lines().filter(|l| !l.contains("_yi/workdir")).collect();
        {
            let mut hub = shared.try_borrow_mut()?;
            play(&mut hub.model, &heard.join("\n"))?;
            let log = Rc::clone(&asked);
            hub.set_sink(move |effect| log.borrow_mut().push(effect));
        }
        let mut win = Win::new(shared, WINDOW);
        sync(&mut win);
        let mut hub = win.hub.try_borrow_mut()?;
        let id = thread(&hub, WINDOW).ok_or("no thread")?.id.clone();
        let row = Row::new(format!("worktree:yi/{}", id.as_str())).text("path", LANE);
        let row = row.cell("changed", Value::Flag(changed));
        step(&mut hub.model, Msg::Feed(CHANGES, FeedEvent::Upsert(row)));
        drop(hub);
        asked.borrow_mut().clear();
        Ok((win, asked))
    }

    #[test]
    fn a_thread_with_work_in_a_checkout_of_its_own_roots_the_sources_there()
    -> Result<(), Box<dyn Error>> {
        let (mut win, asked) = checkout(true)?;
        assert!(sync(&mut win));
        let lane = PathBuf::from(LANE);
        let roots = [(FILES, lane.clone()), (CHANGES, lane.clone()), (LSP, lane)];
        assert_eq!(roots_sent(&asked), roots);
        // The git source reads the checkout afresh and its rows go a moment: the thread stays.
        let reset = Msg::Feed(CHANGES, FeedEvent::Reset(Vec::new()));
        step(&mut win.hub.try_borrow_mut()?.model, reset);
        asked.borrow_mut().clear();
        assert!(!sync(&mut win), "nothing is pointed back at the project");
        assert_eq!(roots_sent(&asked), []);
        Ok(())
    }

    /// Dies with a file tab that waits for ever: the files source drops what it read when the
    /// checkout is found, and the tab, still open, was never asked for again.
    #[test]
    fn a_file_open_when_the_checkout_is_found_is_read_again() -> Result<(), Box<dyn Error>> {
        let (mut win, asked) = checkout(true)?;
        let file = Address::new(Kind::File, "src/lib.rs")?;
        let run = Run::new(yd_core::command::table::OPEN_FILE).on(Target::Thing(file));
        dispatch(&mut win, Msg::Command(Route::Model, run));
        asked.borrow_mut().clear();
        assert!(sync(&mut win));
        let reads = asked.borrow().iter().filter(|effect| {
            matches!(effect, Effect::Source(FILES, SourceCmd::Read(path)) if path.ends_with("src/lib.rs"))
        }).count();
        assert_eq!(reads, 1, "{:?}", asked.borrow());
        Ok(())
    }

    #[test]
    fn new_chat_pointed_at_a_project_lets_at_find_that_projects_files() -> Result<(), Box<dyn Error>>
    {
        let (mut win, asked) = window()?;
        sync(&mut win);
        asked.borrow_mut().clear();
        choose(&mut win, "/Users/someone/tally".to_owned());
        assert!(sync(&mut win));
        let chosen = PathBuf::from("/Users/someone/tally");
        let roots = [
            (FILES, chosen.clone()),
            (CHANGES, chosen.clone()),
            (LSP, chosen),
        ];
        assert_eq!(
            roots_sent(&asked),
            roots,
            "the sources read the chosen folder"
        );
        Ok(())
    }

    /// Dies with Yi's lane held for a session whose own steps ran in the project.
    #[test]
    fn a_checkout_with_nothing_in_it_is_not_followed() -> Result<(), Box<dyn Error>> {
        let (mut win, asked) = checkout(false)?;
        sync(&mut win);
        assert_eq!(roots_sent(&asked), []);
        Ok(())
    }

    #[test]
    fn picking_another_thread_closes_the_old_view_and_opens_the_new() -> Result<(), Box<dyn Error>>
    {
        let (mut win, asked) = window()?;
        sync(&mut win);
        let (first, other) = {
            let hub = win.hub.try_borrow()?;
            let mut listed = roots(&hub.model.wire).into_iter();
            let first = listed.next().ok_or("no thread")?.id.clone();
            (first, listed.next().ok_or("one thread only")?.id.clone())
        };
        asked.borrow_mut().clear();
        show(&mut win, other.clone());
        assert_eq!((views(&win, &first)?, views(&win, &other)?), (0, 1));
        let hub = win.hub.try_borrow()?;
        assert_eq!(thread(&hub, WINDOW).map(|thread| &thread.id), Some(&other));
        assert_eq!(
            of_pane(&hub, PaneId(9)).map(|thread| &thread.id),
            Some(&other)
        );
        let resumed = asked.borrow().iter().any(|effect| match effect {
            Effect::Wire(request) => request.line.contains("session/resume"),
            _ => false,
        });
        assert!(resumed, "the thread now shown is resumed on the wire");
        Ok(())
    }

    /// Sends New chat's words as the composer does and gives the id `session/new` went with.
    fn send_new(win: &mut Win, asked: &Asked) -> Result<u64, Box<dyn Error>> {
        start(win);
        sent(win, SCENE_PANE, "make it".to_owned());
        let place = Address::new(Kind::Place, "/work")?;
        let run = Run {
            text: Some("make it".to_owned()),
            ..Run::new(NEW).on(Target::Thing(place))
        };
        dispatch(win, Msg::Command(Route::Wire, run));
        let asked = asked.borrow();
        let new = asked.iter().rev().find_map(|effect| match effect {
            Effect::Wire(request) if request.line.contains("session/new") => request.id.clone(),
            _ => None,
        });
        match new.ok_or("no session/new was sent")? {
            RequestId::Number(n) => Ok(n),
            RequestId::Text(_) => Err("session/new went with a text id".into()),
        }
    }

    fn answer(win: &Win, line: &str) -> Result<(), Box<dyn Error>> {
        let mut hub = win.hub.try_borrow_mut()?;
        play_line(
            &mut hub.model,
            &format!(r#"{{"dir":"recv","frame":{line}}}"#),
        )?;
        Ok(())
    }

    #[test]
    fn new_chat_shows_the_thread_its_own_call_made_not_one_listed_meanwhile()
    -> Result<(), Box<dyn Error>> {
        let (mut win, asked) = window()?;
        let id = send_new(&mut win, &asked)?;
        {
            let mut hub = win.hub.try_borrow_mut()?;
            let other = ThreadId::new("000-made-by-the-cli");
            hub.model
                .wire
                .threads
                .insert(other.clone(), Thread::new(other));
        }
        sync(&mut win);
        assert!(
            fresh(&*win.hub.try_borrow()?, WINDOW),
            "another client's thread is not ours"
        );
        let made = r#"{"sessionId":"zzz-ours"}"#;
        answer(
            &win,
            &format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{made}}}"#),
        )?;
        sync(&mut win);
        let hub = win.hub.try_borrow()?;
        assert!(!fresh(&hub, WINDOW));
        let shows = thread(&hub, WINDOW).map(|thread| thread.id.as_str());
        assert_eq!(shows, Some("zzz-ours"));
        Ok(())
    }

    #[test]
    fn a_refused_new_chat_stays_open_with_its_words_back() -> Result<(), Box<dyn Error>> {
        let (mut win, asked) = window()?;
        win.edits.entry(field(SCENE_PANE, DRAFT)).or_default();
        let id = send_new(&mut win, &asked)?;
        let refused = r#"{"code":-32000,"message":"no model"}"#;
        answer(
            &win,
            &format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{refused}}}"#),
        )?;
        sync(&mut win);
        assert!(fresh(&*win.hub.try_borrow()?, WINDOW));
        let draft = win
            .edits
            .get(&field(SCENE_PANE, DRAFT))
            .map(|edit| edit.text.as_str());
        assert_eq!(draft, Some("make it"));
        let shown = win
            .hub
            .try_borrow()?
            .panes
            .get::<Shown>(None)
            .map(|s| s.fresh.get(&WINDOW).cloned());
        assert_eq!(
            shown,
            Some(Some(Fresh::Open)),
            "a second send is not waited on as the first"
        );
        Ok(())
    }
}
