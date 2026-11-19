//! The command box: commands, threads and files in one search. Also the one way the app runs
//! a command by id, the shortcut matcher of the window root, and the layer both menus draw in.

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::BTreeMap;

use rune::{Chord, Cx, El, Key, KeyEvent, TextEdit, Tx, col, el, text, text_input};
use rune_kit::{Placement, PopupItem, PopupList, popover};
use yd_core::address::uri::Address;
use yd_core::command::by_kind::COPY_ADDRESS;
use yd_core::command::keys::Map;
use yd_core::command::table::{Command, Commands, Menu, OPEN_COMMANDS, Run, SHOW_KEYS, Target};
use yd_core::command::table::{declared, keycap};
use yd_core::feed::FILES;
use yd_core::ids::{CommandId, ThreadId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::reduce::step::commands::{ALLOW, ALWAYS, DENY, NEW};
use yd_core::row::Rows;

use crate::grid_view;
use crate::kit::icons::{SEARCH, glyph};
use crate::kit::key_hint::key_hint;
use crate::menu::context;
use crate::pane::{Scene, Stage};
use crate::shown;
use crate::theme::look::{BODY, META, colour, kit};
use crate::theme::scale::POPOVER;
use crate::win::{Field, Hub, Win, dispatch};

pub const SCENES: &[Scene] = &[
    Scene {
        name: "menu/command-box",
        stage: Stage::View(scene),
        setup: |hub| {
            borrow(hub, &["shell/mockup", "files/mockup"]);
            open_in(hub, WindowId(0), "rate");
        },
    },
    Scene {
        name: "keys/help",
        stage: Stage::View(scene),
        setup: |hub| {
            open_in(hub, WindowId(0), "");
            if let Some(boxed) = hub
                .panes
                .get_mut::<Boxes>(None)
                .and_then(|boxes| boxes.0.get_mut(&WindowId(0)))
            {
                boxed.keys = true;
            }
        },
    },
];

/// Runs the setups of other scenes the app registers, by name: how a menu's scene gets the
/// threads, files and changes the panes' own mockup scenes build from fixtures.
pub fn borrow(hub: &mut Hub, names: &[&str]) {
    for name in names {
        let scene = hub.scenes.iter().find(|scene| scene.name == *name).copied();
        if let Some(scene) = scene {
            (scene.setup)(hub);
        }
    }
}

pub const KEY: &str = "window.command-box";
const FIELD: &str = "window.command-box.field";
const LIST: &str = "window.command-box.list";
const WIDTH: f32 = 420.0;
/// Files are many: past this the box lists no more matches.
// ponytail: a substring scan of the files table; an index if a tree outgrows it.
const FILE_ROWS: usize = 200;

/// What a window carries out itself, such as the shell's sidebar; the app installs it.
pub type Carry = fn(&mut Win, &mut Tx, CommandId, &Target) -> bool;

/// The commands the app's window carries out, and the function that does.
#[derive(Debug, Default)]
pub struct WindowCommands {
    pub ids: &'static [CommandId],
    pub carry: Option<Carry>,
}

/// Lets the app carry out `ids`, the commands only its window can: tried before the route.
pub fn install(hub: &mut Hub, ids: &'static [CommandId], carry: Carry) {
    if let Some(kept) = hub.panes.get_mut::<WindowCommands>(None) {
        *kept = WindowCommands {
            ids,
            carry: Some(carry),
        };
    }
}

/// Whether a command is carried out by the window rather than by its route.
fn windowed(hub: &Hub, id: CommandId) -> bool {
    let installed = hub.panes.get::<WindowCommands>(None);
    let grid = grid_view::view::commands().any(|command| command.id == id);
    [OPEN_COMMANDS, SHOW_KEYS, COPY_ADDRESS].contains(&id)
        || grid
        || installed.is_some_and(|kept| kept.ids.contains(&id))
}

/// The Mac menu bar, from the table: what the task that draws it reads.
pub fn menu_bar(hub: &Hub) -> Vec<Menu> {
    let built = |command: &Command| Commands::built(command, |id| windowed(hub, id));
    table(hub).menus(built)
}

/// Every command of the app: `yd-core`'s, the grid's and each pane's.
pub fn table(hub: &Hub) -> Commands {
    let panes = hub.defs.iter().flat_map(|def| def.commands);
    Commands::new(declared().chain(grid_view::view::commands()).chain(panes))
}

/// The shortcut map in force: the table's keys with the user's `keys.*` settings over them.
pub fn keymap(hub: &Hub) -> Map {
    Map::new(&table(hub), &hub.model.settings, |text| {
        text.parse::<Chord>().is_ok()
    })
}

/// What running a command came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    Done,
    /// It is listed, and nothing carries it out yet.
    NotBuilt(&'static str),
    /// No table declares it.
    Unknown,
}

/// Runs a command from a key, a menu or the box: the box and the grid first, then what the
/// window carries, then the route the table gives it.
pub fn run(win: &mut Win, tx: &mut Tx, run: Run) -> Ran {
    let id = run.command;
    if id == OPEN_COMMANDS || id == SHOW_KEYS {
        open(win, id == SHOW_KEYS);
        // Incident: opened from the top bar, the box left the keyboard with the composer,
        // and the words typed for it went to the chat.
        tx.focus_when_shown(FIELD, false);
        return Ran::Done;
    }
    if let (COPY_ADDRESS, Target::Thing(address)) = (id, &run.target) {
        tx.copy(address.to_string());
        return Ran::Done;
    }
    let (carry, own, found) = match win.hub.try_borrow() {
        Ok(hub) => {
            let kept = hub.panes.get::<WindowCommands>(None);
            let own = kept.is_some_and(|kept| kept.ids.contains(&id));
            (kept.and_then(|kept| kept.carry), own, table(&hub).find(id))
        }
        Err(_) => return Ran::Unknown,
    };
    // A command the window names is its own even where the grid would take it: Settings
    // and Themes are shown in a window, not docked.
    if own && carry.is_some_and(|carry| carry(win, tx, id, &run.target)) {
        return Ran::Done;
    }
    if grid_view::view::run(win, tx, id) {
        return Ran::Done;
    }
    if carry.is_some_and(|carry| carry(win, tx, id, &run.target)) {
        return Ran::Done;
    }
    match found {
        None => Ran::Unknown,
        Some(command) if !Commands::built(command, |_| false) => Ran::NotBuilt(command.title),
        Some(command) => {
            dispatch(win, Msg::Command(command.route, run));
            Ran::Done
        }
    }
}

/// The window root's keys: Esc closes an open menu, then the shortcut map runs its command.
/// A focused list or text box hears its keys first, so it keeps arrows, Return and Esc.
pub fn keys(win: &mut Win, event: &KeyEvent, tx: &mut Tx) -> bool {
    if event.key == Key::Escape && (close(win) || context::close(win)) {
        return true;
    }
    let map = match win.hub.try_borrow() {
        Ok(hub) => keymap(&hub),
        Err(_) => return false,
    };
    let pressed = |(key, _): &(&str, CommandId)| {
        let chord = key.parse::<Chord>().ok();
        chord.is_some_and(|chord| chord.matches_tx(event, tx))
    };
    let hit = map.bound().find(pressed).map(|(_, id)| id);
    let Some(id) = hit else { return false };
    let mut asked = Run::new(id);
    // Incident: Return on New chat answered another thread's ask, with no card in view. A key
    // answers only the ask of the thread this window shows.
    if [ALLOW, ALWAYS, DENY].contains(&id) {
        let thread = win.hub.try_borrow().ok().and_then(|hub| {
            let shown = shown::thread(&hub, win.window).filter(|_| !shown::fresh(&hub, win.window));
            shown?.address().ok()
        });
        let Some(thread) = thread else { return false };
        asked = asked.on(Target::Thing(thread));
    }
    run(win, tx, asked) != Ran::Unknown
}

/// The window's menus over `base`: the command box and the right-click menu, when open.
pub fn layer(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> Vec<El> {
    let mut layer = context::view(win, hub, cx);
    grid_view::picker::layer(win, hub, cx, &mut layer);
    view(win, hub, &mut layer);
    layer
}

/// One window's open box: what is typed, the row Return runs, and what the last run said.
#[derive(Debug, Clone, Default)]
pub struct Boxed {
    pub query: String,
    pub active: usize,
    pub said: Option<String>,
    /// It lists the shortcut map instead of what a search finds.
    pub keys: bool,
}

#[derive(Debug, Default)]
pub struct Boxes(pub BTreeMap<WindowId, Boxed>);

/// The search field among the window's own: it belongs to no pane.
const QUERY: Field = (None, "command-box");

fn opened(hub: &Hub, window: WindowId) -> Option<&Boxed> {
    hub.panes.get::<Boxes>(None)?.0.get(&window)
}

pub fn open_in(hub: &mut Hub, window: WindowId, query: &str) {
    if let Some(boxes) = hub.panes.get_mut::<Boxes>(None) {
        let boxed = Boxed {
            query: query.to_owned(),
            ..Boxed::default()
        };
        boxes.0.insert(window, boxed);
    }
}

/// Opens the box in this window, empty, as the search or as the list of shortcuts; a
/// second open of the same leaves it as it is.
pub fn open(win: &mut Win, keys: bool) {
    let window = win.window;
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        let same = opened(&hub, window).is_some_and(|boxed| boxed.keys == keys);
        if !same {
            open_in(&mut hub, window, "");
            if let Some(boxed) = hub
                .panes
                .get_mut::<Boxes>(None)
                .and_then(|boxes| boxes.0.get_mut(&window))
            {
                boxed.keys = keys;
            }
        }
    }
    win.edits.remove(&QUERY);
}

/// Closes the box; false when it was not open.
pub fn close(win: &mut Win) -> bool {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    let gone = hub
        .panes
        .get_mut::<Boxes>(None)
        .and_then(|boxes| boxes.0.remove(&window));
    gone.is_some()
}

fn change(win: &mut Win, edit: impl FnOnce(&mut Boxed)) {
    let window = win.window;
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(boxed) = hub
            .panes
            .get_mut::<Boxes>(None)
            .and_then(|b| b.0.get_mut(&window))
    {
        edit(boxed);
    }
}

fn field(win: &mut Win) -> &mut TextEdit {
    let window = win.window;
    let seed = win.hub.try_borrow().ok().and_then(|hub| {
        let query = opened(&hub, window).map(|boxed| boxed.query.clone());
        query.map(TextEdit::new)
    });
    let field = win.edits.entry(QUERY);
    field.or_insert_with(|| seed.unwrap_or_default())
}

/// One row the box can run.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// The command, its title and the key in force for it.
    Command(CommandId, Cow<'static, str>, Option<String>),
    Thread(ThreadId, String),
    File(String),
}

impl Found {
    fn id(&self) -> String {
        match self {
            Found::Command(id, ..) => format!("command:{}", id.0),
            Found::Thread(id, _) => format!("thread:{id}"),
            Found::File(path) => format!("file:{path}"),
        }
    }

    fn title(&self) -> &str {
        match self {
            Found::Command(_, title, _) => title,
            Found::Thread(_, title) => title,
            Found::File(path) => path,
        }
    }
}

/// What `query` finds, in order: commands aimed at the app, threads by title, then files by
/// path. Every word must be in the name, in any case.
pub fn search(hub: &Hub, query: &str) -> Vec<Found> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let fits = |name: &str| {
        let name = name.to_lowercase();
        words.iter().all(|word| name.contains(word.as_str()))
    };
    let table = table(hub);
    let map = keymap(hub);
    // The window carries New Chat with no target, so the box offers it with the app's own.
    let commands = table.iter().filter(|c| c.kinds.is_empty() || c.id == NEW);
    let titled = commands.filter_map(|command| {
        let title = grid_view::picker::title(&hub.model.grid, command)?;
        fits(&title).then_some((command, title))
    });
    let mut found: Vec<Found> = titled
        .map(|(command, title)| {
            let key = map.key(command.id).map(str::to_owned);
            Found::Command(command.id, title, key)
        })
        .collect();
    for thread in hub.model.wire.listed() {
        let title = thread
            .title
            .clone()
            .unwrap_or_else(|| thread.id.to_string());
        if fits(&title) {
            found.push(Found::Thread(thread.id.clone(), title));
        }
    }
    let files = hub.model.tables.get(FILES);
    let rows = files
        .into_iter()
        .flat_map(|t| (0..t.len()).filter_map(|at| t.get(at)));
    let paths = rows.filter(|row| row.words("kind") == Some("file"));
    let paths = paths.map(|row| row.id.as_str()).filter(|path| fits(path));
    found.extend(
        paths
            .take(FILE_ROWS)
            .map(|path| Found::File(path.to_owned())),
    );
    found
}

/// The shortcut map as rows, in the order of the Mac menus, whose title, menu or key has
/// every word of `query`.
pub fn sheet(hub: &Hub, query: &str) -> Vec<(Found, &'static str)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let (map, table) = (keymap(hub), table(hub));
    let mut rows = Vec::new();
    for (group, listed) in map.groups() {
        for row in listed {
            let command = table.find(row.command);
            let title = command.map_or(Some(Cow::Borrowed(row.title)), |command| {
                grid_view::picker::title(&hub.model.grid, command)
            });
            let Some(title) = title else { continue };
            let key = row.key.clone();
            let said = [
                &title,
                group,
                &key.as_deref().map(keycap).unwrap_or_default(),
            ]
            .join(" ")
            .to_lowercase();
            if words.iter().all(|word| said.contains(word.as_str())) {
                rows.push((Found::Command(row.command, title, key), group));
            }
        }
    }
    rows
}

/// Runs the row picked: a command, a thread to show, a file to open.
fn choose(win: &mut Win, tx: &mut Tx, found: Found) {
    let ran = match found {
        Found::Command(id, ..) => run(win, tx, Run::new(id)),
        Found::Thread(id, _) => {
            shown::show(win, id);
            Ran::Done
        }
        Found::File(path) => match Address::new(Kind::File, path) {
            Ok(address) => {
                grid_view::view::open_file(win, tx, address);
                Ran::Done
            }
            Err(_) => Ran::Unknown,
        },
    };
    match ran {
        Ran::Done => {
            close(win);
        }
        Ran::NotBuilt(title) => change(win, |b| b.said = Some(format!("{title} is not built yet"))),
        Ran::Unknown => change(win, |b| {
            b.said = Some("Nothing carries this out".to_owned())
        }),
    }
}

fn found_now(win: &Win) -> Vec<Found> {
    let Ok(hub) = win.hub.try_borrow() else {
        return Vec::new();
    };
    let Some(boxed) = opened(&hub, win.window) else {
        return Vec::new();
    };
    if boxed.keys {
        let listed = sheet(&hub, &boxed.query);
        return listed.into_iter().map(|(found, _)| found).collect();
    }
    search(&hub, &boxed.query)
}

/// Return runs the active row, arrows move it, Esc closes the box.
fn typed(win: &mut Win, event: &KeyEvent, tx: &mut Tx) -> bool {
    let found = found_now(win);
    let last = found.len().saturating_sub(1);
    match event.key {
        Key::Escape => {
            close(win);
        }
        Key::Down => change(win, |b| b.active = b.active.saturating_add(1).min(last)),
        Key::Up => change(win, |b| b.active = b.active.saturating_sub(1)),
        Key::Enter => {
            let active = win
                .hub
                .try_borrow()
                .ok()
                .and_then(|hub| opened(&hub, win.window).map(|boxed| boxed.active));
            if let Some(row) = found.get(active.unwrap_or(0)).cloned() {
                choose(win, tx, row);
            }
        }
        _ => return false,
    }
    true
}

fn edited(win: &mut Win) {
    let query = field(win).text.clone();
    change(win, |boxed| {
        boxed.query = query;
        boxed.active = 0;
        boxed.said = None;
    });
}

/// The box under the window's top edge: the field, then the rows found, on Rune's popover.
fn view(win: &Win, hub: &Hub, layer: &mut Vec<El>) {
    let Some(boxed) = opened(hub, win.window) else {
        return;
    };
    let tokens = &hub.model.tokens;
    let mut k = kit(tokens);
    // Incident: the stock raised fill is white at 8% alpha, so page text showed through.
    k.c.surface_raised = colour(POPOVER.of(tokens));
    let hole = match boxed.keys {
        true => "Filter the keyboard shortcuts",
        false => "Search commands, threads and files",
    };
    let seed = TextEdit::new(boxed.query.as_str());
    let edit = win.edits.get(&QUERY);
    let input = text_input(edit.unwrap_or(&seed), BODY)
        .key(FIELD)
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .color(colour(tokens.fg))
        .placeholder(hole, colour(tokens.faint))
        .label(hole)
        .autofocus()
        .on_edit(field)
        .on_input_change(|win: &mut Win, _tx: &mut Tx| edited(win))
        .on_input_key(typed);
    let said = boxed.said.as_deref().map(|said| {
        text(said)
            .key(format!("{KEY}.said"))
            .font(META)
            .color(colour(tokens.faint))
    });
    let surface = col()
        .key(KEY)
        .w(WIDTH)
        .padding(6.0)
        .gap(4.0)
        .child(
            rune::row()
                .w_full()
                .gap(6.0)
                .center_y()
                .child(glyph(&SEARCH, 11.0, tokens.faint))
                .child(input),
        )
        .children(said);
    // The box hangs from the top centre of the window, where the top bar's own box sits.
    let mount = col()
        .key(format!("{KEY}.mount"))
        .absolute(0.0, 0.0)
        .w_full()
        .pt(30.0)
        .items_center()
        .child(popover::raised(&k, surface, k.radius.md));
    layer.push(mount);
    let listed: Vec<(Found, &str)> = match boxed.keys {
        true => sheet(hub, &boxed.query),
        false => search(hub, &boxed.query)
            .into_iter()
            .map(|f| (f, ""))
            .collect(),
    };
    let rows = listed.into_iter().map(|(found, group)| {
        let hint = match &found {
            Found::Command(_, _, Some(key)) => Some(key_hint(tokens, &keycap(key))),
            Found::Command(..) | Found::File(_) => None,
            Found::Thread(..) => Some(text("thread").font(META).color(colour(tokens.faint))),
        };
        let item = PopupItem::new(found.id(), found.title());
        let item = match group {
            "" => item,
            group => item.description(group),
        };
        match hint {
            Some(hint) => item.trailing(hint),
            None => item,
        }
    });
    PopupList::new(LIST, KEY)
        .placement(Placement::BottomStart)
        .offset(4.0)
        .open(true)
        .width(WIDTH)
        .max_height(320.0)
        .items(rows)
        .active(Some(boxed.active))
        .empty_message("Nothing matches")
        .on_active(|win: &mut Win, at| change(win, |b| b.active = at))
        .on_choose(|win: &mut Win, id: &str, tx: &mut Tx| {
            let row = found_now(win).into_iter().find(|found| found.id() == id);
            if let Some(row) = row {
                choose(win, tx, row);
            }
        })
        .on_close(|win: &mut Win| {
            close(win);
        })
        .build(&k, layer);
}

/// The box open over an empty window, as it opens from the top bar.
fn scene(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let base = el().key("root.base").w_full().h_full();
    rune_kit::root(base, layer(win, hub, cx))
}
