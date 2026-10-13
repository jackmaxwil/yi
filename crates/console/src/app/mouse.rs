use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use crate::client::Outbound;
use crate::keys::Action;
use crate::layout::PaneId;
use crate::model::{Editor, Mode, PaneContent, SessionStatus, Zone};
use crate::sidebar::Control;

use super::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    RightDown,
    Moved,
    Up,
    Drag,
    ScrollUp,
    ScrollDown,
}

impl App {
    pub(super) fn mouse_event(&mut self, outbound: &Outbound, mouse: MouseEvent) {
        let kind = match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => MouseKind::RightDown,
            MouseEventKind::Down(_) => MouseKind::Down,
            MouseEventKind::Up(_) => MouseKind::Up,
            MouseEventKind::Drag(_) => MouseKind::Drag,
            MouseEventKind::Moved => MouseKind::Moved,
            MouseEventKind::ScrollUp => MouseKind::ScrollUp,
            MouseEventKind::ScrollDown => MouseKind::ScrollDown,
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => return,
        };
        self.handle_mouse(outbound, kind, mouse.column, mouse.row);
    }

    pub fn hovered(&self) -> Option<ratatui::layout::Rect> {
        let (x, y) = self.pointer?;
        self.hits.as_ref()?.hover_at(x, y)
    }

    /// A drag copies without the user reaching for a key.
    pub(super) fn copy_out(&mut self, text: &str) {
        self.osc_out.push(crate::select::osc52(text));
        let lines = text.lines().count();
        let noun = if lines == 1 { "line" } else { "lines" };
        self.flash = Some((format!("copied {lines} {noun}"), self.clock.now()));
        self.dirty = true;
    }

    /// A drag held past a pane's top or bottom text row scrolls it, one step per drawn
    /// frame so every row passes through the view the copy is read from.
    pub(super) fn scroll_drag(&mut self) {
        let Some(drag) = &self.selection else { return };
        let step = drag.edge_step();
        if self.dirty || step == 0 {
            return;
        }
        if let Some(pane) = self.state.panes.get_mut(&drag.pane) {
            let scroll = pane.scroll_from_bottom.saturating_add_signed(step);
            self.dirty = scroll != pane.scroll_from_bottom;
            pane.scroll_from_bottom = scroll;
        }
    }

    fn scroll_pane_under(&mut self, x: u16, y: u16, delta: isize) {
        let target = self.hits.as_ref().and_then(|hits| hits.pane_at(x, y));
        if let Some(pane_id) = target
            && let Some(pane) = self.state.panes.get_mut(&pane_id)
        {
            if let PaneContent::Editor(editor) = &mut pane.content {
                editor.scroll_top = if delta >= 0 {
                    editor.scroll_top.saturating_sub(delta.unsigned_abs())
                } else {
                    editor.scroll_top.saturating_add(delta.unsigned_abs())
                };
                self.dirty = true;
                return;
            }
            pane.scroll_from_bottom = if delta >= 0 {
                pane.scroll_from_bottom.saturating_add(delta.unsigned_abs())
            } else {
                pane.scroll_from_bottom.saturating_sub(delta.unsigned_abs())
            };
            self.dirty = true;
        }
    }

    fn editor_cell(&self, pane_id: PaneId, x: u16, y: u16) -> Option<(usize, usize)> {
        let (_, rect) = self
            .hits
            .as_ref()?
            .panes
            .iter()
            .find(|(id, _)| *id == pane_id)?;
        let inner = rect.inner(ratatui::layout::Margin::new(1, 1));
        let PaneContent::Editor(editor) = &self.state.panes.get(&pane_id)?.content else {
            return None;
        };
        let row = usize::from(y.checked_sub(inner.y)?).saturating_add(editor.scroll_top);
        let col = usize::from(x.checked_sub(inner.x)?).checked_sub(editor.gutter())?;
        Some((row, col))
    }

    fn closer_at(&self, x: u16, y: u16) -> Option<PaneId> {
        let hits = self.hits.as_ref()?;
        let (pane, _) = hits
            .closers
            .iter()
            .find(|(_, rect)| rect.contains(Position::new(x, y)))?;
        Some(*pane)
    }

    fn press(&mut self, outbound: &Outbound, x: u16, y: u16) {
        // Any new click drops the last drag's highlight.
        if self.selection.take().is_some() {
            self.dirty = true;
        }
        let Some(hits) = &self.hits else { return };
        // Incident: a lower pane's mark sits on the split it can be dragged by, and a press meant
        // as a resize closed the pane; a mark closes on a release over it that did not drag.
        self.pending_close = self.closer_at(x, y);
        if let Some((_, index)) = hits
            .tabs
            .iter()
            .find(|(rect, _)| rect.contains(Position::new(x, y)))
        {
            let index = *index;
            self.state.select_tab(index);
            self.dirty = true;
            return;
        }
        let strip = hits.settings_tabs.iter();
        if let Some((_, pane, tab)) = strip.copied().find(|(r, ..)| r.contains((x, y).into())) {
            if let Some(layout) = self.state.tab_mut() {
                layout.layout.focus_pane(pane);
            }
            self.state.zone = Zone::Panes;
            if let Some(settings) = self.settings_pane(pane) {
                (settings.tab, settings.cursor) = (tab, 0);
            }
            self.dirty = true;
            return;
        }
        let hint = hits
            .hint_buttons
            .iter()
            .find(|(rect, _)| rect.contains(Position::new(x, y)));
        if let Some((_, action)) = hint {
            return self.apply_action(outbound, *action);
        }
        if hits
            .gear
            .is_some_and(|gear| gear.contains(Position::new(x, y)))
        {
            let Some(pane) = self.open_settings_pane() else {
                return self.apply_action(outbound, Action::Settings);
            };
            if let Some(tab) = self.state.tab_mut() {
                tab.layout.focus_pane(pane);
            }
            self.dirty = true;
            return self.apply_action(outbound, Action::ClosePane);
        }
        if x < hits.sidebar_width {
            let control = hits
                .controls
                .iter()
                .find(|(rect, _)| rect.contains((x, y).into()));
            if let Some((_, control)) = control.cloned() {
                return self.sidebar_control(outbound, control);
            }
            if let Some((_, index)) = hits.sidebar_rows.iter().find(|(row_y, _)| *row_y == y) {
                self.state.selected = *index;
                self.state.cursor_moved = true;
                self.state.zone = Zone::Sidebar;
                self.open_selected(outbound);
            }
            return;
        }
        let split = hits.splits.iter().find(|border| match border.direction {
            ratatui::layout::Direction::Horizontal => {
                x == border.pos
                    && y >= border.area.y
                    && y < border.area.y.saturating_add(border.area.height)
            }
            ratatui::layout::Direction::Vertical => {
                y == border.pos
                    && x >= border.area.x
                    && x < border.area.x.saturating_add(border.area.width)
            }
        });
        if let Some(border) = split {
            self.drag = Some((self.state.active_tab, border.path.clone()));
            return;
        }
        if self.pending_close.is_some() {
            return;
        }
        if let Some(pane_id) = hits.pane_at(x, y) {
            if let Some(tab) = self.state.tab_mut() {
                tab.layout.focus_pane(pane_id);
            }
            self.state.zone = Zone::Panes;
            let scroll = self
                .state
                .panes
                .get(&pane_id)
                .map_or(0, |pane| pane.scroll_from_bottom);
            self.selection = Some(crate::select::Drag::new(pane_id, x, y, scroll));
            if let Some((row, col)) = self.editor_cell(pane_id, x, y) {
                self.selection = None;
                if let Some(editor) = self.editor_mut() {
                    editor.click(row, col);
                }
                self.editor_drag = true;
                self.dirty = true;
                return;
            }
            if let Some(session) = self.state.focused_session()
                && self
                    .state
                    .sessions
                    .get(&session)
                    .is_some_and(|row| row.status == SessionStatus::DoneUnseen)
            {
                self.mark_seen(outbound, &session);
            }
            self.dirty = true;
        }
    }

    fn sidebar_control(&mut self, outbound: &Outbound, control: Control) {
        match control {
            Control::New(root) => {
                let root = root.unwrap_or_else(|| self.state.root.clone());
                if let Some(pane) = self.state.focused_pane_id() {
                    self.new_session_into(outbound, pane, root);
                }
            }
            Control::Search => self.apply_action(outbound, Action::Navigator),
            Control::Fold(root) => self.state.toggle_fold(&root),
        }
        self.dirty = true;
    }

    /// Incident: a click with the palette open reached the pane under it. A popup takes every
    /// press now: outside it, or on its close mark, the press dismisses it and goes no further.
    fn overlay_press(&mut self, kind: MouseKind, x: u16, y: u16) {
        if !matches!(kind, MouseKind::Down | MouseKind::RightDown) {
            return;
        }
        let at = Position::new(x, y);
        let Some(hits) = &self.hits else { return };
        let inside = hits.overlays.iter().any(|rect| rect.contains(at));
        if !inside || hits.dismiss.is_some_and(|rect| rect.contains(at)) {
            self.state.mode = Mode::Normal;
            self.dirty = true;
        }
    }

    /// Clicks and drags resolve against the last draw's hit table, so the
    /// mouse can never disagree with what was on screen.
    pub fn handle_mouse(&mut self, outbound: &Outbound, kind: MouseKind, x: u16, y: u16) {
        match self.state.mode {
            Mode::Menu(_) => return self.menu_mouse(outbound, kind, x, y),
            Mode::Navigator { .. } => return self.overlay_press(kind, x, y),
            Mode::Normal | Mode::Prefix => {}
        }
        match kind {
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let delta: isize = if kind == MouseKind::ScrollUp { 3 } else { -3 };
                self.scroll_pane_under(x, y, delta);
            }
            MouseKind::Down => self.press(outbound, x, y),
            MouseKind::RightDown => self.right_click(x, y),
            MouseKind::Moved => {
                let before = self.hovered();
                self.pointer = Some((x, y));
                self.dirty |= before != self.hovered();
            }
            MouseKind::Drag => {
                self.pending_close = None;
                if self.editor_drag {
                    let cell = self
                        .state
                        .focused_pane_id()
                        .and_then(|id| self.editor_cell(id, x, y));
                    if let (Some((row, col)), Some(editor)) = (cell, self.editor_mut()) {
                        editor.drag_to(row, col);
                        self.dirty = true;
                    }
                    return;
                }
                let Some((tab_index, path)) = self.drag.clone() else {
                    if let Some(drag) = &mut self.selection
                        && drag.head != (x, y)
                    {
                        drag.head = (x, y);
                        drag.moved = true;
                        self.dirty = true;
                    }
                    return;
                };
                if tab_index != self.state.active_tab {
                    self.drag = None;
                    return;
                }
                let border = self.hits.as_ref().and_then(|hits| {
                    hits.splits
                        .iter()
                        .find(|border| border.path == path)
                        .cloned()
                });
                let Some(border) = border else { return };
                let ratio = match border.direction {
                    ratatui::layout::Direction::Horizontal => {
                        let span = f32::from(border.area.width.max(1));
                        f32::from(x.saturating_sub(border.area.x)) / span
                    }
                    ratatui::layout::Direction::Vertical => {
                        let span = f32::from(border.area.height.max(1));
                        f32::from(y.saturating_sub(border.area.y)) / span
                    }
                };
                if let Some(tab) = self.state.tab_mut() {
                    tab.layout.set_ratio_at(&path, ratio);
                }
                self.dirty = true;
            }
            MouseKind::Up => {
                self.drag = None;
                if let Some(pane) = self
                    .pending_close
                    .take()
                    .filter(|pane| self.closer_at(x, y) == Some(*pane))
                {
                    if let Some(tab) = self.state.tab_mut() {
                        tab.layout.focus_pane(pane);
                    }
                    self.apply_action(outbound, Action::ClosePane);
                    self.dirty = true;
                    return;
                }
                if std::mem::take(&mut self.editor_drag) {
                    let copied = self.editor_mut().and_then(Editor::release);
                    if let Some(text) = copied {
                        self.copy_out(&text);
                    }
                    self.dirty = true;
                    return;
                }
                // The highlight goes once its text is on the clipboard; the flash says so.
                if let Some(drag) = self.selection.take() {
                    self.dirty = true;
                    if drag.moved && !self.selected.is_empty() {
                        let text = std::mem::take(&mut self.selected);
                        self.copy_out(&text);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use yi_tui::colors::{ColorTier, Theme};

    #[test]
    fn a_press_outside_the_palette_closes_it_and_goes_no_further() {
        let mut app = App::new("/repo".to_owned(), Theme::new(ColorTier::TrueColor, true));
        app.hits = Some(crate::render::Hits {
            sidebar_width: 20,
            sidebar_rows: vec![(3, 1)],
            overlays: vec![Rect::new(30, 1, 40, 10)],
            hint_buttons: Vec::new(),
            ..crate::render::Hits::default()
        });
        app.state.mode = Mode::Navigator {
            query: String::new(),
            selected: 0,
        };
        let (events, _received) = crate::client::events();
        let socket = std::env::temp_dir().join("yi-overlay-press-no-daemon.sock");
        let (outbound, _threads) = crate::client::spawn(socket, events);
        app.handle_mouse(&outbound, MouseKind::Down, 5, 3);
        outbound.shutdown();
        assert!(
            matches!(app.state.mode, Mode::Normal),
            "the palette stayed open"
        );
        assert_eq!(
            app.state.selected, 0,
            "the press reached the sidebar row under it"
        );
    }
}
