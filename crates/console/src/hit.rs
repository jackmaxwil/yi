use ratatui::layout::Rect;

use crate::app::App;
use crate::keys;
use crate::layout::PaneId;

/// The one control table hover and press both walk; press maps it to an action, hover to a rect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Settings(PaneId, crate::settings::Target),
    Button(keys::Action),
    Gear,
    Title(PaneId),
    New(Option<String>),
    Search,
    Fold(String),
    Session(usize),
    /// A chat pane's run chip: the answer cell and the ordinal of the shell block it stands for.
    Run(PaneId, usize, usize),
}

/// Each run chip a chat pane drew this frame is a control the press and the hover walk.
pub(crate) fn offer_runs(app: &mut App, pane: PaneId, runs: Vec<(Rect, usize, usize)>) {
    if let Some(hits) = app.hits.as_mut() {
        hits.controls.extend(
            runs.into_iter()
                .map(|(rect, cell, ordinal)| (rect, Hit::Run(pane, cell, ordinal))),
        );
    }
}
