use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use crate::client::Outbound;
use crate::keys::Action;
use crate::layout::PaneId;
use crate::model::{Editor, Mode, PaneContent, SessionStatus, Zone};

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

/// Columns one sideways wheel notch slides a file.
const SIDEWAYS: isize = 6;

impl App {
    /// The editor under the pointer slides by `delta` columns, until its widest line ends at the
    /// pane's right edge.
    fn scroll_sideways(&mut self, x: u16, y: u16, delta: isize) {
        let Some((pane, rect)) = self.hits.as_ref().and_then(|hits| {
            let pane = hits.pane_at(x, y)?;
            Some((pane, hits.rect_of(pane)?))
        }) else {
            return;
        };
        let Some(crate::model::Pane {
            content: PaneContent::Editor(editor),
            ..
        }) = self.state.panes.get_mut(&pane)
        else {
            return;
        };
        let inner = rect.inner(crate::layout::pane_margin(true, false));
        let room = usize::from(inner.width).saturating_sub(editor.gutter());
        let most = crate::editor_view::widest(editor).saturating_sub(room);
        editor.scroll_left = editor.scroll_left.saturating_add_signed(delta).min(most);
        self.dirty = true;
    }

    /// A key or click while the title covers the screen only ends it: the hit map and the key
    /// handlers under the cover would otherwise act on a screen nobody can see.
    pub(super) fn skip_title(&mut self) -> bool {
        let playing = crate::title_playing(self);
        self.state.title = None;
        self.dirty |= playing;
        playing
    }

    pub(super) fn mouse_event(&mut self, outbound: &Outbound, mouse: MouseEvent) {
        if crate::title_playing(self) {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.skip_title();
            }
            return;
        }
        let sideways = match mouse.kind {
            MouseEventKind::ScrollLeft => Some(-SIDEWAYS),
            MouseEventKind::ScrollRight => Some(SIDEWAYS),
            MouseEventKind::ScrollUp if mouse.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(-SIDEWAYS)
            }
            MouseEventKind::ScrollDown if mouse.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(SIDEWAYS)
            }
            _ => None,
        };
        if let Some(delta) = sideways {
            self.scroll_sideways(mouse.column, mouse.row, delta);
            return;
        }
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
            self.dirty = pane.scroll_by(step);
        }
    }

    fn scroll_pane_under(&mut self, x: u16, y: u16, delta: isize) {
        if self
            .hits
            .as_ref()
            .is_some_and(|hits| x < hits.sidebar_width)
        {
            return self.scroll_sidebar(delta);
        }
        let Some(pane_id) = self.hits.as_ref().and_then(|hits| hits.pane_at(x, y)) else {
            return;
        };
        if self.settings_wheel(pane_id, delta) {
            return;
        }
        if let Some(pane) = self.state.panes.get_mut(&pane_id) {
            if let PaneContent::Editor(editor) = &mut pane.content {
                editor.scroll_top = if delta >= 0 {
                    editor.scroll_top.saturating_sub(delta.unsigned_abs())
                } else {
                    editor.scroll_top.saturating_add(delta.unsigned_abs())
                };
                self.dirty = true;
                return;
            }
            pane.scroll_by(delta);
            self.dirty = true;
        }
    }

    fn editor_cell(&self, pane_id: PaneId, x: u16, y: u16) -> Option<(usize, usize)> {
        let rect = self.hits.as_ref()?.rect_of(pane_id)?;
        let inner = rect.inner(crate::layout::pane_margin(true, false));
        let PaneContent::Editor(editor) = &self.state.panes.get(&pane_id)?.content else {
            return None;
        };
        let row = usize::from(
            y.checked_sub(inner.y)?
                .checked_sub(crate::editor_view::TAB_ROWS)?,
        )
        .saturating_add(editor.scroll_top);
        let col = usize::from(x.checked_sub(inner.x)?)
            .checked_sub(editor.gutter())?
            .saturating_add(editor.scroll_left);
        let line = editor.text.lines().get(row).map_or("", String::as_str);
        Some((row, crate::editor_view::char_at(line, col)))
    }

    pub(super) fn closer_at(&self, x: u16, y: u16) -> Option<PaneId> {
        self.hits.as_ref()?.closer_at(x, y).map(|(pane, _)| pane)
    }

    pub(super) fn press(&mut self, outbound: &Outbound, x: u16, y: u16) {
        // Any new click drops the last drag's highlight.
        if self.selection.take().is_some() {
            self.dirty = true;
        }
        let Some(hits) = &self.hits else { return };
        // Incident: a lower pane's mark sits on the split it can be dragged by, and a press meant
        // as a resize closed the pane; a mark closes on a release over it that did not drag.
        self.pending_close = self.closer_at(x, y);
        match hits.hit_at(x, y) {
            Some((_, crate::render::Hit::Tab(index))) => {
                self.state.select_tab(index);
                self.dirty = true;
                return;
            }
            Some((_, crate::render::Hit::Settings(pane, target))) => {
                return self.settings_press(outbound, pane, target);
            }
            Some((_, crate::render::Hit::Button(action))) => {
                return self.apply_action(outbound, action);
            }
            Some((_, crate::render::Hit::Gear)) => {
                let Some(pane) = self.open_settings_pane() else {
                    return self.apply_action(outbound, Action::Settings);
                };
                if let Some(tab) = self.state.tab_mut() {
                    tab.layout.focus_pane(pane);
                }
                self.dirty = true;
                return self.apply_action(outbound, Action::ClosePane);
            }
            Some((_, crate::render::Hit::New(root))) => {
                if let Some(root) = root {
                    if let Some(pane) = self.state.focused_pane_id() {
                        self.new_session_into(outbound, pane, root);
                    }
                } else {
                    self.new_session(outbound, (x, y));
                }
                self.dirty = true;
                return;
            }
            Some((_, crate::render::Hit::EditorTab(pane, index))) => {
                return self.show_tab(pane, index);
            }
            Some((_, crate::render::Hit::EditorClose(pane, index))) => {
                return self.close_tab(outbound, pane, index);
            }
            Some((_, crate::render::Hit::Review(pane, pick))) => {
                return self.review_pick(pane, pick);
            }
            Some((_, crate::render::Hit::Search)) => return self.begin_search(),
            Some((_, crate::render::Hit::FilesRefresh)) => {
                if let Some(files) = &mut self.state.files {
                    files.refresh();
                }
                self.dirty = true;
                return;
            }
            Some((_, crate::render::Hit::FilesCollapse)) => {
                if let Some(files) = &mut self.state.files {
                    files.collapse_all();
                }
                self.dirty = true;
                return;
            }
            Some((_, crate::render::Hit::FilesClose)) => {
                self.state.files = None;
                self.dirty = true;
                return;
            }
            Some((_, crate::render::Hit::Run(pane, cell, ordinal))) => {
                self.pending_close = None;
                let looking = self.state.focused_pane_id();
                let code = match self.state.panes.get(&pane).map(|pane| &pane.content) {
                    Some(crate::model::PaneContent::Session {
                        chat: Some(chat), ..
                    }) => chat.app.shell_block(cell, ordinal),
                    _ => None,
                };
                let Some(code) = code else { return };
                return self.run_in_terminal(looking, pane, &code);
            }
            Some((_, crate::render::Hit::Fold(root))) => {
                self.state.fold_from_header(&root);
                self.state.zone = Zone::Sidebar;
                self.dirty = true;
                return;
            }
            Some((_, crate::render::Hit::Session(index))) => {
                self.state.header = None;
                self.state.selected = index;
                self.state.cursor_moved = true;
                self.state.zone = Zone::Sidebar;
                self.open_selected(outbound);
                return;
            }
            // Incident: the lower pane's title sits on the split row, and a press there
            // meant as a resize focused the pane instead; the split is tried first.
            Some((_, crate::render::Hit::Title(pane))) if hits.split_at(x, y).is_none() => {
                self.focus_pane(pane);
                self.mark_focused_seen(outbound);
                self.dirty = true;
                return;
            }
            Some((_, crate::render::Hit::Title(_))) | None => {}
        }
        if x < hits.sidebar_width {
            return;
        }
        if let Some(border) = hits.split_at(x, y) {
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
            self.state.focus_panes();
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
            self.mark_focused_seen(outbound);
            self.dirty = true;
        }
    }

    pub(super) fn mark_focused_seen(&mut self, outbound: &Outbound) {
        if let Some(session) = self.state.focused_session()
            && self
                .state
                .sessions
                .get(&session)
                .is_some_and(|row| row.status == SessionStatus::DoneUnseen)
        {
            self.mark_seen(outbound, &session);
        }
    }

    /// Incident: a click with the palette open reached the pane under it. A popup takes every
    /// press: outside it, or on its close mark, the press dismisses it and goes no further.
    pub(super) fn overlay_press(&mut self, kind: MouseKind, x: u16, y: u16) -> bool {
        let press = matches!(kind, MouseKind::Down | MouseKind::RightDown);
        let at = Position::new(x, y);
        let Some(hits) = &self.hits else { return press };
        if hits.overlays.iter().any(|rect| rect.contains(at))
            && !hits.dismiss.is_some_and(|rect| rect.contains(at))
        {
            return false;
        }
        if press {
            self.state.mode = Mode::Normal;
            self.dirty = true;
        }
        true
    }

    /// Incident: a pointer shape asked for by a mouse move waited in the queue for the next frame,
    /// and a move that changes nothing on screen draws none, so it arrived about a second late.
    pub fn flush_osc(&mut self, out: &mut impl std::io::Write) {
        for sequence in self.osc_out.drain(..) {
            let _ = out.write_all(sequence.as_bytes());
        }
        let _ = out.flush();
    }

    /// The shape the pointer takes over what is under it, as a CSS cursor name for OSC 22.
    fn pointer_shape(&self, x: u16, y: u16) -> &'static str {
        let Some(hits) = &self.hits else {
            return "default";
        };
        let at = Position::new(x, y);
        if self.state.snap.as_ref().is_some_and(|drag| drag.dragging()) {
            return "grabbing";
        }
        if hits.titles.iter().any(|(_, rect)| rect.contains(at)) {
            return "grab";
        }
        if let Some(border) = hits.split_at(x, y) {
            return match border.direction {
                ratatui::layout::Direction::Horizontal => "ew-resize",
                ratatui::layout::Direction::Vertical => "ns-resize",
            };
        }
        let on_tree = hits.files.is_some_and(|side| {
            side.contains(at)
                && (y == side.y
                    || self
                        .state
                        .files
                        .as_ref()
                        .is_some_and(|files| files.pointed(side, x, y).is_some()))
        });
        if on_tree || hits.hover_at(x, y).is_some() || self.closer_at(x, y).is_some() {
            return "pointer";
        }
        let on_editor = hits
            .pane_at(x, y)
            .and_then(|id| self.state.panes.get(&id))
            .is_some_and(|pane| matches!(pane.content, PaneContent::Editor(_)));
        if on_editor { "text" } else { "default" }
    }

    /// Clicks and drags resolve against the last draw's hit table, so the
    /// mouse can never disagree with what was on screen.
    pub fn handle_mouse(&mut self, outbound: &Outbound, kind: MouseKind, x: u16, y: u16) {
        self.dispatch_mouse(outbound, kind, x, y);
        let shape = self.pointer_shape(x, y);
        if shape != self.state.pointer {
            self.state.pointer = shape;
            self.osc_out.push(format!("\x1b]22;{shape}\x1b\\"));
        }
    }

    fn dispatch_mouse(&mut self, outbound: &Outbound, kind: MouseKind, x: u16, y: u16) {
        match self.state.mode {
            Mode::Menu(_) => {
                self.release_browser_presses();
                return self.menu_mouse(outbound, kind, x, y);
            }
            Mode::Navigator { .. } => {
                self.release_browser_presses();
                self.overlay_press(kind, x, y);
                return;
            }
            Mode::Normal | Mode::Prefix => {}
        }
        if self.files_mouse(kind, x, y) {
            self.release_browser_presses();
            return;
        }
        if self.snap_mouse(outbound, kind, x, y) {
            return;
        }
        if self.browser_mouse(kind, x, y) {
            return;
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

    fn app_with_a_browser_page() -> Result<(App, PaneId), Box<dyn std::error::Error>> {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        let pane = app.state.focused_pane_id().ok_or("a pane")?;
        let mut browser = crate::browser::BrowserPane::new(Some("http://localhost:1/".to_owned()));
        browser.view(Rect::new(0, 0, 40, 20), true, &theme);
        if let Some(slot) = app.state.panes.get_mut(&pane) {
            slot.content = PaneContent::Browser(Box::new(browser));
        }
        app.hits = Some(crate::render::Hits {
            panes: vec![(pane, Rect::new(0, 0, 40, 20))],
            ..crate::render::Hits::default()
        });
        Ok((app, pane))
    }

    fn mouse(app: &mut App, kind: MouseKind, x: u16, y: u16) {
        let (events, _received) = crate::client::events();
        let socket = std::env::temp_dir().join("yi-browser-mouse-no-daemon.sock");
        let (outbound, _threads) = crate::client::spawn(socket, events);
        app.handle_mouse(&outbound, kind, x, y);
        outbound.shutdown();
    }

    /// Incident: the page took every event over it, so a split drag or a chat selection that
    /// moved onto a browser pane never moved and never finished.
    #[test]
    fn a_drag_that_began_outside_the_page_goes_on_over_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let (mut app, pane) = app_with_a_browser_page()?;
        app.selection = Some(crate::select::Drag::new(pane, 1, 1, 0));
        mouse(&mut app, MouseKind::Drag, 10, 10);
        assert_eq!(app.selection.as_ref().map(|drag| drag.head), Some((10, 10)));
        app.drag = Some((app.state.active_tab, Vec::new()));
        mouse(&mut app, MouseKind::Up, 10, 10);
        assert!(app.drag.is_none(), "the release never reached the console");
        assert!(app.selection.is_none(), "the selection never finished");
        Ok(())
    }

    #[test]
    fn a_press_on_the_page_keeps_its_drag_and_release_from_the_console()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut app, pane) = app_with_a_browser_page()?;
        mouse(&mut app, MouseKind::Down, 10, 10);
        app.selection = Some(crate::select::Drag::new(pane, 1, 1, 0));
        mouse(&mut app, MouseKind::Drag, 30, 15);
        mouse(&mut app, MouseKind::Up, 30, 15);
        assert_eq!(
            app.selection.as_ref().map(|drag| drag.head),
            Some((1, 1)),
            "the console moved a selection the page's drag owned"
        );
        mouse(&mut app, MouseKind::Drag, 12, 12);
        assert_eq!(app.selection.as_ref().map(|drag| drag.head), Some((12, 12)));
        Ok(())
    }
}
