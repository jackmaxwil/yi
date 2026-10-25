use std::time::{Duration, Instant};

use ratatui::layout::Direction;
use serde_json::{Value, json};
use yi_types::acp::{AcpSessionUpdate, AcpToolCallStatus, AcpToolCallUpdate};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content};

use crate::client::Outbound;
use crate::keys::Action;
use crate::layout::PaneId;
use crate::model::{FileDiff, PaneContent, SessionId};

use super::notebook::apply_notebook;
use super::{App, RequestKind};

pub(crate) const NOTEBOOK_IDLE: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SideKind {
    Notebook,
    Diff,
}

pub(super) fn path_of_patch(patch: &str) -> Option<String> {
    patch
        .lines()
        .find_map(|line| line.strip_prefix("+++ b/"))
        .map(str::to_owned)
}

pub(super) fn replayed_updates(message: &AgentMessage) -> Vec<AcpSessionUpdate> {
    match message {
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|block| match block {
                Content::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => Some(AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
                    tool_call_id: id.clone(),
                    title: Some(name.clone()),
                    raw_input: Some(Value::Object(arguments.clone())),
                    ..AcpToolCallUpdate::default()
                })),
                _ => None,
            })
            .collect(),
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            details,
            ..
        } => {
            let plots = (tool_name == "ipython").then(|| {
                (content.iter())
                    .filter(|block| matches!(block, Content::Image { .. }))
                    .filter_map(|block| {
                        serde_json::from_value(json!({"type": "content", "content": block})).ok()
                    })
                    .collect()
            });
            vec![AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
                tool_call_id: tool_call_id.clone(),
                status: Some(AcpToolCallStatus::Completed),
                raw_output: details.clone(),
                content: plots,
                ..AcpToolCallUpdate::default()
            })]
        }
        _ => Vec::new(),
    }
}

fn is_side(content: &PaneContent, session: &SessionId) -> bool {
    match content {
        PaneContent::Notebook {
            session: Some(bound),
            ..
        }
        | PaneContent::SessionDiff { session: bound, .. } => bound == session,
        _ => false,
    }
}

impl App {
    /// Invariant: every write to a session's diff comes through here, so its revision is what
    /// tells a drawn Review its rows went stale.
    pub(super) fn diff_mut(&mut self, session: &SessionId) -> &mut crate::model::SessionDiff {
        let diff = self.state.diffs.entry(session.clone()).or_default();
        diff.rev = diff.rev.wrapping_add(1);
        diff
    }

    pub(super) fn absorb_edit(
        &mut self,
        outbound: &Outbound,
        session: &SessionId,
        update: &AcpSessionUpdate,
    ) {
        if let Some(path) = self.record_edit(session, update, None) {
            self.track(outbound, session, vec![path], true);
        }
    }

    /// `replayed` is the turn a replayed edit ran in; a live edit takes the console's own turn
    /// and the todo running now.
    fn record_edit(
        &mut self,
        session: &SessionId,
        update: &AcpSessionUpdate,
        replayed: Option<u64>,
    ) -> Option<String> {
        let AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
            raw_output: Some(details),
            ..
        }) = update
        else {
            return None;
        };
        let patch = details.get("patch").and_then(Value::as_str)?;
        let path = path_of_patch(patch)?;
        let count = |key: &str| details.get(key).and_then(Value::as_u64).unwrap_or(0);
        let file = FileDiff {
            patch: patch.to_owned(),
            added: count("added"),
            removed: count("removed"),
            tracked: false,
            serving: replayed.map_or_else(|| self.serving(session), |_| None),
            turn: replayed
                .unwrap_or_else(|| self.state.diffs.get(session).map_or(0, |diff| diff.turn)),
        };
        self.diff_mut(session).insert(path.clone(), file);
        self.editors_touched(Some(&path));
        Some(path)
    }

    fn track(&mut self, outbound: &Outbound, session: &SessionId, paths: Vec<String>, open: bool) {
        let root = self.root_of(session);
        if self.connected() {
            self.send_request(
                outbound,
                RequestKind::Tracked(session.clone(), paths.clone(), open),
                "_yi/tracked",
                json!({"sessionId": session.0, "root": root, "paths": paths}),
            );
        }
        self.dirty = true;
    }

    /// Replay entries are whole messages, so they fill the Notebook and Review as live frames do;
    /// `full`, a replay from entry 0, is the branch as it stands and restarts the Notebook.
    pub(super) fn absorb_history(
        &mut self,
        outbound: &Outbound,
        session: &SessionId,
        entries: &[Entry],
        full: bool,
    ) {
        let kept = self.state.diffs.get(session).filter(|_| !full);
        let mut turn = kept.map_or(0, |diff| diff.turn);
        if full && self.state.notebooks.remove(session).is_some() {
            self.redraw_notebooks(session);
        }
        let mut edited = std::collections::BTreeSet::new();
        for entry in entries {
            let Entry::Message {
                message, timestamp, ..
            } = entry
            else {
                continue;
            };
            if let AgentMessage::User { attribution, .. } = message
                && attribution.reads_as_typed(*timestamp)
            {
                turn = turn.saturating_add(1);
            }
            for update in replayed_updates(message) {
                self.absorb_review(session, &update);
                edited.extend(self.record_edit(session, &update, Some(turn)));
                self.fill_notebook(session, &update);
            }
        }
        if self.state.diffs.contains_key(session) {
            self.diff_mut(session).turn = turn;
        }
        if !edited.is_empty() {
            self.track(outbound, session, edited.into_iter().collect(), false);
        }
    }

    pub(super) fn absorb_tracked(
        &mut self,
        session: &SessionId,
        paths: &[String],
        result: &Value,
        open: bool,
    ) {
        let flags = result.get("tracked").and_then(Value::as_array);
        let mut any = false;
        if self.state.diffs.contains_key(session) {
            let diff = self.diff_mut(session);
            for (index, path) in paths.iter().enumerate() {
                let tracked = flags
                    .and_then(|flags| flags.get(index))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(file) = diff.file_mut(path) {
                    file.tracked = tracked;
                    any |= tracked;
                }
            }
        }
        if any && open {
            self.maybe_open_side(session, SideKind::Diff);
        }
        self.dirty = true;
    }

    pub(super) fn absorb_kernel(&mut self, session: &SessionId, update: &AcpSessionUpdate) {
        if let AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
            title: Some(title), ..
        }) = update
            && title == "ipython"
        {
            self.maybe_open_side(session, SideKind::Notebook);
        }
    }

    pub(super) fn fill_notebook(&mut self, session: &SessionId, update: &AcpSessionUpdate) {
        if matches!(update, AcpSessionUpdate::ToolCallUpdate(_))
            && apply_notebook(
                self.state.notebooks.entry(session.clone()).or_default(),
                update,
            )
        {
            self.redraw_notebooks(session);
        }
    }

    fn redraw_notebooks(&mut self, session: &SessionId) {
        for (pane_id, pane) in &mut self.state.panes {
            if pane.session() == Some(session)
                && let PaneContent::Notebook { drawn, .. } = &mut pane.content
            {
                *drawn = None;
                pane.scroll_from_bottom = 0;
                if let Some(seen) = self.auto_notebooks.get_mut(pane_id) {
                    *seen = Instant::now();
                }
            }
        }
    }

    fn root_of(&self, session: &SessionId) -> String {
        self.state
            .sessions
            .get(session)
            .map_or_else(|| self.state.root.clone(), |row| row.root.clone())
    }

    fn maybe_open_side(&mut self, session: &SessionId, kind: SideKind) {
        if !self.state.auto_side || self.state.side_opened.contains(session) {
            return;
        }
        let Some(tab) = self.state.tab() else {
            return;
        };
        let ids = tab.layout.pane_ids();
        let panes = &self.state.panes;
        if ids
            .iter()
            .filter_map(|id| panes.get(id))
            .any(|pane| is_side(&pane.content, session))
        {
            self.state.side_opened.insert(session.clone());
            return;
        }
        let home = ids.iter().copied().find(|id| {
            panes.get(id).is_some_and(|pane| {
                matches!(&pane.content, PaneContent::Session { session: Some(bound), .. } if bound == session)
            })
        });
        let Some(home) = home else {
            return;
        };
        let opened = self.open_side(home, Some(session.clone()), kind);
        if let (SideKind::Notebook, Some(opened)) = (kind, opened) {
            self.auto_notebooks.insert(opened, Instant::now());
        }
        self.state.side_opened.insert(session.clone());
    }

    pub(super) fn close_idle_notebooks(&mut self, now: Instant) {
        let focused = self.state.focused_pane_id();
        let due: Vec<PaneId> = self
            .auto_notebooks
            .iter()
            .filter(|(_, seen)| now.saturating_duration_since(**seen) >= NOTEBOOK_IDLE)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            let session = match self.state.panes.get(&id).map(|pane| &pane.content) {
                Some(PaneContent::Notebook { session, .. }) => {
                    let running = session.as_ref().is_some_and(|session| {
                        crate::model::notebook_cells(&self.state.notebooks, session)
                            .iter()
                            .any(|cell| cell.running)
                    });
                    if running {
                        continue;
                    }
                    session.clone()
                }
                _ => None,
            };
            self.auto_notebooks.remove(&id);
            if Some(id) == focused {
                continue;
            }
            let on_tab = self
                .state
                .tab()
                .is_some_and(|tab| tab.layout.pane_ids().contains(&id));
            if !on_tab {
                continue;
            }
            if let Some(tab) = self.state.tab_mut() {
                tab.layout.focus_pane(id);
            }
            self.state.close_focused_pane();
            if let Some(session) = session {
                self.state.side_opened.remove(&session);
            }
            self.dirty = true;
        }
    }

    fn open_side(
        &mut self,
        home: PaneId,
        session: Option<SessionId>,
        kind: SideKind,
    ) -> Option<PaneId> {
        if let (SideKind::Diff, Some(session)) = (kind, &session) {
            self.refresh_branch(session);
        }
        if let Some(tab) = self.state.tab_mut() {
            tab.layout.focus_pane(home);
        }
        self.split_with_anim(Direction::Horizontal);
        let opened = self.state.focused_pane_id().filter(|id| *id != home);
        if let Some(pane) = self.state.focused_pane_mut() {
            (pane.content, pane.scroll_from_bottom) = match (kind, session) {
                (SideKind::Diff, Some(session)) => {
                    let scope = crate::model::ReviewScope::default();
                    let drawn = None;
                    (
                        PaneContent::SessionDiff {
                            session,
                            scope,
                            drawn,
                        },
                        usize::MAX,
                    )
                }
                (_, session) => {
                    let (full, drawn) = (false, None);
                    (
                        PaneContent::Notebook {
                            session,
                            full,
                            drawn,
                        },
                        0,
                    )
                }
            };
        }
        if let Some(tab) = self.state.tab_mut() {
            tab.layout.focus_pane(home);
        }
        self.dirty = true;
        opened
    }

    pub(super) fn toggle_side(&mut self, outbound: &Outbound, kind: SideKind) {
        let Some(home) = self.state.focused_pane_id() else {
            return;
        };
        let on_side = self.state.panes.get(&home).is_some_and(|pane| match kind {
            SideKind::Notebook => matches!(pane.content, PaneContent::Notebook { .. }),
            SideKind::Diff => matches!(pane.content, PaneContent::SessionDiff { .. }),
        });
        if on_side {
            return self.apply_action(outbound, Action::ClosePane);
        }
        let session = self.state.focused_session();
        if kind != SideKind::Notebook && session.is_none() {
            return self.note("no session in this pane");
        }
        self.open_side(home, session, kind);
        let matches_kind = |pane: &crate::model::Pane| match kind {
            SideKind::Notebook => matches!(pane.content, PaneContent::Notebook { .. }),
            SideKind::Diff => matches!(pane.content, PaneContent::SessionDiff { .. }),
        };
        let side = self
            .state
            .tab()
            .map(|tab| tab.layout.pane_ids())
            .unwrap_or_default()
            .into_iter()
            .find(|id| *id != home && self.state.panes.get(id).is_some_and(matches_kind));
        if let (Some(side), Some(tab)) = (side, self.state.tab_mut()) {
            tab.layout.focus_pane(side);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NbCell;

    fn app_with_session() -> (App, SessionId) {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        let session = SessionId("s-alpha".to_owned());
        let home = app.state.focused_pane_id().expect("a pane");
        if let Some(pane) = app.state.panes.get_mut(&home) {
            pane.content = PaneContent::Session {
                session: Some(session.clone()),
                chat: None,
            };
        }
        (app, session)
    }

    #[test]
    fn an_idle_auto_notebook_closes_and_the_next_cell_reopens_it() {
        let (mut app, session) = app_with_session();
        let home = app.state.focused_pane_id().expect("home");
        app.maybe_open_side(&session, SideKind::Notebook);
        assert_eq!(app.state.panes.len(), 2, "opened beside the chat");
        let later = Instant::now() + NOTEBOOK_IDLE;
        app.state.notebooks.insert(
            session.clone(),
            vec![NbCell {
                running: true,
                ..NbCell::default()
            }],
        );
        app.close_idle_notebooks(later);
        assert_eq!(app.state.panes.len(), 2, "a running cell holds it open");
        app.state.notebooks.clear();
        app.close_idle_notebooks(later);
        assert_eq!(app.state.panes.len(), 1, "closed once idle");
        assert_eq!(app.state.focused_pane_id(), Some(home), "focus back home");
        assert!(app.auto_notebooks.is_empty());
        app.maybe_open_side(&session, SideKind::Notebook);
        assert_eq!(app.state.panes.len(), 2, "the next cell opens it again");
    }

    /// Incident: closing the notebook and reopening it showed "no kernel cells yet" while the
    /// kernel still held every variable; a cell that ran while it was closed was also lost.
    #[test]
    fn cells_survive_the_notebook_closing_and_reopening() {
        use yi_types::acp::{AcpToolCallStatus, AcpUpdateParams};
        let (mut app, session) = app_with_session();
        let (events, _received) = crate::client::events();
        let socket = std::env::temp_dir().join("yi-notebook-persist-no-daemon.sock");
        let (outbound, _threads) = crate::client::spawn(socket, events);
        let send = |app: &mut App, update: AcpToolCallUpdate| {
            let update = AcpUpdateParams {
                session_id: session.0.clone(),
                update: AcpSessionUpdate::ToolCallUpdate(update),
            };
            app.reduce_update(&outbound, update);
        };
        let start = |id: &str, code: &str| AcpToolCallUpdate {
            tool_call_id: id.to_owned(),
            title: Some("ipython".to_owned()),
            raw_input: Some(json!({"code": code})),
            ..AcpToolCallUpdate::default()
        };
        let end = |id: &str, stdout: &str| AcpToolCallUpdate {
            tool_call_id: id.to_owned(),
            status: Some(AcpToolCallStatus::Completed),
            raw_output: Some(json!({"stdout": stdout})),
            ..AcpToolCallUpdate::default()
        };
        send(&mut app, start("c1", "x = 1 + 2"));
        send(&mut app, end("c1", "3\n"));
        let opened = *app.auto_notebooks.keys().next().expect("auto-opened");
        if let Some(tab) = app.state.tab_mut() {
            tab.layout.focus_pane(opened);
        }
        app.state.close_focused_pane();
        assert_eq!(app.state.panes.len(), 1, "closed");
        send(&mut app, start("c2", "x * 2"));
        send(&mut app, end("c2", "6\n"));
        outbound.shutdown();
        app.state.side_opened.clear();
        app.maybe_open_side(&session, SideKind::Notebook);
        assert_eq!(app.state.panes.len(), 2, "reopened");
        let cells = &app.state.notebooks[&session];
        let seen: Vec<_> = cells
            .iter()
            .map(|c| (c.code.as_str(), c.stdout.as_str()))
            .collect();
        assert_eq!(seen, [("x = 1 + 2", "3\n"), ("x * 2", "6\n")]);
    }

    /// Incident: re-pointing a notebook pane to another session kept the rows cached for the
    /// first, so the title named the new session over the old one's cells.
    #[test]
    fn a_repointed_notebook_pane_shows_the_new_sessions_cells() {
        let (mut app, alpha) = app_with_session();
        let beta = SessionId("s-beta".to_owned());
        let pane = app.state.focused_pane_id().expect("pane");
        let cell = |code: &str| {
            vec![NbCell {
                number: 1,
                code: code.to_owned(),
                ..NbCell::default()
            }]
        };
        app.state
            .notebooks
            .insert(alpha.clone(), cell("alpha_marker"));
        app.state
            .notebooks
            .insert(beta.clone(), cell("beta_marker"));
        if let Some(slot) = app.state.panes.get_mut(&pane) {
            slot.content = PaneContent::Notebook {
                session: Some(alpha),
                full: false,
                drawn: None,
            };
        }
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let area = ratatui::layout::Rect::new(0, 0, 100, 30);
        let shown = |app: &mut App| -> String {
            let view = crate::render::compute_view(app, area, &theme);
            view.panes
                .iter()
                .flat_map(|pane| &pane.lines)
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(shown(&mut app).contains("alpha_marker"));
        app.bind_pane(pane, &beta);
        let after = shown(&mut app);
        assert!(after.contains("beta_marker"), "{after}");
        assert!(!after.contains("alpha_marker"), "{after}");
    }

    #[test]
    fn a_focused_auto_notebook_becomes_the_users() {
        let (mut app, session) = app_with_session();
        app.maybe_open_side(&session, SideKind::Notebook);
        let opened = *app.auto_notebooks.keys().next().expect("tracked");
        if let Some(tab) = app.state.tab_mut() {
            tab.layout.focus_pane(opened);
        }
        app.close_idle_notebooks(Instant::now() + NOTEBOOK_IDLE);
        assert_eq!(app.state.panes.len(), 2, "stays open");
        assert!(app.auto_notebooks.is_empty(), "no longer tracked");
    }
}
