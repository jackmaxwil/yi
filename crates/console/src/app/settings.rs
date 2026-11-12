use ratatui::crossterm::event::KeyEvent;
use serde_json::{Value, json};
use yi_types::acp::AcpConfigOption;

use super::{App, RequestKind};
use crate::client::Outbound;
use crate::layout::PaneId;
use crate::model::{PaneContent, SessionId};
use crate::settings::{Intent, Settings, Target, ThemePreview};
use yi_tui::colors::Theme;
use yi_tui::theme_file::{BUILT_IN, ground_id, themes_dir};

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
        self.fill_theme_choices(pane, None);
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
                self.fill_theme_choices(pane, Some(row));
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
            Intent::PreviewTheme { id, name } => self.preview_theme(pane, &id, &name),
            Intent::RestoreTheme => self.restore_theme(),
            Intent::SaveDefault { id, value } => {
                if yi_tui::theme_file::is_theme_row(&id) {
                    self.preview_theme(pane, &id, value.as_str().unwrap_or(BUILT_IN));
                    if let Some(preview) = &mut self.state.theme_preview {
                        preview.saving = true;
                    }
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
        let theme_saved = self
            .state
            .theme_preview
            .as_ref()
            .map(|preview| preview.saving);
        if saved && theme_saved == Some(true) {
            match reply.pointer("/error/message") {
                Some(_) => self.restore_theme(),
                None => self.state.theme_preview = None,
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
}

impl App {
    fn fill_theme_choices(&mut self, pane: PaneId, row: Option<usize>) {
        let opens = self
            .settings_pane(pane)
            .is_some_and(|settings| crate::settings::opens_theme_list(settings, row));
        if opens {
            let home = self.state.home.as_deref().map(std::path::Path::new);
            let choices = yi_tui::theme_file::list(themes_dir(home).as_deref());
            if let Some(settings) = self.settings_pane(pane) {
                settings.theme_choices = choices;
            }
        }
    }

    /// Paints `name` when row `id` names this terminal's ground; the other row only saves.
    fn preview_theme(&mut self, pane: PaneId, id: &str, name: &str) {
        if id != ground_id(self.theme.dark) {
            return;
        }
        let preview = self
            .state
            .theme_preview
            .get_or_insert_with(|| ThemePreview {
                restore: self.theme,
                pane,
                id: id.to_owned(),
                shown: String::new(),
                warned: false,
                saving: false,
            });
        if preview.shown == name {
            return;
        }
        preview.shown = name.to_owned();
        let home = self.state.home.as_deref().map(std::path::Path::new);
        let dir = themes_dir(home);
        let loaded =
            yi_tui::theme_file::preview(self.theme.tier, self.theme.dark, dir.as_deref(), name);
        let warn = loaded
            .warning
            .filter(|_| !std::mem::replace(&mut preview.warned, true));
        if let Some(warning) = warn {
            self.note(&format!("warning: {warning}"));
        }
        self.install_theme(loaded.theme);
    }

    fn restore_theme(&mut self) {
        if let Some(preview) = self.state.theme_preview.take() {
            self.install_theme(preview.restore);
        }
    }

    /// A list closed by anything but Enter, a moved focus or a closed pane, puts the palette back.
    pub(crate) fn settle_theme_preview(&mut self) {
        let Some(preview) = &self.state.theme_preview else {
            return;
        };
        let open = self.state.focused_pane_id() == Some(preview.pane)
            && self
                .state
                .panes
                .get(&preview.pane)
                .is_some_and(|pane| match &pane.content {
                    PaneContent::Settings(settings) => {
                        crate::settings::theme_list_open(settings).as_ref() == Some(&preview.id)
                    }
                    _ => false,
                });
        if !open && !preview.saving {
            self.restore_theme();
        }
    }

    /// Every open chat and cached pane row repaints: their caches are keyed on the palette.
    fn install_theme(&mut self, theme: Theme) {
        self.theme = theme;
        for pane in self.state.panes.values_mut() {
            if let PaneContent::Session {
                chat: Some(chat), ..
            } = &mut pane.content
            {
                chat.app.set_theme(theme);
            }
        }
        for chat in self.state.parked.values_mut() {
            chat.app.set_theme(theme);
        }
        self.dirty = true;
    }
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

    fn listing() -> Result<(App, PaneId, PaneId, Outbound), Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        let chat = app.state.focused_pane_id().ok_or("no pane")?;
        let (events, _received) = crate::client::events();
        let socket = std::env::temp_dir().join("yi-theme-preview-no-daemon.sock");
        let (outbound, _threads) = crate::client::spawn(socket, events);
        app.open_settings(&outbound, 0);
        let pane = app
            .state
            .focused_pane_id()
            .filter(|id| *id != chat)
            .ok_or("no settings")?;
        let rows = yi_types::settings::rows();
        let interface = rows.iter().filter(|row| row.tab == "Interface");
        let cursor = interface.clone().position(|row| row.id == "theme.dark");
        app.absorb_settings(pane, false, &json!({"result": {"rows": rows}}));
        let settings = app.settings_pane(pane).ok_or("not a settings pane")?;
        let tab = yi_types::settings::TABS
            .iter()
            .position(|tab| *tab == "Interface");
        settings.tab = tab.ok_or("no Interface tab")? + 1;
        settings.cursor = cursor.ok_or("no theme.dark row")?;
        let enter = KeyEvent::from(ratatui::crossterm::event::KeyCode::Enter);
        app.settings_key(&outbound, enter);
        Ok((app, chat, pane, outbound))
    }

    #[test]
    fn a_preview_closed_by_moving_focus_puts_the_palette_back()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut app, chat, pane, outbound) = listing()?;
        let built = app.theme.accent;
        app.settings_key(
            &outbound,
            KeyEvent::from(ratatui::crossterm::event::KeyCode::Down),
        );
        assert_ne!(app.theme.accent, built, "alabaster painted the console");
        app.settle_theme_preview();
        assert_ne!(app.theme.accent, built, "an open list keeps its preview");
        app.focus_pane(chat);
        app.settle_theme_preview();
        assert_eq!(
            app.theme.accent, built,
            "settings pane {pane:?} lost focus mid-list"
        );
        outbound.shutdown();
        Ok(())
    }

    #[test]
    fn enter_on_this_grounds_list_keeps_the_saved_theme_painted()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut app, _chat, pane, outbound) = listing()?;
        let built = app.theme.accent;
        let key = |code| KeyEvent::from(code);
        app.settings_key(&outbound, key(ratatui::crossterm::event::KeyCode::Down));
        let chosen = app.theme.accent;
        app.settings_key(&outbound, key(ratatui::crossterm::event::KeyCode::Enter));
        app.settle_theme_preview();
        assert_eq!(
            app.theme.accent, chosen,
            "the list closed on Enter while the save was out"
        );
        let rows = yi_types::settings::rows();
        app.absorb_settings(pane, true, &json!({"result": {"rows": rows}}));
        app.settle_theme_preview();
        assert_eq!(app.theme.accent, chosen, "the saved theme snapped back");
        assert_ne!(chosen, built);
        outbound.shutdown();
        Ok(())
    }

    #[test]
    fn saving_the_other_grounds_theme_leaves_this_palette_alone()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut app, _chat, pane, outbound) = listing()?;
        let built = app.theme.accent;
        let saved = Intent::SaveDefault {
            id: "theme.light".to_owned(),
            value: json!("light-prism"),
        };
        app.settings_intent(&outbound, pane, None, saved);
        app.absorb_settings(
            pane,
            true,
            &json!({"result": {"rows": yi_types::settings::rows()}}),
        );
        app.settle_theme_preview();
        assert_eq!(
            app.theme.accent, built,
            "a light theme painted on a dark terminal"
        );
        outbound.shutdown();
        Ok(())
    }
}
