//! Everything that can happen to the model, the one pure function that applies it, and what
//! it asks of the world in return.

use crate::address::attach::{self, AttachMsg};
use crate::command::run;
use crate::command::table::{Route, Run};
use crate::feed::{FeedEvent, SourceCmd};
use crate::findings::triage::{self, TriageMsg};
use crate::grid::step::{self as grid_step, GridMsg};
use crate::grid::windows::WindowOp;
use crate::ids::{FeedId, Millis, ThreadId};
use crate::model::Model;
use crate::reduce;
use crate::settings::store::{self, SettingsMsg};
use crate::tokens::Tokens;
use crate::wire::event::WireEvent;
use crate::wire::request::Request;

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// A frame from Yi's server.
    Wire(WireEvent),
    /// Rows or a status from a source.
    Feed(FeedId, FeedEvent),
    /// A user action: a command, where it goes and what it is aimed at.
    Command(Route, Run),
    /// A typed ask of one source: keys for a shell, a size, a folder, a file to read.
    Source(FeedId, SourceCmd),
    /// A change to a window's grid that is not a command: a line dragged, a pane dropped.
    Grid(GridMsg),
    Attach(AttachMsg),
    Triage(TriageMsg),
    Settings(SettingsMsg),
    /// The theme in use changed: these are the tokens every window now draws with.
    Theme(Box<Tokens>),
    /// The app's clock. Timers are events.
    Tick(Millis),
}

/// Something that needs the user, for the bell, the dock badge and a system notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub thread: Option<ThreadId>,
    pub title: String,
    pub body: String,
}

/// What a step asks for. The app carries each out; `step` itself touches nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A request to Yi's server, for the wire's writer thread.
    Wire(Request),
    /// A command for a running source; the app starts the source if it is not running.
    Source(FeedId, SourceCmd),
    /// Open, close or move a real window.
    Window(WindowOp),
    Notify(Notice),
}

/// Applies `msg` to `model`. Pure: the same model and message give the same result.
pub fn step(model: &mut Model, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Wire(event) => reduce::step::step(model, event),
        Msg::Feed(feed, event) => {
            model.tables.apply(feed, event);
            Vec::new()
        }
        Msg::Command(Route::Model, command) => run::run(model, &command),
        Msg::Command(Route::Wire, command) => reduce::step::request(model, &command),
        Msg::Command(Route::Source(feed), command) => {
            vec![Effect::Source(feed, SourceCmd::Run(command))]
        }
        Msg::Source(feed, ask) => vec![Effect::Source(feed, ask)],
        Msg::Grid(change) => {
            // The grid reads the person's word on what opens by itself as the settings say it.
            model.grid.rules = model.settings.opens();
            grid_step::step(&mut model.grid, change)
        }
        Msg::Attach(change) => attach::step(&mut model.attached, change),
        Msg::Triage(change) => triage::step(model, change),
        Msg::Settings(change) => store::step(model, change),
        Msg::Theme(tokens) => {
            model.tokens = *tokens;
            Vec::new()
        }
        Msg::Tick(now) => {
            model.now = now;
            Vec::new()
        }
    }
}
