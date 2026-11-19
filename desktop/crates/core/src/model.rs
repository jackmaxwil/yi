//! The one model, on the UI thread, shared by every window. Each part belongs to the task
//! that owns its module; a part is added there, never here.

use crate::address::attach::Attached;
use crate::feed::Tables;
use crate::findings::state::Findings;
use crate::grid::windows::Windows;
use crate::ids::Millis;
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
}
