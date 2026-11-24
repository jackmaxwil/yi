//! The shell's view: top bar, sidebar, the grid's place and the status bar, live from the model.

mod scenes;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use rune::{Cx, El, Tx, col, el, row};
use yd_core::address::attach::PICK;
use yd_core::address::uri::Address;
use yd_core::command::by_kind::VM_SCREEN;
use yd_core::command::table::{
    ACTUAL_SIZE, CYCLE_WINDOWS, OPEN_FOLDER, Run, TOGGLE_NOTIFICATIONS, TOGGLE_SIDEBAR, Target,
    ZOOM_IN, ZOOM_OUT,
};
use yd_core::command::tools::{container, plugin, service};
use yd_core::ids::{CommandId, PaneId, WindowId};
use yd_core::kind::Kind;
use yd_core::model::Model;
use yd_core::pane::PaneKind;
use yd_core::reduce::state::Wire;
use yd_core::reduce::step::commands::NEW;
use yd_core::reduce::threads::{Status, Thread};
use yd_core::row::{Dot, Row, Rows};
use yd_core::tokens::Tokens;
use yd_panes::chat_doc::view as chat_doc;
use yd_panes::new_chat::view as new_chat;
use yd_panes::notifications::view as notifications;
use yd_ui::grid_view;
use yd_ui::menu::command_box;
use yd_ui::overlay::pick;
use yd_ui::theme::look::colour;
use yd_ui::theme::scale::{
    Ink, WASH, WASH_BASE, WASH_BLUE, WASH_EDGE, WASH_FROM, WASH_GLOW, WASH_MIDDLE, WASH_TEAL,
    WASH_TO, WASH_VIOLET, WASH_WINE,
};
use yd_ui::win::{Hub, Win};

use crate::root;
use crate::shell::{sidebar, status_bar, top_bar};
use crate::windows::{self, hub::APART};

pub use scenes::SCENES;
/// The thread a window shows is kept where every pane reads it; the shell picks it.
pub use yd_ui::shown::{roots, show as select, thread as current};

pub const PICK_LAYOUT: CommandId = grid_view::picker::PICK;
/// What the window itself carries out: the table declares them, `carry` does them. The
/// container and service commands are here because a pane's `carry` asks before it sends.
pub const WINDOWED: &[CommandId] = &[
    TOGGLE_SIDEBAR,
    CYCLE_WINDOWS,
    ZOOM_IN,
    ZOOM_OUT,
    ACTUAL_SIZE,
    TOGGLE_NOTIFICATIONS,
    OPEN_FOLDER,
    PICK,
    container::LOGS,
    container::SHELL,
    container::RESTART,
    container::STOP,
    container::START,
    container::REMOVE,
    service::START,
    service::STOP,
    service::RESTART,
    service::LOGS,
    service::LOGIN,
    plugin::BUILD,
    plugin::RELOAD,
    plugin::OPEN_SOURCE,
    VM_SCREEN,
    grid_view::picker::SHOW_SETTINGS,
    grid_view::picker::SHOW_THEMES,
    yd_panes::chat::pane::environment::TOGGLE,
];

/// The saved layout that is the conversation alone: no sidebar, no status bar.
pub const JUST_CHAT: &str = "just-chat";
/// New chat's title row, which the sidebar beside it leaves to the traffic lights.
const NEW_TOP: f32 = 30.0;

/// One window's shell: whether its sidebar is hidden (open or gone, never folded), the width
/// a drag of its edge left it (none: the default) and the width while a drag is under way.
#[derive(Debug, Clone, Default)]
pub struct Shell {
    pub hidden: bool,
    pub width: Option<f32>,
    pub held: Option<f32>,
}

/// Every window's shell, kept in the hub so a scene can set it.
#[derive(Debug, Default)]
pub struct Shells(pub BTreeMap<WindowId, Shell>);

pub fn shell(hub: &Hub, window: WindowId) -> Shell {
    let shells = hub.panes.get::<Shells>(None);
    shells
        .and_then(|shells| shells.0.get(&window))
        .cloned()
        .unwrap_or_default()
}

/// Changes window `window`'s shell in place.
pub fn change(hub: &mut Hub, window: WindowId, edit: impl FnOnce(&mut Shell)) {
    if let Some(shells) = hub.panes.get_mut::<Shells>(None) {
        edit(shells.0.entry(window).or_default());
    }
}

/// Runs a command from a shell control, as a key or a menu would: the command box, the
/// grid, what the window carries, then the route the table gives it.
pub fn run(win: &mut Win, tx: &mut Tx, command: CommandId, target: Target) {
    command_box::run(win, tx, Run::new(command).on(target));
}

/// What only a window can do: the sidebar, the zoom, the bell, pick mode, New chat for a
/// `thread.new` with no words, and what each pane's own `carry` does for its buttons.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    if command == TOGGLE_SIDEBAR {
        let window = win.window;
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            change(&mut hub, window, |shell| shell.hidden = !shell.hidden);
        }
        return true;
    }
    if command == CYCLE_WINDOWS {
        return windows::hub::cycle(win);
    }
    if [ZOOM_IN, ZOOM_OUT, ACTUAL_SIZE].contains(&command) {
        return root::zoom_to(win, command);
    }
    if command == OPEN_FOLDER {
        new_chat::choose_folder(win, tx);
        return true;
    }
    if command == NEW {
        yd_ui::shown::start(win);
        tx.focus_when_shown("new-chat.composer.input", false);
        return true;
    }
    let apart = grid_view::picker::pane_of(command).filter(|kind| APART.contains(kind));
    if let Some(kind) = apart {
        return windows::hub::apart(win, kind);
    }
    notifications::run(win, tx, command)
        || (command == PICK && pick::run(win, tx, command, target))
        || yd_panes::settings::pane::pane_word(win, tx, command, target)
        || yd_panes::agents::pane::carry(win, tx, command, target)
        || yd_panes::chat::pane::child::carry(win, tx, command, target)
        || yd_panes::trace::pane::carry(win, tx, command, target)
        || yd_panes::logs::pane::carry(win, tx, command, target)
        || yd_panes::settings::pane::carry(win, tx, command, target)
        || yd_panes::findings::pane::carry(win, tx, command, target)
        || yd_panes::code_view::refs::carry(win, tx, command, target)
        || yd_panes::debugger::pane::carry(win, tx, command, target)
        || yd_panes::lsp::pane::carry(win, tx, command, target)
        || yd_panes::plugins::pane::carry(win, tx, command, target)
        || yd_panes::plugin_kit::pane::carry(win, tx, command, target)
        || yd_panes::containers::feed::carry(win, tx, command, target)
        || yd_panes::daemons::feed::carry(win, tx, command, target)
        || yd_panes::assets::pane::carry(win, tx, command, target)
        || yd_panes::vms::pane::carry(win, tx, command, target)
        || yd_panes::design::pane::carry(win, tx, command, target)
        || yd_panes::remote::pane::carry(win, tx, command, target)
        || yd_panes::chat::pane::carry(win, tx, command, target)
}

pub fn thing(kind: Kind, id: &str) -> Option<Target> {
    Address::new(kind, id).ok().map(Target::Thing)
}

/// The project a thread belongs to: the last folder of where it runs.
pub fn project(cwd: &str) -> &str {
    cwd.rsplit('/').find(|part| !part.is_empty()).unwrap_or(cwd)
}

/// A thread's dot: a held approval needs you, a question asked without one is a ring.
pub fn dot(wire: &Wire, thread: &Thread) -> Dot {
    match wire.status(&thread.id) {
        Status::NeedsYou if wire.asks.iter().any(|ask| ask.thread == thread.id) => Dot::Need,
        Status::NeedsYou => Dot::Ask,
        Status::Working => Dot::Work,
        Status::Failed => Dot::Fail,
        Status::Done => Dot::Done,
        Status::Idle => Dot::Idle,
    }
}

fn git_rows(model: &Model) -> impl Iterator<Item = &Row> {
    let table = model.tables.get(yd_sources::git::FEED);
    table
        .into_iter()
        .flat_map(|table| (0..table.len()).filter_map(|index| table.get(index)))
}

/// The git feed's row for `project`, else its `repo` row: the shown thread's repository.
pub fn repo<'a>(model: &'a Model, project: &str) -> Option<&'a Row> {
    let named = git_rows(model).find(|row| row.words("project") == Some(project));
    named.or_else(|| git_rows(model).find(|row| row.id.as_str() == "repo"))
}

/// The branch `project`'s repository is based on, when the git feed names the project.
pub fn base<'a>(model: &'a Model, project: &str) -> Option<&'a str> {
    let named = git_rows(model).find(|row| row.words("project") == Some(project));
    named.and_then(|row| row.words("base"))
}

pub fn just_chat(model: &Model, window: WindowId) -> bool {
    let shown = model.grid.window(window).and_then(|window| window.saved);
    shown == Some(JUST_CHAT)
}

/// The chat pane of `window`: the one whose conversation Just chat shows as a document.
fn chat_pane(model: &Model, window: WindowId) -> Option<PaneId> {
    let docked = model.grid.window(window)?.layout.docked();
    let mut panes = docked.map(|(pane, _)| pane);
    panes.find(|pane| model.grid.kind(*pane) == Some(PaneKind::Chat))
}

/// What a window draws in place of its bars and grid: New chat while it is asked for or no
/// thread exists, and in Just chat the document alone (plan default 12).
fn whole(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> Option<El> {
    if yd_ui::shown::fresh(hub, win.window) {
        // The owner's first look: New chat keeps the sidebar, unless it was hidden.
        let side = (!shell(hub, win.window).hidden).then(|| sidebar::view(win, hub, cx, NEW_TOP));
        let page = col().grow(1.0).min_w(0.0).h_full();
        let page = page.child(new_chat::page(win, hub, cx));
        return Some(row().w_full().h_full().children(side).child(page));
    }
    let chat = chat_pane(&hub.model, win.window).filter(|_| just_chat(&hub.model, win.window));
    chat.map(|pane| chat_doc::page_of(win, hub, cx, pane))
}

fn bars_and_grid(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    // The grid fills this slot: every pane of the window's layout, where `place` puts it.
    let grid = col()
        .key("shell.grid")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .h_full()
        .child(grid_view::view::grid(win, hub, cx));
    let side = (!shell(hub, win.window).hidden).then(|| sidebar::view(win, hub, cx, 0.0));
    let body = row()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .children(side)
        .child(grid);
    col()
        .w_full()
        .h_full()
        .child(top_bar::view(win, hub, cx))
        .child(body)
        .child(status_bar::view(win, hub))
}

/// One hue of the wash: an ellipse `radii` of the window across, centred at `at`, that fades
/// out by `end` of the way to its edge.
type Hue = (Ink, [f32; 2], [f32; 2], f32);

/// The mockup's own stops, bottom first, written for its 1200 by 750 window.
const HUES: [Hue; 4] = [
    (WASH_WINE, [0.18, 0.96], [0.44, 0.58], 0.6),
    (WASH_VIOLET, [0.72, 0.92], [0.54, 0.72], 0.62),
    (WASH_TEAL, [0.88, 0.12], [0.46, 0.6], 0.6),
    (WASH_BLUE, [0.12, 0.18], [0.6, 0.8], 0.62),
];

/// The muted desktop the mockup shows through its thin glass, painted under the window's
/// tint as layers of `size`; a theme's `wash` of 0 leaves the real desktop to show instead.
fn wash(tokens: &Tokens, size: [f32; 2]) -> Vec<El> {
    let strength = WASH.of(tokens).clamp(0.0, 1.0);
    if strength <= 0.0 {
        return Vec::new();
    }
    let [w, h] = size;
    let ink = |ink: Ink, share: f32| {
        let value = ink.of(tokens);
        colour(value.alpha(value.a * strength * share))
    };
    let layer = || el().absolute(0.0, 0.0).size(w, h);
    let light = [
        (0.0, ink(WASH_FROM, 1.0)),
        (0.5, ink(WASH_MIDDLE, 1.0)),
        (1.0, ink(WASH_TO, 1.0)),
    ];
    let mut layers = vec![
        layer()
            .fill(ink(WASH_BASE, 1.0))
            .linear_gradient(128f32.to_radians(), light),
    ];
    for (hue, at, radii, end) in HUES {
        let stops = [(0.0, ink(hue, 1.0)), (end, ink(hue, 0.0))];
        layers.push(layer().radial_gradient(at, [radii[0] * w, radii[1] * h], stops));
    }
    let edge = [(0.55, ink(WASH_EDGE, 0.0)), (1.0, ink(WASH_EDGE, 1.0))];
    layers.push(layer().radial_gradient([0.5, 0.42], [1.3 * w, 1.2 * h], edge));
    let glow = [(0.0, ink(WASH_GLOW, 1.0)), (1.0, ink(WASH_GLOW, 0.0))];
    layers.push(layer().radial_gradient([0.75, 0.8], 300.0, glow));
    layers
}

pub fn view(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let frame = match whole(win, hub, cx) {
        Some(page) => page,
        None => bars_and_grid(win, hub, cx),
    };
    let layers = std::iter::once(frame).chain(notifications::layer(hub));
    backdrop(hub, cx, "shell", layers)
}

/// What every window draws under `layers`, keyed `key`: the wash, then the glass layer's tint.
pub fn backdrop(
    hub: &Hub,
    cx: &mut Cx<Win>,
    key: &'static str,
    layers: impl IntoIterator<Item = El>,
) -> El {
    let tokens = &hub.model.tokens;
    // Invariant: the root carries the window's one material; this tint is the glass layer's.
    let tint = colour(tokens.window_tint.alpha(tokens.glass_window));
    let washed = cx.layout_size(key).map(|size| wash(tokens, size));
    let body = col().w_full().h_full().fill(tint).children(layers);
    col()
        .key(key)
        .w_full()
        .h_full()
        .children(washed.into_iter().flatten())
        .child(body)
}
