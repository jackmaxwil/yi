use ratatui::crossterm::event::KeyEvent;
use serde_json::{Value, json};
use yi_tui::colors::Theme;
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
        let home = self.state.home.clone();
        let Some(settings) = self.settings_pane(pane) else {
            return;
        };
        settings.status = None;
        fill_theme_choices(settings, &home);
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
                let open = self
                    .settings_pane(pane)
                    .is_some_and(|settings| crate::settings::theme_list_open(settings));
                if open {
                    self.restore_theme();
                }
                if let Some(settings) = self.settings_pane(pane) {
                    (settings.tab, settings.cursor) = (tab, 0);
                    (settings.edit, settings.status) = (None, None);
                }
            }
            Target::Row(row) => {
                let open = self
                    .settings_pane(pane)
                    .is_some_and(|settings| crate::settings::theme_list_open(settings));
                if open {
                    self.restore_theme();
                }
                let (session, options) = self.settings_options(pane);
                let home = self.state.home.clone();
                let Some(settings) = self.settings_pane(pane) else {
                    return;
                };
                fill_theme_choices(settings, &home);
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
            Intent::PreviewTheme(name) => {
                self.remember_theme();
                self.preview_theme(&name);
            }
            Intent::RestoreTheme => self.restore_theme(),
            Intent::SaveDefault { id, value } => {
                if id == "theme.dark" || id == "theme.light" {
                    let name = value
                        .as_str()
                        .unwrap_or(yi_tui::theme_file::BUILT_IN)
                        .to_owned();
                    self.remember_theme();
                    self.preview_theme(&name);
                }
                self.send_request(
                    outbound,
                    RequestKind::Settings(pane, true),
                    "_yi/set_default",
                    json!({"id": id, "value": value}),
                );
            }
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
        if saved {
            let refused = reply.pointer("/error/message").is_some();
            if refused {
                self.restore_theme();
            } else {
                self.theme_restore = None;
                self.theme_warned = false;
            }
        }
        let refusal = reply.pointer("/error/message").and_then(Value::as_str);
        let rows = reply.pointer("/result/rows").cloned().and_then(|rows| {
            serde_json::from_value::<Vec<yi_types::settings::SettingRow>>(rows).ok()
        });
        if let Some(rows) = &rows {
            let side = rows.iter().find(|row| row.id == "console.autoSide");
            self.state.auto_side = side.and_then(|row| row.value.as_bool()).unwrap_or(true);
        }
        let follow = rows.as_ref().and_then(|rows| {
            rows.iter()
                .find(|row| row.id == "tui.follow")
                .and_then(|row| {
                    serde_json::from_value::<yi_types::config::FollowMode>(row.value.clone()).ok()
                })
        });
        if rows.is_some() {
            self.state
                .set_follow(follow.unwrap_or(self.state.follow_fallback));
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

    fn themes_dir(&self) -> std::path::PathBuf {
        let home = self
            .state
            .home
            .clone()
            .or_else(|| std::env::var("HOME").ok());
        match home {
            Some(home) => std::path::PathBuf::from(home).join(".yi").join("themes"),
            None => std::path::PathBuf::from(".yi").join("themes"),
        }
    }

    fn remember_theme(&mut self) {
        if self.theme_restore.is_none() {
            self.theme_restore = Some(self.theme);
            self.theme_warned = false;
        }
    }

    fn preview_theme(&mut self, name: &str) {
        let loaded =
            yi_tui::theme_file::preview(self.theme.tier, self.theme.dark, &self.themes_dir(), name);
        if let Some(warning) = loaded.warning.filter(|_| !self.theme_warned) {
            self.theme_warned = true;
            self.note(&format!("warning: {warning}"));
        }
        self.install_theme(loaded.theme);
    }

    fn restore_theme(&mut self) {
        self.theme_warned = false;
        if let Some(theme) = self.theme_restore.take() {
            self.install_theme(theme);
        }
    }

    fn install_theme(&mut self, theme: Theme) {
        self.theme = theme;
        for pane in self.state.panes.values_mut() {
            match &mut pane.content {
                crate::model::PaneContent::Session {
                    chat: Some(chat), ..
                } => chat.app.set_theme(theme),
                crate::model::PaneContent::Markdown { drawn, .. }
                | crate::model::PaneContent::Diff { drawn, .. } => *drawn = None,
                crate::model::PaneContent::Notebook { drawn, .. } => *drawn = None,
                crate::model::PaneContent::SessionDiff { drawn, .. } => *drawn = None,
                _ => {}
            }
        }
        for chat in self.state.parked.values_mut() {
            chat.app.set_theme(theme);
        }
        // A resize throws away the frame and the images. A theme change has to do the same,
        // or rows and pictures painted at startup stay on that palette until the next launch.
        self.avatars.forget();
        self.pending_clear = true;
        self.dirty = true;
    }
}

fn fill_theme_choices(settings: &mut crate::settings::Settings, home: &Option<String>) {
    let dir = home
        .as_ref()
        .map(|home| std::path::PathBuf::from(home).join(".yi").join("themes"))
        .unwrap_or_else(|| std::path::PathBuf::from(".yi").join("themes"));
    settings.theme_choices = yi_tui::theme_file::list(&dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionId;

    #[test]
    fn follow_applies_live_to_visible_and_parked_chats() {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        let session = SessionId("s-alpha".to_owned());
        let pane = app.state.focused_pane_id().expect("a chat pane");
        let chat = app.make_chat(&session);
        if let Some(slot) = app.state.panes.get_mut(&pane) {
            slot.content = PaneContent::Session {
                session: Some(session.clone()),
                chat: Some(chat),
            };
        }
        app.state
            .parked
            .insert(session, app.make_chat(&SessionId("s-alpha".to_owned())));
        let mut rows = yi_types::settings::rows();
        for row in &mut rows {
            if row.id == "tui.follow" {
                row.value = Value::String("still".to_owned());
            }
        }
        app.absorb_settings(pane, true, &json!({"result": {"rows": rows}}));
        assert_eq!(app.state.chat_follow, yi_types::config::FollowMode::Still);
        for chat in app
            .state
            .panes
            .values()
            .filter_map(|pane| match &pane.content {
                PaneContent::Session {
                    chat: Some(chat), ..
                } => Some(chat),
                _ => None,
            })
        {
            assert_eq!(chat.app.follow(), yi_types::config::FollowMode::Still);
        }
        for chat in app.state.parked.values() {
            assert_eq!(chat.app.follow(), yi_types::config::FollowMode::Still);
        }
    }
}
