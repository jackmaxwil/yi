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
    FilesRefresh,
    FilesCollapse,
    FilesClose,
    Fold(String),
    Session(usize),
    /// A chat pane's run chip: the answer cell and the ordinal of the shell block it stands for.
    Run(PaneId, usize, usize),
    /// An editor panel's tab, and its ✕: the panel and the tab's place in the strip.
    EditorTab(PaneId, usize),
    /// A scope chip or file row in a review pane's pinned header.
    Review(PaneId, crate::panes::ReviewPick),
    EditorClose(PaneId, usize),
}
/// Every pane the last frame drew, in draw order: the ids the offer passes and the snap zone walk.
pub(crate) fn drawn(app: &App) -> Vec<PaneId> {
    app.hits
        .as_ref()
        .map(|hits| hits.panes.iter().map(|(id, _)| *id).collect())
        .unwrap_or_default()
}

/// Each control an offer pass found is a control the press and the hover walk.
pub(crate) fn offer(app: &mut App, found: Vec<(Rect, Hit)>) {
    if let Some(hits) = app.hits.as_mut() {
        hits.controls.extend(found);
    }
}

/// Each run chip a chat pane drew this frame is a control the press and the hover walk.
pub(crate) fn offer_runs(app: &mut App, pane: PaneId, runs: Vec<(Rect, usize, usize)>) {
    offer(
        app,
        runs.into_iter()
            .map(|(rect, cell, ordinal)| (rect, Hit::Run(pane, cell, ordinal)))
            .collect(),
    );
}
