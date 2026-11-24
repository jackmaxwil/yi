//! Debugger pane: source with marks, controls, side lists and a console.

use rune::{Cx, El, Tx, col, row};
use yd_core::address::thing::Thing;
use yd_core::command::by_kind::{BREAK_HERE, VARIABLE_WATCH};
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::command::tools::debug;
use yd_core::feed::DEBUGGER;
use yd_core::ids::{CommandId, PaneId, TabId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::list::row as listed;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef};
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win, dispatch};

use super::desc::{self, VARIABLE, WATCH_VARIABLE, of_row, row_command};
use super::{console, source};
use session::Session;
use yd_ui::states::status::Status;

use super::states::WORDS;

mod header;
mod scenes;
pub mod session;
mod side;
#[cfg(test)]
mod tests;

const fn command(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[],
        route: Route::Source(DEBUGGER),
    }
}

const VARIABLES: &[Kind] = &[Kind::Variable];
const BREAKPOINTS: &[Kind] = &[Kind::Breakpoint];

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Debugger,
    view,
    scenes: scenes::SCENES,
    commands: &[
        command(debug::LAUNCH, "Launch in Debugger"),
        command(debug::ATTACH, "Debug a Running Process"),
        command(debug::CONTINUE, "Continue"),
        command(debug::STEP_OVER, "Step Over"),
        command(debug::STEP_IN, "Step In"),
        command(debug::STEP_OUT, "Step Out"),
        command(debug::RESTART, "Restart Debugging"),
        command(debug::STOP, "Stop Debugging"),
        command(debug::BREAK, "Toggle Breakpoint"),
        command(debug::WATCH, "Watch Expression"),
        command(debug::EVAL, "Evaluate in Debugger Console"),
        row_command(desc::WATCH_VARIABLE, "Watch", VARIABLES),
        row_command(desc::STOP_WHEN_CHANGES, "Stop When It Changes", VARIABLES),
        row_command(desc::STOP_WHEN_SET, "Stop When It Is Set", VARIABLES),
        row_command(desc::EDIT_WATCH, "Edit Watch", VARIABLES),
        row_command(desc::REMOVE_WATCH, "Remove Watch", VARIABLES),
        row_command(desc::ADD_CONDITION, "Add Condition", BREAKPOINTS),
        row_command(desc::EDIT_CONDITION, "Edit Condition", BREAKPOINTS),
        row_command(desc::TURN_ON, "Turn On", BREAKPOINTS),
        row_command(desc::TURN_OFF, "Turn Off", BREAKPOINTS),
    ],
    settings: &[],
    tabs: Some(tabs),
    on_tab: None,
    finder: Some(Finder::Keyed(thing_at)),
};

/// Carries the right-click rows and detail buttons that reach the debugger: a breakpoint
/// toggled on a line, and a variable watched by its path. False for anything else.
pub fn carry(win: &mut Win, _: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(at) = target else {
        return false;
    };
    let run = match (command, at.kind()) {
        (BREAK_HERE, Kind::Lines) => Run::new(debug::BREAK).on(target.clone()),
        (WATCH_VARIABLE | VARIABLE_WATCH, Kind::Variable) => {
            let path = at.id().strip_prefix(VARIABLE).unwrap_or(at.id());
            Run {
                text: Some(path.to_owned()),
                ..Run::new(debug::WATCH)
            }
        }
        _ => return false,
    };
    dispatch(win, Msg::Command(Route::Source(DEBUGGER), run));
    true
}

fn session(hub: &Hub) -> Option<Session> {
    Session::of(hub.model.tables.get(DEBUGGER))
}

/// The debugger and, beside it, the file the source area shows.
fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let session = session(pane.hub);
    let debugger = Tab {
        meta: session.as_ref().map(Session::meta).unwrap_or_default(),
        fixed: true,
        ..Tab::of(PaneKind::Debugger)
    };
    let file = session
        .as_ref()
        .and_then(|session| source::stand(pane.hub, pane.pane, session))
        .map(|at| Tab {
            id: TabId(1),
            title: at.path.rsplit('/').next().unwrap_or(&at.path).to_owned(),
            fixed: true,
            ..Tab::of(PaneKind::Code)
        });
    Tabs {
        tabs: std::iter::once(debugger).chain(file).collect(),
        active: Some(TabId(0)),
        adds: false,
        tool: None,
    }
}

/// What a keyed row or source line of this pane stands for.
fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    let Some(id) = listed::id_of(key) else {
        return source::thing_at(hub, pane, key);
    };
    let found = hub.model.tables.get(DEBUGGER)?.find(&id)?;
    listed::thing(of_row(id.as_str())?, found).ok()
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Debugger);
    let tokens = pane.tokens();
    let session = session(pane.hub);
    let adapter = session.as_ref().map(|session| session.adapter.as_str());
    let left = col()
        .key(format!("{key}.left"))
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(source::view(pane, &key, session.as_ref()))
        .child(console::view(pane, &key, adapter.unwrap_or_default()));
    // With no run there is nothing to step through: the adapter's state stands in for the body.
    let idle = session.is_none().then(|| {
        let table = pane.model().tables.get(yd_core::feed::DEBUGGER);
        Status::of_table(table).unwrap_or(Status::Empty)
    });
    let body = match idle {
        Some(status) => yd_ui::states::status::view(pane, &key, &status, &WORDS),
        None => row()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(left)
            .child(side::view(pane, &key)),
    };
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(header::view(pane, &key, session.as_ref()))
        .child(body)
}
