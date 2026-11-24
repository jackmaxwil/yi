//! The one model, on the UI thread, shared by every window. Each part belongs to the task
//! that owns its module; a part is added there, never here.

use crate::address::attach::Attached;
use crate::feed::Tables;
use crate::findings::state::Findings;
use crate::grid::windows::Windows;
use crate::ids::{Millis, PaneId};
use crate::reduce::state::Wire;
use crate::settings::store::Settings;
use crate::tokens::Tokens;
use crate::trace::Trace;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Model {
    /// Threads, turns, approvals, child agents, todos and usage, folded from the server.
    pub wire: Wire,
    /// Every feed's rows.
    pub tables: Tables,
    /// The windows, the panes in them and how each window's grid is laid out.
    pub grid: Windows,
    /// The things attached to the next prompt.
    pub attached: Attached,
    pub findings: Findings,
    pub settings: Settings,
    pub trace: Trace,
    pub tokens: Tokens,
    /// The last time the app's clock ticked.
    pub now: Millis,
    /// The person did something since the wire last told the time.
    pub acted: bool,
    /// When the person last did something, as late as the wire's next word after it.
    pub touched: Millis,
    /// Lines the model pointed at, each for its pane to scroll to once its file is read.
    pub reveals: Vec<(PaneId, u32)>,
}

/// The most lines kept for panes still reading their file.
const MAX_REVEALS: usize = 8;

impl Model {
    /// Notes the line `pane` scrolls to once its file is read; past the cap the oldest goes.
    pub fn reveal(&mut self, pane: PaneId, line: u32) {
        self.reveals.retain(|(other, _)| *other != pane);
        self.reveals.push((pane, line));
        let over = self.reveals.len().saturating_sub(MAX_REVEALS);
        self.reveals.drain(..over);
    }
}
