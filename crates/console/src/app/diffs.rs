use ratatui::layout::Direction;
use serde_json::{Value, json};
use yi_types::acp::AcpSessionUpdate;

use crate::client::Outbound;
use crate::keys::Action;
use crate::layout::PaneId;
use crate::model::{FileDiff, PaneContent, SessionId};

use super::{App, RequestKind};

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

fn is_side(content: &PaneContent, session: &SessionId) -> bool {
    match content {
        PaneContent::Notebook {
            session: Some(bound),
            ..
        }
        | PaneContent::SessionDiff { session: bound } => bound == session,
        _ => false,
    }
}

impl App {
    pub(super) fn absorb_edit(
        &mut self,
        outbound: &Outbound,
        session: &SessionId,
        update: &AcpSessionUpdate,
    ) {
        let AcpSessionUpdate::ToolCallUpdate {
            raw_output: Some(details),
            ..
        } = update
        else {
            return;
        };
        let Some(patch) = details.get("patch").and_then(Value::as_str) else {
            return;
        };
        let Some(path) = path_of_patch(patch) else {
            return;
        };
        let count = |key: &str| details.get(key).and_then(Value::as_u64).unwrap_or(0);
        let file = FileDiff {
            patch: patch.to_owned(),
            added: count("added"),
            removed: count("removed"),
            tracked: false,
        };
        self.state
            .diffs
            .entry(session.clone())
            .or_default()
            .insert(path.clone(), file);
        self.editors_touched(Some(&path));
        let root = self.root_of(session);
        if self.connected() {
            self.send_request(
                outbound,
                RequestKind::Tracked(session.clone(), vec![path.clone()]),
                "_yi/tracked",
                json!({"sessionId": session.0, "root": root, "paths": [path]}),
            );
        }
        self.dirty = true;
    }

    pub(super) fn absorb_tracked(&mut self, session: &SessionId, paths: &[String], result: &Value) {
        let flags = result.get("tracked").and_then(Value::as_array);
        let mut any = false;
        if let Some(diff) = self.state.diffs.get_mut(session) {
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
        if any {
            self.maybe_open_side(session, SideKind::Diff);
        }
        self.dirty = true;
    }

    pub(super) fn absorb_kernel(&mut self, session: &SessionId, update: &AcpSessionUpdate) {
        if let AcpSessionUpdate::ToolCallUpdate {
            title: Some(title), ..
        } = update
            && title == "ipython"
        {
            self.maybe_open_side(session, SideKind::Notebook);
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
        self.open_side(home, Some(session.clone()), kind);
        self.state.side_opened.insert(session.clone());
    }

    fn open_side(&mut self, home: PaneId, session: Option<SessionId>, kind: SideKind) {
        if let Some(tab) = self.state.tab_mut() {
            tab.layout.focus_pane(home);
        }
        self.split_with_anim(Direction::Horizontal);
        if let Some(pane) = self.state.focused_pane_mut() {
            pane.content = match (kind, session) {
                (SideKind::Diff, Some(session)) => PaneContent::SessionDiff { session },
                (_, session) => PaneContent::Notebook {
                    session,
                    cells: Vec::new(),
                    input: crate::model::notebook_input(),
                },
            };
            pane.scroll_from_bottom = 0;
        }
        if let Some(tab) = self.state.tab_mut() {
            tab.layout.focus_pane(home);
        }
        self.dirty = true;
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
        if kind == SideKind::Diff && session.is_none() {
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
