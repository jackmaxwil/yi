use ratatui::crossterm::event::KeyEvent;
use yi_tui::keymap::SingleKey;
use yi_tui::mcp::McpPage;
use yi_tui::popup::{BottomView, PopupResult};

use crate::model::PaneContent;

use crate::app::App;

impl App {
    pub(in crate::app) fn on_page(&self) -> bool {
        self.on_settings() || self.on_mcp()
    }

    pub(in crate::app) fn page_key(&mut self, outbound: &crate::client::Outbound, key: KeyEvent) {
        if self.on_mcp() {
            self.mcp_key(key);
        } else {
            self.settings_key(outbound, key);
        }
    }

    pub(in crate::app) fn pump_panes(&mut self) {
        self.pump_browsers();
        self.poll_mcp();
    }

    fn on_mcp(&self) -> bool {
        self.state
            .focused_pane()
            .is_some_and(|pane| matches!(pane.content, PaneContent::Mcp(_)))
    }

    pub(in crate::app) fn open_mcp(&mut self) {
        let existing = self.state.tab().and_then(|tab| {
            tab.layout.pane_ids().into_iter().find(|id| {
                self.state
                    .panes
                    .get(id)
                    .is_some_and(|pane| matches!(pane.content, PaneContent::Mcp(_)))
            })
        });
        if let Some(id) = existing {
            if let Some(tab) = self.state.tabs.get_mut(self.state.active_tab) {
                tab.layout.focus_pane(id);
            }
        } else {
            self.state
                .split_focused(ratatui::layout::Direction::Horizontal);
            let page = McpPage::open(self.state.home.as_deref().map(Into::into));
            if let Some(pane) = self.state.focused_pane_mut() {
                pane.content = PaneContent::Mcp(Box::new(page));
            }
        }
        self.state.focus_panes();
        self.dirty = true;
    }

    fn mcp_key(&mut self, event: KeyEvent) {
        let Some(key) = SingleKey::from_event(&event) else {
            return;
        };
        let close = match self.state.focused_pane_mut().map(|pane| &mut pane.content) {
            Some(PaneContent::Mcp(page)) => matches!(page.handle_key(&key), PopupResult::Close),
            _ => false,
        };
        if close {
            self.state.close_focused_pane();
        }
        self.dirty = true;
    }

    fn poll_mcp(&mut self) {
        for pane in self.state.panes.values_mut() {
            if let PaneContent::Mcp(page) = &mut pane.content
                && page.poll()
            {
                self.dirty = true;
            }
        }
    }
}
