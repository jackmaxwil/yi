//! Panels placed by snap zone. ⌘⌥⇧ arrows and ⌘⌥ d/f/g move the focused panel; a drag from "new
//! session", a session row or a pane title lights the zone under the pointer and drops there.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use yi_tui::colors::Theme;

use crate::client::Outbound;
use crate::hit::Hit;
use crate::keys::{Action, Shift};
use crate::layout::{PaneId, Snap};
use crate::model::Pane;

use super::{App, MouseKind};

/// A press this far from where it began is a drag, not a click.
const SLOP: u16 = 2;

pub(crate) enum Source {
    New(Option<String>),
    Session(usize),
    Pane(PaneId),
    /// A file tab of an editor panel, by its place in the strip.
    Tab(PaneId, usize),
    /// A project's header in the sidebar: dragging it up or down reorders the projects.
    Group(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Drop {
    Edge(Snap),
    Onto(PaneId),
}

impl Drag {
    pub(crate) fn dragging(&self) -> bool {
        self.moved
    }
}

pub(crate) struct Drag {
    source: Source,
    from: (u16, u16),
    /// The press waits for the release to run as a click; a pane title acts on the press itself.
    replay: bool,
    moved: bool,
    over: Option<Drop>,
}

/// Every pane rectangle together: the area a snap is measured against.
fn area_of(app: &App) -> Option<Rect> {
    let hits = app.hits.as_ref()?;
    hits.panes
        .iter()
        .map(|(_, rect)| *rect)
        .reduce(|all, rect| all.union(rect))
}

/// The bottom eighth takes a third by how far along it the pointer is (as Rectangle's bottom edge
/// does); above it the outer quarter on a side takes that half, and the middle lands on the pane.
fn zone_at(app: &App, x: u16, y: u16) -> Option<Drop> {
    let area = area_of(app)?;
    let (dx, dy) = (
        u32::from(x.checked_sub(area.x)?),
        u32::from(y.checked_sub(area.y)?),
    );
    let (w, h) = (u32::from(area.width), u32::from(area.height));
    if dx >= w || dy >= h {
        return None;
    }
    let edge = if dy * 8 >= h * 7 {
        match dx * 3 / w {
            0 => Snap::LeftThird,
            1 => Snap::MiddleThird,
            _ => Snap::RightThird,
        }
    } else if dx * 4 < w {
        Snap::Left
    } else if dx * 4 >= w * 3 {
        Snap::Right
    } else if dy * 4 < h {
        Snap::Top
    } else if dy * 4 >= h * 3 {
        Snap::Bottom
    } else {
        return app.hits.as_ref()?.pane_at(x, y).map(Drop::Onto);
    };
    Some(Drop::Edge(edge))
}

fn rect_of(app: &App, drop: Drop) -> Option<Rect> {
    let area = area_of(app)?;
    let (half_w, half_h) = (area.width / 2, area.height / 2);
    Some(match drop {
        Drop::Edge(Snap::Left) => Rect {
            width: half_w,
            ..area
        },
        Drop::Edge(Snap::Right) => Rect {
            x: area.right().saturating_sub(half_w),
            width: half_w,
            ..area
        },
        Drop::Edge(Snap::Top) => Rect {
            height: half_h,
            ..area
        },
        Drop::Edge(Snap::Bottom) => Rect {
            y: area.bottom().saturating_sub(half_h),
            height: half_h,
            ..area
        },
        Drop::Edge(Snap::LeftThird) => Rect {
            width: area.width / 3,
            ..area
        },
        Drop::Edge(Snap::MiddleThird) => Rect {
            x: area.x.saturating_add(area.width / 3),
            width: area.width / 3,
            ..area
        },
        Drop::Edge(Snap::RightThird) => Rect {
            x: area.right().saturating_sub(area.width / 3),
            width: area.width / 3,
            ..area
        },
        Drop::Onto(pane) => app.hits.as_ref()?.rect_of(pane)?,
    })
}

fn label(drop: Drop) -> &'static str {
    match drop {
        Drop::Edge(Snap::Left) => " left half ",
        Drop::Edge(Snap::Right) => " right half ",
        Drop::Edge(Snap::Top) => " top half ",
        Drop::Edge(Snap::Bottom) => " bottom half ",
        Drop::Edge(Snap::LeftThird) => " left third ",
        Drop::Edge(Snap::MiddleThird) => " middle third ",
        Drop::Edge(Snap::RightThird) => " right third ",
        Drop::Onto(_) => " fill this panel ",
    }
}

/// Zone names in dim text while a drag is up, so the thirds and halves can be found before the
/// pointer is over one; those the lit zone covers are left out, as dim text cannot be read on it.
fn guides(area: Rect, lit: Option<Rect>, buffer: &mut Buffer, theme: &Theme) {
    let style = Style::default().fg(theme.dim);
    let width = |text: &str| u16::try_from(text.chars().count()).unwrap_or(0);
    let centred = |text: &str| {
        area.x
            .saturating_add(area.width.saturating_sub(width(text)) / 2)
    };
    let (mid, top) = (
        area.y.saturating_add(area.height / 2),
        area.y.saturating_add(1),
    );
    let low = area.y.saturating_add(area.height.saturating_mul(3) / 4);
    let strip = area.bottom().saturating_sub(1);
    let third = area.width / 3;
    let third_at = |index: u16, text: &str| {
        let start = area.x.saturating_add(third.saturating_mul(index));
        start.saturating_add(third.saturating_sub(width(text)) / 2)
    };
    let zones = [
        (area.x.saturating_add(1), mid, "◂ left half"),
        (
            area.right()
                .saturating_sub(width("right half ▸").saturating_add(1)),
            mid,
            "right half ▸",
        ),
        (centred("▴ top half"), top, "▴ top half"),
        (centred("▾ bottom half"), low, "▾ bottom half"),
        (third_at(0, "left third"), strip, "left third"),
        (third_at(1, "middle third"), strip, "middle third"),
        (third_at(2, "right third"), strip, "right third"),
    ];
    for (x, y, text) in zones {
        let end = x.saturating_add(width(text));
        let covered = lit.is_some_and(|rect| {
            rect.y <= y && y < rect.bottom() && x < rect.right() && end > rect.x
        });
        if !covered {
            buffer.set_string(x, y, text, style);
        }
    }
}

/// The lit rectangle of the zone the pointer is over, drawn over the panes.
pub(crate) fn overlay(app: &App, buffer: &mut Buffer, theme: &Theme) {
    let Some(drag) = app.state.snap.as_ref().filter(|drag| drag.moved) else {
        return;
    };
    if let Some(area) = area_of(app) {
        let lit = drag.over.and_then(|drop| rect_of(app, drop));
        guides(area, lit, buffer, theme);
    }
    let Some(rect) = drag
        .over
        .and_then(|drop| rect_of(app, drop).zip(Some(drop_label(drag))))
    else {
        return;
    };
    let (rect, text) = rect;
    for at in rect.intersection(buffer.area).positions() {
        if let Some(cell) = buffer.cell_mut(at) {
            cell.set_bg(theme.selection_bg());
        }
    }
    let x = rect
        .x
        .saturating_add(rect.width.saturating_sub(text.chars().count() as u16) / 2);
    let y = rect.y.saturating_add(rect.height / 2);
    let style = Style::default()
        .fg(theme.text)
        .bg(theme.selection_bg())
        .add_modifier(Modifier::BOLD)
        .remove_modifier(Modifier::DIM);
    buffer.set_string(x, y, text, style);
}

fn drop_label(drag: &Drag) -> &'static str {
    drag.over.map_or("", label)
}

impl App {
    /// The focused panel moves to `snap`; a lone panel has nowhere to go.
    pub(super) fn snap_focused(&mut self, snap: Snap) {
        let moved = self.state.focused_pane_id().is_some_and(|id| {
            self.state.tab_mut().is_some_and(|tab| {
                tab.zoomed = false;
                tab.layout.snap(id, snap)
            })
        });
        if !moved {
            self.note("only one panel is open, so there is nothing to move");
        }
    }

    /// A pane's title and the rest of its top border move it, even where that border is the split
    /// row: a press there is a move, so a resize is only ever grabbed on the line between panels.
    fn snap_source(&self, x: u16, y: u16) -> Option<Source> {
        let hits = self.hits.as_ref()?;
        let many = self
            .state
            .tab()
            .is_some_and(|tab| tab.layout.pane_count() > 1);
        let top = || {
            let pane = hits.pane_at(x, y)?;
            let rect = hits.rect_of(pane)?;
            (y == rect.y && hits.split_at(x, y).is_none()).then_some(pane)
        };
        match hits.hit_at(x, y).map(|(_, hit)| hit) {
            Some(Hit::New(root)) => Some(Source::New(root)),
            Some(Hit::Session(index)) => Some(Source::Session(index)),
            Some(Hit::Fold(root)) => Some(Source::Group(root)),
            Some(Hit::EditorTab(pane, index)) => Some(Source::Tab(pane, index)),
            _ if self.closer_at(x, y).is_some() || !many => None,
            Some(Hit::Title(pane)) => Some(Source::Pane(pane)),
            None => top().map(Source::Pane),
            Some(_) => None,
        }
    }

    /// True when the event belongs to a drag: the press that may start one waits for the release,
    /// which replays it as the click it was if the pointer never left.
    pub(super) fn snap_mouse(
        &mut self,
        outbound: &Outbound,
        kind: MouseKind,
        x: u16,
        y: u16,
    ) -> bool {
        match kind {
            MouseKind::Down => {
                let Some(source) = self.snap_source(x, y) else {
                    return false;
                };
                let replay = !matches!(source, Source::Pane(_));
                if let Source::Pane(pane) = source {
                    self.focus_pane(pane);
                    self.mark_focused_seen(outbound);
                }
                self.state.snap = Some(Drag {
                    source,
                    from: (x, y),
                    replay,
                    moved: false,
                    over: None,
                });
                true
            }
            MouseKind::Drag => {
                if let Some(root) = self
                    .state
                    .snap
                    .as_ref()
                    .and_then(|drag| match &drag.source {
                        Source::Group(root) => Some(root.clone()),
                        _ => None,
                    })
                {
                    self.drag_group(&root, (x, y));
                    return true;
                }
                let over = zone_at(self, x, y);
                let is_editor = matches!(
                    over,
                    Some(Drop::Onto(pane))
                        if matches!(
                            self.state.panes.get(&pane).map(|pane| &pane.content),
                            Some(crate::model::PaneContent::Editor(_))
                        )
                );
                let Some(drag) = &mut self.state.snap else {
                    return false;
                };
                let far = x.abs_diff(drag.from.0) >= SLOP || y.abs_diff(drag.from.1) >= SLOP;
                drag.moved |= far;
                if drag.moved {
                    let source = &drag.source;
                    drag.over = over.filter(|drop| match (source, drop) {
                        (Source::Pane(_), Drop::Onto(_)) => false,
                        (Source::Tab(from, _), Drop::Onto(to)) => to != from && is_editor,
                        _ => true,
                    });
                }
                self.dirty = true;
                true
            }
            MouseKind::Up => {
                let Some(drag) = self.state.snap.take() else {
                    return false;
                };
                self.dirty = true;
                if !drag.moved {
                    if drag.replay {
                        self.press(outbound, drag.from.0, drag.from.1);
                    }
                    return false;
                }
                if let Some(drop) = drag.over {
                    self.snap_drop(outbound, drag.source, drop);
                }
                true
            }
            _ => false,
        }
    }

    /// The project dragged by its header takes the place of the header the pointer crosses.
    fn drag_group(&mut self, root: &str, (x, y): (u16, u16)) {
        let Some(drag) = &mut self.state.snap else {
            return;
        };
        drag.moved |= x.abs_diff(drag.from.0) >= SLOP || y.abs_diff(drag.from.1) >= SLOP;
        if !drag.moved {
            return;
        }
        let over = self.hits.as_ref().and_then(|hits| {
            hits.controls.iter().find_map(|(rect, hit)| match hit {
                Hit::Fold(other) if rect.y == y && other != root => Some(other.clone()),
                _ => None,
            })
        });
        if let Some(other) = over {
            crate::sidebar::move_group(&mut self.state, root, &other);
        }
        self.dirty = true;
    }

    /// ⌥⇧↑↓: the project under the cursor swaps with its neighbour.
    pub(crate) fn shift_project(&mut self, step: isize) {
        let roots: Vec<String> = self
            .state
            .groups()
            .into_iter()
            .filter_map(|group| group.root)
            .collect();
        let here = self.state.header.clone().or_else(|| {
            self.state
                .selected_id()
                .and_then(|id| self.state.sessions.get(id))
                .map(|row| row.root.clone())
        });
        let neighbour = here.as_ref().and_then(|root| {
            let at = roots.iter().position(|known| known == root)?;
            roots.get(at.checked_add_signed(step)?)
        });
        if let (Some(root), Some(other)) = (here, neighbour) {
            crate::sidebar::move_group(&mut self.state, &root, other);
            self.dirty = true;
        }
    }

    /// A new empty panel on the screen's `snap` edge, focused.
    fn new_pane_at(&mut self, snap: Snap) -> Option<PaneId> {
        let ids = &mut self.state.pane_ids;
        let tab = self.state.tabs.get_mut(self.state.active_tab)?;
        tab.zoomed = false;
        let pane = tab.layout.snap_new(ids, snap);
        self.state.panes.insert(pane, Pane::empty_session());
        Some(pane)
    }

    /// The file at tab `index`, out of its panel; a lone file takes its panel with it.
    fn extract_tab(&mut self, from: PaneId, index: usize) -> Option<crate::model::Editor> {
        let many = self.tabs_of(from).map(|tabs| tabs.count() > 1)?;
        if many {
            return self.tabs_of(from)?.take(index);
        }
        self.focus_pane(from);
        match self.state.close_focused_pane()?.content {
            crate::model::PaneContent::Editor(tabs) => Some((*tabs).into_shown()),
            _ => None,
        }
    }

    fn drop_tab(&mut self, from: PaneId, index: usize, drop: Drop) {
        let Some(file) = self.extract_tab(from, index) else {
            return;
        };
        let target = match drop {
            Drop::Onto(pane) => pane,
            Drop::Edge(snap) => {
                let Some(pane) = self.new_pane_at(snap) else {
                    return;
                };
                if let Some(slot) = self.state.panes.get_mut(&pane) {
                    slot.content = crate::model::PaneContent::Editor(Box::new(
                        crate::editor_view::Tabs::new(file),
                    ));
                }
                return self.state.focus_panes();
            }
        };
        if let Some(tabs) = self.tabs_of(target) {
            tabs.open(file);
        }
        self.focus_pane(target);
        self.state.focus_panes();
    }

    pub(super) fn layout_action(&mut self, action: Action) {
        match action {
            Action::Snap(snap) => self.snap_focused(snap),
            Action::Shift(Shift::FileBack) => self.file_tab(-1),
            Action::Shift(Shift::FileForward) => self.file_tab(1),
            Action::Shift(Shift::ProjectUp) => self.shift_project(-1),
            Action::Shift(Shift::ProjectDown) => self.shift_project(1),
            _ => {}
        }
    }

    fn snap_drop(&mut self, outbound: &Outbound, source: Source, drop: Drop) {
        let whole = match &source {
            Source::Pane(id) => Some(*id),
            Source::Tab(id, _) if self.tabs_of(*id).is_some_and(|tabs| tabs.count() <= 1) => {
                Some(*id)
            }
            _ => None,
        };
        if let (Some(id), Drop::Edge(snap)) = (whole, drop) {
            if let Some(tab) = self.state.tab_mut() {
                tab.zoomed = false;
                tab.layout.snap(id, snap);
            }
            self.state.focus_panes();
            return;
        }
        if let Source::Tab(from, index) = source {
            return self.drop_tab(from, index, drop);
        }
        let pane = match drop {
            Drop::Edge(snap) => {
                let Some(pane) = self.new_pane_at(snap) else {
                    return;
                };
                pane
            }
            Drop::Onto(pane) => {
                if let Some(tab) = self.state.tab_mut() {
                    tab.layout.focus_pane(pane);
                }
                pane
            }
        };
        match source {
            Source::New(Some(root)) => {
                self.new_session_into(outbound, pane, root);
            }
            Source::New(None) => {
                if let Some(tab) = self.state.tab_mut() {
                    tab.layout.focus_pane(pane);
                }
                self.new_session(outbound, (0, 0));
            }
            Source::Session(index) => {
                self.state.header = None;
                self.state.selected = index;
                self.state.cursor_moved = true;
                self.open_selected(outbound);
            }
            Source::Pane(_) | Source::Tab(..) | Source::Group(_) => {}
        }
        self.state.focus_panes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SessionId, SessionRow, SessionStatus};

    fn two_projects() -> Result<App, Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        for (id, root) in [("s-a", "/a"), ("s-b", "/b")] {
            app.state.upsert_row(SessionRow {
                id: SessionId(id.to_owned()),
                root: root.to_owned(),
                status: SessionStatus::Idle,
                attached: false,
                name: None,
                created_ms: 0,
                last_ms: 0,
                pinned: None,
            });
        }
        let hits = crate::render::Hits {
            controls: vec![(
                Rect::new(0, 9, 8, 1),
                crate::hit::Hit::Fold("/b".to_owned()),
            )],
            ..Default::default()
        };
        app.hits = Some(hits);
        Ok(app)
    }

    /// Dies with a header that wobbles one cell reordering the projects: a press that never left
    /// the slop is the click its release replays, which folds the header.
    #[test]
    fn a_header_that_wobbles_under_the_slop_moves_no_project()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut app = two_projects()?;
        app.state.snap = Some(Drag {
            source: Source::Group("/a".to_owned()),
            from: (0, 9),
            replay: false,
            moved: false,
            over: None,
        });
        let before = app.state.side.order.clone();
        app.drag_group("/a", (1, 9));
        assert_eq!(
            app.state.side.order, before,
            "a press that wobbled reordered the projects"
        );
        app.drag_group("/a", (3, 9));
        let place = |root: &str| app.state.side.order.iter().position(|known| known == root);
        assert!(
            matches!((place("/b"), place("/a")), (Some(b), Some(a)) if b < a),
            "a drag past the slop did not move the project: {:?}",
            app.state.side.order
        );
        Ok(())
    }
}
