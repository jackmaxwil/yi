// Derived from `ratatui::Terminal` (MIT, Florian Dehau / The Ratatui Developers)
// with codex's mutable-viewport model (`codex-rs/tui/src/custom_terminal.rs`,
// `tui.rs::draw`, `tui/scrollback.rs::grow_viewport`).
//
// Incident: ratatui's `Viewport::Inline(h)` fixes the height at construction and
// `resize` recomputes from that stored height, so a live region that grows or
// shrinks cannot follow it; rebuilding the terminal instead appends blank lines
// into scrollback on every rebuild. Codex forked `Terminal` for exactly this.
// Kept from codex: the app owns the viewport rect, `set_viewport_area`, growth
// by scroll region, clear-on-change. Dropped: its buffer differ, hyperlink
// coalescing, cursor styles, alt-screen and suspend paths, and the
// per-terminal scrollback strategies — ratatui's `Buffer::diff` plus
// `Backend::draw` cover the first two and the rest are product surface.
use std::io;

use ratatui::backend::{Backend, ClearType};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::widgets::Widget;

pub struct Frame<'a> {
    viewport_area: Rect,
    buffer: &'a mut Buffer,
}

impl Frame<'_> {
    pub const fn area(&self) -> Rect {
        self.viewport_area
    }

    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }
}

pub struct Terminal<B: Backend> {
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    viewport_area: Rect,
    screen_size: Size,
    last_cursor: Position,
}

impl<B: Backend> Terminal<B> {
    /// Anchors an inline viewport of `height` rows at the cursor, scrolling the
    /// screen up to make room exactly as ratatui's `compute_inline_size` does.
    pub fn new(mut backend: B, height: u16) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let cursor = backend.get_cursor_position().unwrap_or(Position::ORIGIN);
        let height = height.min(screen_size.height);
        let mut row = cursor.y;
        let lines_after_cursor = height.saturating_sub(1);
        backend.append_lines(lines_after_cursor)?;
        let available = screen_size.height.saturating_sub(row).saturating_sub(1);
        row = row.saturating_sub(lines_after_cursor.saturating_sub(available));
        let area = Rect {
            x: 0,
            y: row,
            width: screen_size.width,
            height,
        };
        Ok(Self {
            backend,
            buffers: [Buffer::empty(area), Buffer::empty(area)],
            current: 0,
            viewport_area: area,
            screen_size,
            last_cursor: Position { x: 0, y: row },
        })
    }

    pub const fn viewport_area(&self) -> Rect {
        self.viewport_area
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    pub fn set_viewport_area(&mut self, area: Rect) {
        self.current_buffer_mut().resize(area);
        self.previous_buffer_mut().resize(area);
        self.viewport_area = area;
    }

    /// U2 + resize: direct port of codex `tui.rs::update_inline_viewport_for_resize_reflow`.
    /// The live region asks for the height it needs; growth scrolls the rows
    /// above the viewport up (`grow_viewport`). When the *terminal* shrank the
    /// scroll is skipped — the emulator already moved those rows, and scrolling
    /// again would move the viewport twice. Returns whether the caller must
    /// rebuild what sits above the viewport and repaint the viewport in full.
    pub fn resize_viewport(&mut self, height: u16) -> io::Result<bool> {
        let screen = self.backend.size()?;
        let terminal_height_shrank = screen.height < self.screen_size.height;
        let terminal_height_grew = screen.height > self.screen_size.height;
        let viewport_was_bottom_aligned = self.viewport_area.bottom() == self.screen_size.height;
        let previous_area = self.viewport_area;

        let mut area = self.viewport_area;
        area.height = height.min(screen.height).max(1);
        area.width = screen.width;
        let mut needs_full_repaint = false;

        if area.bottom() > screen.height {
            let scroll_by = area.bottom() - screen.height;
            if !terminal_height_shrank {
                self.backend.scroll_region_up(0..area.top(), scroll_by)?;
            }
            area.y = screen.height.saturating_sub(area.height);
        } else if terminal_height_grew && viewport_was_bottom_aligned {
            area.y = screen.height.saturating_sub(area.height);
        }

        if area != self.viewport_area {
            // Incident: clearing from the *old* anchor alone leaves a stale
            // composer box per resize step — on a shrink the new anchor is
            // above the old one, and once the old anchor falls off the shorter
            // screen the clear degenerates to a single row.
            let clear_position = Position::new(0, previous_area.y.min(area.y));
            self.set_viewport_area(area);
            self.clear_after_position(clear_position)?;
            needs_full_repaint = true;
        }
        self.screen_size = screen;
        Ok(needs_full_repaint)
    }

    /// codex `custom_terminal.rs::invalidate_viewport`: force the next draw to
    /// repaint every viewport cell. Resetting the diff buffer alone is not
    /// enough — a default-style space equals its previous cell, so stale
    /// terminal content shows through the blanks. ratatui 0.29 has no per-cell
    /// `AlwaysUpdate`, so the previous buffer is poisoned with a symbol no
    /// renderer emits.
    pub fn invalidate_viewport(&mut self) {
        let previous = self.previous_buffer_mut();
        previous.reset();
        for cell in &mut previous.content {
            cell.set_symbol("\u{0}");
        }
    }

    /// codex resize reflow: after the emulator re-wraps the screen, the rows
    /// above the viewport hold mangled copies of earlier frames. Rebuild them
    /// from transcript source instead of trusting what the re-wrap left there.
    pub fn repaint_history<F: FnOnce(&mut Buffer)>(&mut self, draw_fn: F) -> io::Result<()> {
        let rows = self.viewport_area.top();
        if rows == 0 || self.screen_size.width == 0 {
            return Ok(());
        }
        for y in 0..rows {
            self.backend.set_cursor_position(Position { x: 0, y })?;
            self.backend.clear_region(ClearType::CurrentLine)?;
        }
        let area = Rect {
            x: 0,
            y: 0,
            width: self.screen_size.width,
            height: rows,
        };
        let mut buffer = Buffer::empty(area);
        draw_fn(&mut buffer);
        let cells = buffer.content.clone();
        self.draw_lines(0, rows, &cells)?;
        self.backend.set_cursor_position(self.last_cursor)
    }

    pub fn draw<F: FnOnce(&mut Frame)>(&mut self, render: F) -> io::Result<()> {
        let viewport_area = self.viewport_area;
        let mut frame = Frame {
            viewport_area,
            buffer: self.current_buffer_mut(),
        };
        render(&mut frame);
        self.flush()?;
        self.backend.hide_cursor()?;
        self.previous_buffer_mut().reset();
        self.current = 1 - self.current;
        self.backend.flush()
    }

    fn flush(&mut self) -> io::Result<()> {
        let previous = &self.buffers[1 - self.current];
        let current = &self.buffers[self.current];
        let updates = previous.diff(current);
        // The anchor `reanchor_after_resize` compares against: where the
        // terminal's cursor sits once this frame has been written.
        if let Some(&(x, y, _)) = updates.last() {
            self.last_cursor = Position { x, y };
        }
        self.backend.draw(updates.into_iter())
    }

    /// Clear from the viewport's top row through the end of the screen and
    /// force a full repaint on the next draw.
    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
    }

    fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// U3: commit finished cells above the viewport using DEC scroll regions
    /// (ratatui's `insert_before_scrolling_regions`, ported onto the mutable
    /// viewport). The no-scroll-region fallback is dropped — the feature is on.
    pub fn insert_before<F: FnOnce(&mut Buffer)>(
        &mut self,
        mut height: u16,
        draw_fn: F,
    ) -> io::Result<()> {
        let area = Rect {
            x: 0,
            y: 0,
            width: self.viewport_area.width,
            height,
        };
        let mut buffer = Buffer::empty(area);
        draw_fn(&mut buffer);
        let mut cells = buffer.content.as_slice();

        if self.viewport_area.height >= self.screen_size.height {
            // The viewport owns the whole screen: borrow its top row, draw one
            // line into it, and scroll that row into scrollback, repeatedly.
            let mut first = true;
            while !cells.is_empty() {
                cells = if first {
                    self.draw_lines(0, 1, cells)?
                } else {
                    self.draw_lines_over_cleared(0, 1, cells)?
                };
                first = false;
                self.backend.scroll_region_up(0..1, 1)?;
            }
            let width = usize::from(self.viewport_area.width);
            let top_line = self.buffers[1 - self.current]
                .content
                .get(..width)
                .unwrap_or_default()
                .to_vec();
            self.draw_lines_over_cleared(0, 1, &top_line)?;
            return self.backend.set_cursor_position(self.last_cursor);
        }

        let viewport_top = self.viewport_area.top();
        let viewport_bottom = self.viewport_area.bottom();
        let screen_bottom = self.screen_size.height;
        if viewport_bottom < screen_bottom {
            let to_draw = height.min(screen_bottom - viewport_bottom);
            self.backend
                .scroll_region_down(viewport_top..viewport_bottom + to_draw, to_draw)?;
            cells = self.draw_lines_over_cleared(viewport_top, to_draw, cells)?;
            self.set_viewport_area(Rect {
                y: viewport_top + to_draw,
                ..self.viewport_area
            });
            height -= to_draw;
        }

        let viewport_top = self.viewport_area.top();
        while height > 0 && viewport_top > 0 {
            let to_draw = height.min(viewport_top);
            self.backend.scroll_region_up(0..viewport_top, to_draw)?;
            cells = self.draw_lines_over_cleared(viewport_top - to_draw, to_draw, cells)?;
            height -= to_draw;
        }
        // Cursor-position-neutral, so the resize anchor stays meaningful
        // (codex `insert_history.rs`).
        self.backend.set_cursor_position(self.last_cursor)
    }

    fn draw_lines<'a>(
        &mut self,
        y_offset: u16,
        lines: u16,
        cells: &'a [Cell],
    ) -> io::Result<&'a [Cell]> {
        let width = usize::from(self.screen_size.width);
        let take = width.saturating_mul(usize::from(lines)).min(cells.len());
        let (to_draw, remainder) = cells.split_at(take);
        if lines > 0 && width > 0 {
            let iter = to_draw.iter().enumerate().filter_map(|(i, cell)| {
                let x = u16::try_from(i % width).ok()?;
                let y = y_offset.checked_add(u16::try_from(i / width).ok()?)?;
                Some((x, y, cell))
            });
            self.backend.draw(iter)?;
            self.backend.flush()?;
        }
        Ok(remainder)
    }

    fn draw_lines_over_cleared<'a>(
        &mut self,
        y_offset: u16,
        lines: u16,
        cells: &'a [Cell],
    ) -> io::Result<&'a [Cell]> {
        let width = usize::from(self.screen_size.width);
        let take = width.saturating_mul(usize::from(lines)).min(cells.len());
        let (to_draw, remainder) = cells.split_at(take);
        if lines > 0 && width > 0 {
            let area = Rect::new(0, y_offset, self.screen_size.width, lines);
            let old = Buffer::empty(area);
            let new = Buffer {
                area,
                content: to_draw.to_vec(),
            };
            self.backend.draw(old.diff(&new).into_iter())?;
            self.backend.flush()?;
        }
        Ok(remainder)
    }
}
