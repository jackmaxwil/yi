use crate::client::Outbound;
use crate::model::{SessionStatus, Zone};

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
    /// Clicks and drags resolve against the last draw's hit table, so the
    /// mouse can never disagree with what was on screen.
    pub fn handle_mouse(&mut self, outbound: &Outbound, kind: MouseKind, x: u16, y: u16) {
        match kind {
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let delta: isize = if kind == MouseKind::ScrollUp { 3 } else { -3 };
                let target = self.hits.as_ref().and_then(|hits| {
                    hits.panes
                        .iter()
                        .find(|(_, rect)| rect.contains(ratatui::layout::Position::new(x, y)))
                        .map(|(id, _)| *id)
                });
                if let Some(pane_id) = target
                    && let Some(pane) = self.state.panes.get_mut(&pane_id)
                {
                    pane.scroll_from_bottom = if delta >= 0 {
                        pane.scroll_from_bottom.saturating_add(delta.unsigned_abs())
                    } else {
                        pane.scroll_from_bottom.saturating_sub(delta.unsigned_abs())
                    };
                    self.dirty = true;
                }
            }
            MouseKind::Down => {
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
                    if let Some((_, index)) =
                        hits.sidebar_rows.iter().find(|(row_y, _)| *row_y == y)
                    {
                        self.state.selected = *index;
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
            MouseKind::Drag => {
                let Some((tab_index, path)) = self.drag.clone() else {
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
            }
        }
    }
}
