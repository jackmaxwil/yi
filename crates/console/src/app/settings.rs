use ratatui::crossterm::event::KeyEvent;
use serde_json::{Value, json};

use crate::client::Outbound;
use crate::layout::PaneId;
use crate::model::PaneContent;
use crate::settings::{Intent, Settings};

use super::{App, RequestKind};

impl App {
    pub(super) fn on_settings(&self) -> bool {
        self.state
            .focused_pane()
            .is_some_and(|pane| matches!(pane.content, PaneContent::Settings(_)))
    }

    fn settings_pane(&mut self, pane: PaneId) -> Option<&mut Settings> {
        match &mut self.state.panes.get_mut(&pane)?.content {
            PaneContent::Settings(settings) => Some(settings),
            _ => None,
        }
    }

    /// Focuses the tab's settings pane, opening one beside the focused pane if there is none.
    pub(super) fn open_settings(&mut self, outbound: &Outbound, tab: usize) {
        let session = self.state.focused_session();
        let open = self.state.tab().and_then(|layout| {
            layout.layout.pane_ids().into_iter().find(|id| {
                self.state
                    .panes
                    .get(id)
                    .is_some_and(|pane| matches!(pane.content, PaneContent::Settings(_)))
            })
        });
        let pane = match open {
            Some(pane) => pane,
            None => {
                let home = self.state.focused_pane_id();
                self.split_with_anim(ratatui::layout::Direction::Horizontal);
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

    pub(super) fn settings_key(&mut self, outbound: &Outbound, key: KeyEvent) {
        let Some(pane) = self.state.focused_pane_id() else {
            return;
        };
        let session = self
            .settings_pane(pane)
            .and_then(|settings| settings.session.clone());
        let options = session
            .as_ref()
            .and_then(|id| self.state.session_options.get(id))
            .cloned()
            .unwrap_or_default();
        let Some(settings) = self.settings_pane(pane) else {
            return;
        };
        settings.status = None;
        let intent = crate::settings::key(settings, key, &options);
        self.dirty = true;
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
