//! The command record, where a command goes, what it is aimed at, the app-level table, the
//! one lookup every command is found in, and the shortcut map and Mac menu bar it gives.

use crate::address::attach;
use crate::address::uri::Address;
use crate::command::{by_kind, run};
use crate::ids::{CommandId, FeedId, PaneId};
use crate::kind::Kind;
use crate::reduce::step::commands as wire;

/// Who carries a command out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// It changes the model: `command::run` handles it.
    Model,
    /// It becomes a request to Yi's server.
    Wire,
    /// It goes to the source that fills this feed.
    Source(FeedId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Command {
    pub id: CommandId,
    pub title: &'static str,
    /// The shortcut as Rune parses it, such as `cmd+k`.
    pub key: Option<&'static str>,
    /// The Mac menu it is listed in.
    pub menu: Option<&'static str>,
    /// The kinds of thing it can be aimed at; none means the app itself.
    pub kinds: &'static [Kind],
    pub route: Route,
}

/// What a command is aimed at.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Target {
    #[default]
    App,
    Pane(PaneId),
    Thing(Address),
}

/// One use of a command.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub command: CommandId,
    pub target: Target,
    /// What the user typed for it: a prompt, a search, a value.
    pub text: Option<String>,
}

impl Run {
    pub fn new(command: CommandId) -> Self {
        Self {
            command,
            target: Target::App,
            text: None,
        }
    }

    pub fn on(self, target: Target) -> Self {
        Self { target, ..self }
    }
}

pub const FLIP_CHAT: CommandId = CommandId("layout.flip-chat");
/// Opens a file in the code viewer: aimed at a `file` address, or `lines` of one.
pub const OPEN_FILE: CommandId = CommandId("code.open");
/// Opens the command box in the window it is run from.
pub const OPEN_COMMANDS: CommandId = CommandId("command.open");
/// Hides the window's sidebar, or shows it again.
pub const TOGGLE_SIDEBAR: CommandId = CommandId("sidebar.toggle");
pub const TOGGLE_NOTIFICATIONS: CommandId = CommandId("notifications.toggle");
/// Opens the Mac's folder chooser and shows New chat on the folder chosen.
pub const OPEN_FOLDER: CommandId = CommandId("project.open");
/// Opens the command box as the list of every shortcut.
pub const SHOW_KEYS: CommandId = CommandId("help.keyboard");
/// The window's zoom, one step larger or smaller, or back to the Mac's own points.
pub const ZOOM_IN: CommandId = CommandId("view.zoom-in");
pub const ZOOM_OUT: CommandId = CommandId("view.zoom-out");
pub const ACTUAL_SIZE: CommandId = CommandId("view.actual-size");
/// Brings the app's next window forward, as ⌘` does in every Mac app.
pub const CYCLE_WINDOWS: CommandId = CommandId("window.cycle");
/// Shows the thing under the pointer in a card over the window, or closes that card.
pub const PEEK: CommandId = CommandId("thing.peek");

const fn app(
    id: CommandId,
    title: &'static str,
    key: Option<&'static str>,
    menu: &'static str,
) -> Command {
    Command {
        id,
        title,
        key,
        menu: Some(menu),
        kinds: &[],
        route: Route::Model,
    }
}

/// Commands that belong to no pane. A pane lists its own in its definition.
pub const COMMANDS: &[Command] = &[
    app(
        OPEN_COMMANDS,
        "Search or Run a Command",
        Some("cmd+k"),
        "Yi",
    ),
    app(OPEN_FOLDER, "Open Folder…", Some("cmd+o"), "File"),
    app(TOGGLE_SIDEBAR, "Toggle Sidebar", Some("alt+cmd+s"), "View"),
    app(TOGGLE_NOTIFICATIONS, "Show Notifications", None, "View"),
    app(FLIP_CHAT, "Move Chat to the Other Side", None, "View"),
    app(ZOOM_IN, "Zoom In", Some("cmd+="), "View"),
    app(ZOOM_OUT, "Zoom Out", Some("cmd+-"), "View"),
    app(ACTUAL_SIZE, "Actual Size", Some("cmd+0"), "View"),
    app(
        CYCLE_WINDOWS,
        "Cycle Through Windows",
        Some("cmd+`"),
        "Window",
    ),
    app(SHOW_KEYS, "Keyboard Shortcuts", Some("cmd+/"), "Help"),
    // Invariant: no Mac menu lists it, so bare Space is never a menu's key and still types.
    Command {
        id: PEEK,
        title: "Peek",
        key: Some("space"),
        menu: None,
        kinds: &[],
        route: Route::Model,
    },
];

/// Every command `yd-core` declares: the app's own, the right-click items' and the wire's.
pub fn declared() -> impl Iterator<Item = &'static Command> {
    COMMANDS
        .iter()
        .chain(attach::COMMANDS)
        .chain(by_kind::COMMANDS)
        .chain(wire::COMMANDS)
}

/// Every command of the app, found by id and by the kinds it is aimed at. A window builds it
/// from [`declared`], each pane's definition and its engines' tables.
#[derive(Debug, Clone, Default)]
pub struct Commands {
    rows: Vec<&'static Command>,
}

impl Commands {
    pub fn new(rows: impl IntoIterator<Item = &'static Command>) -> Self {
        Self {
            rows: rows.into_iter().collect(),
        }
    }

    /// The command declared under `id`; the first row wins when two share it.
    pub fn find(&self, id: CommandId) -> Option<&'static Command> {
        self.rows.iter().copied().find(|command| command.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static Command> + '_ {
        self.rows.iter().copied()
    }

    /// The ids declared by more than one row: none, in a sound table.
    pub fn twice(&self) -> Vec<CommandId> {
        let mut ids: Vec<CommandId> = self.rows.iter().map(|command| command.id).collect();
        ids.sort();
        let pairs = ids.windows(2).filter_map(|pair| match pair {
            [a, b] if a == b => Some(*a),
            _ => None,
        });
        let mut twice: Vec<CommandId> = pairs.collect();
        twice.dedup();
        twice
    }

    /// Whether running `command` does anything yet. A wire or source command is carried out
    /// by its route; a model one by `command::run` or by the window (`window` says which).
    pub fn built(command: &Command, window: impl Fn(CommandId) -> bool) -> bool {
        match command.route {
            Route::Wire | Route::Source(_) => true,
            Route::Model => run::carries(command.id) || window(command.id),
        }
    }

    /// The Mac menu bar: each menu a command names, in [`MENUS`] order then as first named,
    /// its items in table order, greyed while not built.
    pub fn menus(&self, built: impl Fn(&Command) -> bool) -> Vec<Menu> {
        let mut titles: Vec<&'static str> = MENUS.to_vec();
        for named in self.rows.iter().filter_map(|command| command.menu) {
            if !titles.contains(&named) {
                titles.push(named);
            }
        }
        let menu = |title: &'static str| Menu {
            title,
            items: self
                .rows
                .iter()
                .filter(|command| command.menu == Some(title))
                .map(|command| MenuItem {
                    command: command.id,
                    title: command.title,
                    key: command.key,
                    enabled: built(command),
                })
                .collect(),
        };
        let menus = titles.into_iter().map(menu);
        menus.filter(|menu| !menu.items.is_empty()).collect()
    }
}

/// The menus of the Mac menu bar, in the record's order (5): any other a command names
/// comes after them.
pub const MENUS: &[&str] = &["Yi", "File", "Edit", "View", "Thread", "Window", "Help"];

/// One menu of the Mac menu bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub title: &'static str,
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuItem {
    pub command: CommandId,
    pub title: &'static str,
    pub key: Option<&'static str>,
    pub enabled: bool,
}

/// Modifiers in the order a key is written and matched.
const HELD: &[&str] = &["ctrl", "alt", "shift", "cmd"];

/// One spelling of a key: lower case, modifiers in [`HELD`] order, `enter` as `return`.
pub fn normal(key: &str) -> String {
    let lower = key.to_lowercase();
    let mut parts: Vec<&str> = lower.split('+').collect();
    let last = match parts.pop().unwrap_or_default() {
        "enter" => "return",
        "esc" => "escape",
        "option" => "alt",
        other => other,
    };
    let held = HELD.iter().filter(|name| parts.contains(name));
    held.copied().chain([last]).collect::<Vec<_>>().join("+")
}

/// A key as the menus draw it: `cmd+shift+c` is `⌘⇧C`, as the mockup writes it.
pub fn keycap(key: &str) -> String {
    let lower = normal(key);
    let mut parts: Vec<&str> = lower.split('+').collect();
    let last = parts.pop().unwrap_or_default();
    let order = [("ctrl", "⌃"), ("alt", "⌥"), ("cmd", "⌘"), ("shift", "⇧")];
    let held = order.iter().filter(|(name, _)| parts.contains(name));
    let key = match last {
        "return" => "↵".to_owned(),
        "escape" => "esc".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        key => key.to_uppercase(),
    };
    held.map(|(_, glyph)| *glyph)
        .chain([key.as_str()])
        .collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn no_command_is_declared_twice() {
        assert_eq!(Commands::new(declared()).twice(), Vec::<CommandId>::new());
        let twice = Commands::new(COMMANDS.iter().chain(COMMANDS));
        assert_eq!(
            twice.twice().len(),
            COMMANDS.len(),
            "the check sees a repeat"
        );
    }

    #[test]
    fn a_command_is_found_by_its_id_and_the_wires_are_in_the_table() -> Result<(), Box<dyn Error>> {
        let table = Commands::new(declared());
        let open = table.find(OPEN_COMMANDS).ok_or("no command box")?;
        assert_eq!(open.key, Some("cmd+k"));
        let new = table.find(wire::NEW).ok_or("no new thread")?;
        assert_eq!(new.route, Route::Wire, "thread.new reaches the server");
        assert!(table.find(CommandId("no.such")).is_none());
        Ok(())
    }

    #[test]
    fn the_menu_bar_lists_each_menu_once_in_order_and_greys_what_is_not_built()
    -> Result<(), Box<dyn Error>> {
        let table = Commands::new(declared());
        let menus = table.menus(|command| Commands::built(command, |_| false));
        let titles: Vec<&str> = menus.iter().map(|menu| menu.title).collect();
        assert_eq!(titles, ["Yi", "File", "View", "Thread", "Window", "Help"]);
        let view = menus.get(2).ok_or("no View menu")?;
        let item = |id| view.items.iter().find(|item| item.command == id);
        let flip = item(FLIP_CHAT).ok_or("no chat side")?;
        assert!(flip.enabled, "the model flips the chat");
        let sidebar = item(TOGGLE_SIDEBAR).ok_or("no sidebar")?;
        assert_eq!((sidebar.key, sidebar.enabled), (Some("alt+cmd+s"), false));
        let thread = menus.get(3).ok_or("no Thread menu")?;
        assert!(
            thread.items.iter().all(|item| item.enabled),
            "wire commands are built"
        );
        Ok(())
    }

    #[test]
    fn a_key_reads_as_the_mockup_writes_it() {
        assert_eq!(keycap("cmd+shift+c"), "⌘⇧C");
        assert_eq!(keycap("ctrl+cmd+w"), "⌃⌘W");
        assert_eq!(keycap("cmd+k"), "⌘K");
        assert_eq!(keycap("alt+cmd+1"), "⌥⌘1");
        assert_eq!(keycap("cmd+return"), "⌘↵");
        assert_eq!(keycap("ctrl+shift+cmd+left"), "⌃⌘⇧←");
        assert_eq!(
            keycap("return"),
            "↵",
            "the mockup's Return, on Run and Allow"
        );
    }
}
