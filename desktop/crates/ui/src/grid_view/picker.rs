//! The layout and pane commands, and the Panes panel the top bar's button opens.

use std::borrow::Cow;

use yd_core::command::table::{Command, Route};
use yd_core::grid::saved::{FIRST_OWN, OwnId, Slug};
use yd_core::grid::windows::Windows;
use yd_core::ids::{CommandId, SettingKey};
use yd_core::pane::PaneKind;
use yd_core::settings::schema::{Choice, Control, Setting, SettingValue, Store};

mod panel;
mod places;

pub use panel::{Naming, close, layer};

/// Opens and closes the Panes panel: what the top bar's Panes button runs.
pub const PICK: CommandId = CommandId("layout.pick");
/// The top bar's Panes button, which the panel hangs from.
pub const BUTTON: &str = "shell.panes";

const fn layout(id: &'static str, title: &'static str, key: &'static str) -> Command {
    Command {
        id: CommandId(id),
        title,
        key: Some(key),
        menu: Some("View"),
        kinds: &[],
        route: Route::Model,
    }
}

/// The five saved layouts, in the record's order (2.7.4), with plan default 1's keys.
pub const LAYOUTS: &[(Command, Slug)] = &[
    (
        layout("layout.everything", "Everything", "alt+cmd+1"),
        "everything",
    ),
    (layout("layout.review", "Review", "alt+cmd+2"), "review"),
    (layout("layout.screens", "Screens", "alt+cmd+3"), "screens"),
    (layout("layout.code", "Code", "alt+cmd+4"), "code"),
    (
        layout("layout.just-chat", "Just Chat", "alt+cmd+5"),
        "just-chat",
    ),
];

/// The command that opens the panel. `grid_view::view::run` carries these, `LAYOUTS` and
/// `OWN` out. ⌥⌘0 sits beside the layouts' own ⌥⌘ keys.
pub const COMMANDS: &[Command] = &[Command {
    id: PICK,
    title: "Panes and Layouts",
    key: Some("alt+cmd+0"),
    menu: Some("View"),
    kinds: &[],
    route: Route::Model,
}];

const fn own(id: &'static str, title: &'static str, key: Option<&'static str>) -> Command {
    Command {
        id: CommandId(id),
        title,
        key,
        menu: Some("View"),
        kinds: &[],
        route: Route::Model,
    }
}

/// A command for each number a kept layout can have, 6 to 9 with their ⌥⌘ keys; menus and
/// the command box name each after its layout and leave out a number nobody keeps.
pub const OWN: &[Command] = &[
    own("layout.own-6", "Saved Layout 6", Some("alt+cmd+6")),
    own("layout.own-7", "Saved Layout 7", Some("alt+cmd+7")),
    own("layout.own-8", "Saved Layout 8", Some("alt+cmd+8")),
    own("layout.own-9", "Saved Layout 9", Some("alt+cmd+9")),
    own("layout.own-10", "Saved Layout 10", None),
    own("layout.own-11", "Saved Layout 11", None),
    own("layout.own-12", "Saved Layout 12", None),
    own("layout.own-13", "Saved Layout 13", None),
];

pub const CHAT_SIDE_KEY: &str = "layout.chat_side";
/// The saved layout the main window shows.
pub const LAYOUT_KEY: &str = "layout.saved";
const SIDES: &[Choice] = &[choice("Left", "left"), choice("Right", "right")];
const SAVED: &[Choice] = &[
    choice("Everything", "everything"),
    choice("Review", "review"),
    choice("Screens", "screens"),
    choice("Code", "code"),
    choice("Just chat", "just-chat"),
];

const fn choice(label: &'static str, value: &'static str) -> Choice {
    Choice {
        label: Cow::Borrowed(label),
        value: Cow::Borrowed(value),
    }
}

const fn setting(
    key: &'static str,
    name: &'static str,
    description: &'static str,
    control: Control,
    default: &'static str,
) -> Setting {
    Setting {
        key: SettingKey::fixed(key),
        group: Cow::Borrowed("Layout"),
        name: Cow::Borrowed(name),
        description: Cow::Borrowed(description),
        control,
        store: Store::App,
        default: Some(SettingValue::Text(Cow::Borrowed(default))),
        value: None,
    }
}

/// The layout's own settings: each shows what the grid holds, and a change to one changes
/// the grid (record 7.3). The grid is kept with the windows' places, not in the settings file.
pub const SETTINGS: &[Setting] = &[
    setting(
        CHAT_SIDE_KEY,
        "Chat side",
        "Which side the chat pane opens on.",
        Control::Segmented(Cow::Borrowed(SIDES)),
        "left",
    ),
    setting(
        LAYOUT_KEY,
        "Layout",
        "The saved layout the main window shows.",
        Control::Select(Cow::Borrowed(SAVED)),
        "everything",
    ),
];

const fn shows(id: &'static str, title: &'static str) -> Command {
    Command {
        id: CommandId(id),
        title,
        key: None,
        menu: None,
        kinds: &[],
        route: Route::Model,
    }
}

/// Settings and Themes are shown in a window of their own, which the app's window layer opens.
pub const SHOW_SETTINGS: CommandId = CommandId("pane.show-settings");
pub const SHOW_THEMES: CommandId = CommandId("pane.show-themes");

/// Every pane, each shown by a command: `show_pane` carries them out.
pub const PANES: &[(Command, PaneKind)] = &[
    (shows("pane.show-chat", "Show Chat"), PaneKind::Chat),
    (shows("pane.show-files", "Show Files"), PaneKind::Files),
    (
        shows("pane.show-changes", "Show Changes"),
        PaneKind::Changes,
    ),
    (shows("pane.show-code", "Show Code"), PaneKind::Code),
    (
        shows("pane.show-terminal", "Show Terminal"),
        PaneKind::Terminal,
    ),
    (
        shows("pane.show-remote", "Show Remote desktop"),
        PaneKind::Remote,
    ),
    (shows("pane.show-agents", "Show Agents"), PaneKind::Agents),
    (shows("pane.show-trace", "Show Trace"), PaneKind::Trace),
    (shows("pane.show-logs", "Show Logs"), PaneKind::Logs),
    (
        shows("pane.show-findings", "Show Findings"),
        PaneKind::Findings,
    ),
    (
        Command {
            key: Some("cmd+,"),
            menu: Some("Yi"),
            ..shows(SHOW_SETTINGS.0, "Show Settings")
        },
        PaneKind::Settings,
    ),
    (
        Command {
            menu: Some("Yi"),
            ..shows(SHOW_THEMES.0, "Show Themes")
        },
        PaneKind::Themes,
    ),
    (
        shows("pane.show-debugger", "Show Debugger"),
        PaneKind::Debugger,
    ),
    (
        shows("pane.show-lsp", "Show Language servers"),
        PaneKind::LanguageServers,
    ),
    (shows("pane.show-assets", "Show Assets"), PaneKind::Assets),
    (
        shows("pane.show-containers", "Show Containers"),
        PaneKind::Containers,
    ),
    (shows("pane.show-vms", "Show VMs"), PaneKind::Vms),
    (
        shows("pane.show-daemons", "Show Daemons"),
        PaneKind::Daemons,
    ),
    (
        shows("pane.show-plugins", "Show Plugins"),
        PaneKind::Plugins,
    ),
    (
        shows("pane.show-plugin-kit", "Show Plugin kit"),
        PaneKind::PluginKit,
    ),
    (shows("pane.show-design", "Show Design"), PaneKind::Design),
    (shows("pane.show-web", "Show Web app"), PaneKind::Web),
    (shows("pane.show-report", "Show Report"), PaneKind::Report),
    (shows("pane.show-script", "Show Script"), PaneKind::Script),
    (
        shows("pane.show-ipython", "Show IPython"),
        PaneKind::IPython,
    ),
];

/// The pane a show command opens.
pub fn pane_of(command: CommandId) -> Option<PaneKind> {
    let found = PANES.iter().find(|(each, _)| each.id == command);
    found.map(|(_, kind)| *kind)
}

/// The command that shows a pane of `kind`, for a chip or a button that opens one.
pub fn show(kind: PaneKind) -> Option<CommandId> {
    let found = PANES.iter().find(|(_, each)| *each == kind);
    found.map(|(command, _)| command.id)
}

/// The saved layout a layout command shows.
pub fn slug_of(command: CommandId) -> Option<Slug> {
    let found = LAYOUTS.iter().find(|(each, _)| each.id == command);
    found.map(|(_, slug)| *slug)
}

/// What a menu, the command box and the key sheet call `command`: a kept layout's name, and
/// none for a number nobody keeps; any other command's own title.
pub fn title(grid: &Windows, command: &Command) -> Option<Cow<'static, str>> {
    let Some(id) = own_of(command.id) else {
        return Some(Cow::Borrowed(command.title));
    };
    let kept = grid.own.iter().find(|own| own.id == id);
    kept.map(|own| Cow::Owned(format!("Layout: {}", own.name)))
}

/// The number of the kept layout `command` shows.
pub fn own_of(command: CommandId) -> Option<OwnId> {
    let at = OWN.iter().position(|each| each.id == command)?;
    let at = u8::try_from(at).ok()?;
    FIRST_OWN.checked_add(at).map(OwnId)
}

/// The command that shows the kept layout numbered `id`.
pub fn own_command(id: OwnId) -> Option<&'static Command> {
    OWN.get(usize::from(id.0.checked_sub(FIRST_OWN)?))
}
