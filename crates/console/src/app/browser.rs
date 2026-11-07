use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Direction;

use super::{App, MouseKind};
use crate::browser::BrowserPane;
use crate::model::PaneContent;

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
        self.split_share(Direction::Horizontal, 0.5);
        if self.state.focused_pane_id() == home {
            return;
        }
        if let Some(pane) = self.state.focused_pane_mut() {
            pane.content = PaneContent::Browser(Box::new(BrowserPane::new(url)));
        }
        self.state.focus_panes();
    }

    /// Only the panes the screen shows: a hidden pane's frames wait in Chrome's unacked queue.
    pub(super) fn pump_browsers(&mut self) {
        if !self
            .state
            .panes
            .values()
            .any(|pane| matches!(pane.content, PaneContent::Browser(_)))
        {
            return;
        }
        let (kitty, cell) = (self.kitty, crate::chrome::cell_pixels());
        let shown = crate::hit::drawn(self);
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

    /// A press on a browser pane focuses it before the page sees the click; a drag and its
    /// release go to the page only when its press began there.
    pub(super) fn browser_mouse(&mut self, kind: MouseKind, x: u16, y: u16) -> bool {
        let held = matches!(kind, MouseKind::Drag | MouseKind::Up);
        let id = match held {
            true => self.state.panes.iter().find_map(|(id, pane)| {
                matches!(&pane.content, PaneContent::Browser(page) if page.pressed()).then_some(*id)
            }),
            false => self.hits.as_ref().and_then(|hits| hits.pane_at(x, y)),
        };
        let Some(id) = id else {
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
            self.state.focus_panes();
            self.dirty = true;
        }
        true
    }

    /// An overlay takes every mouse event, so a press the page still holds can never see its
    /// release; ending it in Chrome returns drags to the console when the overlay closes.
    pub(super) fn release_browser_presses(&mut self) {
        let cell = crate::chrome::cell_pixels();
        for pane in self.state.panes.values_mut() {
            if let PaneContent::Browser(browser) = &mut pane.content {
                browser.release(cell);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    /// Dies with the browser chord reaching a third party unasked: with no local address to open,
    /// the pane waits for the owner to type one.
    #[test]
    fn a_browser_with_no_local_url_opens_the_bar() -> Result<(), Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        app.open_browser();
        let page = app
            .state
            .panes
            .values_mut()
            .find_map(|pane| match &mut pane.content {
                PaneContent::Browser(page) => Some(page),
                _ => None,
            })
            .ok_or("no browser pane")?;
        let (_, lines) = page.view(Rect::new(0, 0, 40, 6), true, &theme);
        let bar = lines.first().map(ToString::to_string).unwrap_or_default();
        assert!(
            bar.ends_with('\u{258f}'),
            "the bar is open for typing: {bar:?}"
        );
        assert!(
            !bar.contains("google"),
            "an address nobody asked for: {bar:?}"
        );
        Ok(())
    }
}
