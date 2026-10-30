use ratatui::crossterm::event::KeyEvent;
use serde_json::{Value, json};
use yi_types::acp::AcpConfigOption;

use super::{App, RequestKind};
use crate::client::Outbound;
use crate::layout::PaneId;
use crate::model::{PaneContent, SessionId};
use crate::settings::{Intent, Settings, Target};

impl App {
    pub(super) fn on_settings(&self) -> bool {
        self.state
            .focused_pane()
            .is_some_and(|pane| matches!(pane.content, PaneContent::Settings(_)))
    }

    pub(super) fn open_settings_pane(&self) -> Option<PaneId> {
        self.state.tab()?.layout.pane_ids().into_iter().find(|id| {
            self.state
                .panes
                .get(id)
                .is_some_and(|pane| matches!(pane.content, PaneContent::Settings(_)))
        })
    }

    pub(super) fn settings_pane(&mut self, pane: PaneId) -> Option<&mut Settings> {
        match &mut self.state.panes.get_mut(&pane)?.content {
            PaneContent::Settings(settings) => Some(settings),
            _ => None,
        }
    }

    /// Focuses the tab's settings pane, opening one beside the focused pane if there is none.
    pub(super) fn open_settings(&mut self, outbound: &Outbound, tab: usize) {
        let session = self.state.focused_session();
        let pane = match self.open_settings_pane() {
            Some(pane) => pane,
            None => {
                let home = self.state.focused_pane_id();
                self.split_share(ratatui::layout::Direction::Horizontal, 0.5);
                let Some(pane) = self.state.focused_pane_id().filter(|id| Some(*id) != home) else {
                    return;
                };
                if let Some(slot) = self.state.panes.get_mut(&pane) {
                    slot.content =
                        PaneContent::Settings(Box::new(Settings::new(session.clone(), tab)));
                }
                pane
            }
        };
        if let Some(layout) = self.state.tab_mut() {
            layout.layout.focus_pane(pane);
        }
        self.state.zone = crate::model::Zone::Panes;
        if let Some(settings) = self.settings_pane(pane) {
            settings.tab = tab;
            settings.cursor = 0;
            if session.is_some() {
                settings.session = session;
            }
        }
        self.send_request(
            outbound,
            RequestKind::Settings(pane, false),
            "_yi/settings",
            json!({}),
        );
    }

    /// The settings pane under the press: the session it edits and its option list, once.
    fn settings_options(&mut self, pane: PaneId) -> (Option<SessionId>, Vec<AcpConfigOption>) {
        let session = self
            .settings_pane(pane)
            .and_then(|settings| settings.session.clone());
        let options = session
            .as_ref()
            .and_then(|id| self.state.session_options.get(id))
            .cloned()
            .unwrap_or_default();
        (session, options)
    }

    pub(super) fn settings_key(&mut self, outbound: &Outbound, key: KeyEvent) {
        let Some(pane) = self.state.focused_pane_id() else {
            return;
        };
        let (session, options) = self.settings_options(pane);
        let Some(settings) = self.settings_pane(pane) else {
            return;
        };
        settings.status = None;
        let intent = crate::settings::key(settings, key, &options);
        self.dirty = true;
        self.settings_intent(outbound, pane, session, intent);
    }

    /// A press on a tab opens it; a press on a row focuses it and works its control.
    pub(super) fn settings_press(&mut self, outbound: &Outbound, pane: PaneId, target: Target) {
        self.focus_pane(pane);
        self.dirty = true;
        match target {
            Target::Tab(tab) => {
                if let Some(settings) = self.settings_pane(pane) {
                    (settings.tab, settings.cursor) = (tab, 0);
                    (settings.edit, settings.status) = (None, None);
                }
            }
            Target::Row(row) => {
                let (session, options) = self.settings_options(pane);
                let Some(settings) = self.settings_pane(pane) else {
                    return;
                };
                let intent = crate::settings::click(settings, row, &options);
                self.settings_intent(outbound, pane, session, intent);
            }
        }
    }

    /// A wheel over the pane moves its cursor, so a row past the window is reachable by pointer.
    pub(super) fn settings_wheel(&mut self, pane: PaneId, delta: isize) -> bool {
        let (_, options) = self.settings_options(pane);
        let Some(settings) = self.settings_pane(pane) else {
            return false;
        };
        if settings.edit.is_some() {
            return true;
        }
        let last = crate::settings::count(settings, &options).saturating_sub(1);
        let moved = if delta >= 0 {
            settings.cursor.saturating_sub(1)
        } else {
            settings.cursor.saturating_add(1).min(last)
        };
        let changed = moved != settings.cursor;
        settings.cursor = moved;
        self.dirty |= changed;
        true
    }

    fn settings_intent(
        &mut self,
        outbound: &Outbound,
        pane: PaneId,
        session: Option<SessionId>,
        intent: Intent,
    ) {
        match intent {
            Intent::None => {}
            Intent::Note(text) => {
                if let Some(settings) = self.settings_pane(pane) {
                    settings.status = Some(text);
                }
            }
            Intent::SaveDefault { id, value } => self.send_request(
                outbound,
                RequestKind::Settings(pane, true),
                "_yi/set_default",
                json!({"id": id, "value": value}),
            ),
            Intent::SetSession { config_id, value } => {
                if let Some(session) = session {
                    self.send_request(
                        outbound,
                        RequestKind::SetConfig(session.clone()),
                        "session/set_config_option",
                        json!({"sessionId": session.0, "configId": config_id, "type": "id", "value": value}),
                    );
                }
            }
        }
    }

    /// A reply that belongs to the pane that asked, error or not; false for every other kind.
    pub(super) fn pane_reply(
        &mut self,
        outbound: &Outbound,
        kind: &RequestKind,
        reply: &Value,
    ) -> bool {
        if let RequestKind::Settings(pane, saved) = kind {
            self.absorb_settings(*pane, *saved, reply);
            return true;
        }
        if let RequestKind::Fork(pane) = kind {
            self.absorb_fork(outbound, *pane, reply);
            return true;
        }
        false
    }

    /// The daemon's rows, or its refusal named on the pane; the console's own rows apply here.
    pub(super) fn absorb_settings(&mut self, pane: PaneId, saved: bool, reply: &Value) {
        self.dirty = true;
        let refusal = reply.pointer("/error/message").and_then(Value::as_str);
        let rows = reply.pointer("/result/rows").cloned().and_then(|rows| {
            serde_json::from_value::<Vec<yi_types::settings::SettingRow>>(rows).ok()
        });
        if let Some(rows) = &rows {
            let side = rows.iter().find(|row| row.id == "console.autoSide");
            self.state.auto_side = side.and_then(|row| row.value.as_bool()).unwrap_or(true);
        }
        let Some(settings) = self.settings_pane(pane) else {
            return;
        };
        match (refusal, rows) {
            (Some(why), _) => settings.status = Some(format!("not saved: {why}")),
            (None, Some(rows)) => {
                if saved {
                    settings.status = Some("saved".to_owned());
                }
                settings.rows = Some(rows);
            }
            (None, None) => settings.status = Some("the daemon sent no settings".to_owned()),
        }
    }
}
