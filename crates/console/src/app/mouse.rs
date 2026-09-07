use crate::client::Outbound;
use crate::layout::PaneId;
use crate::model::{Editor, PaneContent, SessionStatus, Zone};

use super::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Up,
    Drag,
    ScrollUp,
    ScrollDown,
}

impl App {
    /// A drag copies without the user reaching for a key.
    fn copy_out(&mut self, text: &str) {
        self.osc_out.push(crate::select::osc52(text));
    }

    fn scroll_pane_under(&mut self, x: u16, y: u16, delta: isize) {
        let target = self.hits.as_ref().and_then(|hits| {
            hits.panes
                .iter()
                .find(|(_, rect)| rect.contains(ratatui::layout::Position::new(x, y)))
                .map(|(id, _)| *id)
        });
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

    fn press(&mut self, outbound: &Outbound, x: u16, y: u16) {
        // Any new click drops the last drag's highlight.
        if self.selection.take().is_some() {
            self.dirty = true;
        }
        let Some(hits) = &self.hits else { return };
        if let Some((_, index)) = hits
            .tabs
            .iter()
            .find(|(rect, _)| rect.contains(ratatui::layout::Position::new(x, y)))
        {
            let index = *index;
            self.state.select_tab(index);
            self.dirty = true;
            return;
        }
        if x < hits.sidebar_width {
            if let Some((_, index)) = hits.root_rows.iter().find(|(row_y, _)| *row_y == y) {
                let root = self.state.roots().get(*index).cloned();
                let next = root.filter(|r| self.state.root_filter.as_ref() != Some(r));
                self.state.set_root_filter(next);
                self.state.zone = Zone::Sidebar;
                self.dirty = true;
                return;
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
        if let Some((pane_id, _)) = hits
            .panes
            .iter()
            .find(|(_, rect)| rect.contains(ratatui::layout::Position::new(x, y)))
        {
            let pane_id = *pane_id;
            if let Some(tab) = self.state.tab_mut() {
                tab.layout.focus_pane(pane_id);
            }
            self.state.zone = Zone::Panes;
            self.selection = Some(crate::select::Selection::new(x, y));
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

    /// Clicks and drags resolve against the last draw's hit table, so the
    /// mouse can never disagree with what was on screen.
    pub fn handle_mouse(&mut self, outbound: &Outbound, kind: MouseKind, x: u16, y: u16) {
        match kind {
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let delta: isize = if kind == MouseKind::ScrollUp { 3 } else { -3 };
                self.scroll_pane_under(x, y, delta);
            }
            MouseKind::Down => self.press(outbound, x, y),
            MouseKind::Drag => {
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
                    if let Some(selection) = &mut self.selection {
                        selection.head = (x, y);
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
                if std::mem::take(&mut self.editor_drag) {
                    let copied = self.editor_mut().and_then(Editor::release);
                    if let Some(text) = copied {
                        self.copy_out(&text);
                    }
                    self.dirty = true;
                    return;
                }
                if self.selection.is_some_and(|drag| drag.anchor == drag.head) {
                    self.selection = None;
                    self.dirty = true;
                    return;
                }
                if self.selection.is_some() && !self.selected.is_empty() {
                    let text = self.selected.clone();
                    self.copy_out(&text);
                }
            }
        }
    }
}
