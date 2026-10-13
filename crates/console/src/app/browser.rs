use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Direction;

use super::{App, MouseKind};
use crate::browser::BrowserPane;
use crate::model::{PaneContent, Zone};

impl App {
    fn focused_browser(&mut self) -> Option<&mut BrowserPane> {
        match &mut self.state.focused_pane_mut()?.content {
            PaneContent::Browser(browser) => Some(browser),
            _ => None,
        }
    }

    /// Beside the focused pane, at the last local URL its session printed; on a browser pane
    /// the same chord closes it.
    pub(super) fn open_browser(&mut self) {
        if self.focused_browser().is_some() {
            self.state.close_focused_pane();
            return;
        }
        let url = self
            .state
            .focused_session()
            .and_then(|id| self.state.chat(&id))
            .and_then(|chat| crate::browser::last_local_url(chat.app.sources()));
        let home = self.state.focused_pane_id();
        self.split_with_anim(Direction::Horizontal);
        if self.state.focused_pane_id() == home {
            return;
        }
        if let Some(pane) = self.state.focused_pane_mut() {
            pane.content = PaneContent::Browser(Box::new(BrowserPane::new(url)));
        }
        self.state.zone = Zone::Panes;
    }

    /// Only the panes the screen shows: a hidden pane's frames wait in Chrome's unacked queue.
    pub(super) fn pump_browsers(&mut self) {
        let (kitty, cell) = (self.kitty, crate::chrome::cell_pixels());
        let shown: Vec<_> = (self.hits.iter())
            .flat_map(|hits| hits.panes.iter().map(|(id, _)| *id))
            .collect();
        let state = &mut self.state;
        for id in shown {
            if let Some(PaneContent::Browser(browser)) =
                state.panes.get_mut(&id).map(|pane| &mut pane.content)
            {
                self.dirty |= browser.pump(kitty, cell, &mut state.chrome);
            }
        }
    }

    pub(super) fn browser_key(&mut self, key: &KeyEvent) -> bool {
        let taken = self
            .focused_browser()
            .is_some_and(|browser| browser.key(key));
        self.dirty |= taken;
        taken
    }

    pub(super) fn browser_paste(&mut self, text: &str) -> bool {
        let Some(browser) = self.focused_browser() else {
            return false;
        };
        browser.paste(text);
        true
    }

    /// A press on a browser pane focuses it before the page sees the click.
    pub(super) fn browser_mouse(&mut self, kind: MouseKind, x: u16, y: u16) -> bool {
        let Some(id) = self.hits.as_ref().and_then(|hits| hits.pane_at(x, y)) else {
            return false;
        };
        let cell = crate::chrome::cell_pixels();
        let Some(PaneContent::Browser(browser)) =
            self.state.panes.get_mut(&id).map(|pane| &mut pane.content)
        else {
            return false;
        };
        if !browser.mouse(kind, x, y, cell) {
            return false;
        }
        if kind == MouseKind::Down {
            if let Some(tab) = self.state.tab_mut() {
                tab.layout.focus_pane(id);
            }
            self.state.zone = Zone::Panes;
            self.dirty = true;
        }
        true
    }
}
