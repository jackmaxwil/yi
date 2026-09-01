// Derived from `ratatui::Terminal` (MIT, Florian Dehau / The Ratatui Developers)
// with codex's mutable-viewport model (`codex-rs/tui/src/custom_terminal.rs`).

// Incident: ratatui's `Viewport::Inline(h)` fixes the height at construction, so
// a live region that grows or shrinks cannot follow it, and rebuilding the
// terminal instead appends blank lines into scrollback on every rebuild.
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

    /// Growth scrolls the rows above the viewport up, but when the *terminal*
    /// shrank the scroll is skipped — the emulator already moved those rows, and
    /// scrolling again moves the viewport twice. True ⇒ rebuild above and repaint.
    pub fn resize_viewport(&mut self, height: u16, floor: u16) -> io::Result<bool> {
        let screen = self.backend.size()?;
        let terminal_height_shrank = screen.height < self.screen_size.height;
        let terminal_height_grew = screen.height > self.screen_size.height;
        let viewport_was_bottom_aligned = self.viewport_area.bottom() == self.screen_size.height;
        let previous_area = self.viewport_area;

        let mut area = self.viewport_area;
        area.height = height.min(screen.height).max(1);
        area.width = screen.width;
        let mut needs_full_repaint = false;
        let mut vacated = 0;

        if area.bottom() > screen.height {
            let scroll_by = area.bottom() - screen.height;
            if !terminal_height_shrank {
                // Incident: scrolling only above the viewport left every live row
                // one position stale, so a streamed line erased and repainted the
                // region. The `floor` rows under the live text hold still instead.
                let carries_live = previous_area.bottom() == screen.height
                    && previous_area.width == screen.width
                    && floor <= previous_area.height;
                let region_bottom = if carries_live {
                    screen.height - floor
                } else {
                    area.top()
                };
                self.backend.scroll_region_up(0..region_bottom, scroll_by)?;
                if carries_live {
                    vacated = scroll_by;
                }
            }
            area.y = screen.height.saturating_sub(area.height);
        } else if terminal_height_grew && viewport_was_bottom_aligned {
            area.y = screen.height.saturating_sub(area.height);
        }

        if area != self.viewport_area {
            if vacated > 0 {
                self.rebase_after_scroll(area, previous_area.height - floor, vacated);
            } else {
                // Incident: clearing from the old anchor alone left a stale composer
                // box per resize step — on a shrink the new anchor is above the old
                // one, which off a shorter screen degenerates to a single row.
                let clear_position = Position::new(0, previous_area.y.min(area.y));
                self.set_viewport_area(area);
                self.clear_after_position(clear_position)?;
                needs_full_repaint = true;
            }
        }
        self.screen_size = screen;
        Ok(needs_full_repaint)
    }

    /// The screen still holds the last frame, only at other coordinates, so the
    /// diff baseline is re-indexed rather than thrown away: the rows above the
    /// scroll keep their place and the `vacated` rows it opened arrive blank.
    fn rebase_after_scroll(&mut self, area: Rect, above: u16, vacated: u16) {
        let row = |n: u16| usize::from(area.width).saturating_mul(usize::from(n));
        let previous = self.previous_buffer_mut();
        let at = row(above).min(previous.content.len());
        previous
            .content
            .splice(at..at, std::iter::repeat_n(Cell::EMPTY, row(vacated)));
        previous.resize(area);
        self.current_buffer_mut().resize(area);
        self.viewport_area = area;
    }

    /// Resetting the diff buffer alone leaves stale terminal content showing
    /// through the blanks — a default-style space equals its previous cell. With
    /// no per-cell `AlwaysUpdate` in 0.29, poison it with an unemittable symbol.
    pub fn invalidate_viewport(&mut self) {
        let previous = self.previous_buffer_mut();
        previous.reset();
        for cell in &mut previous.content {
            cell.set_symbol("\u{0}");
        }
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

    /// Also forces a full repaint on the next draw.
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
            // The viewport owns the whole screen: draw into its top row and
            // scroll that row into scrollback, once per line.
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

impl<B: Backend + std::io::Write> Terminal<B> {
    /// One `write!`, not a sequence of backend calls: some terminals honour the
    /// scrollback purge only when it arrives with the clear. Reset scroll region,
    /// reset style, home, clear screen, purge scrollback, home again.
    pub fn clear_scrollback_and_visible_screen(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        std::io::Write::flush(&mut self.backend)?;
        self.last_cursor = Position { x: 0, y: 0 };
        self.previous_buffer_mut().reset();
        let mut area = self.viewport_area;
        area.y = 0;
        self.set_viewport_area(area);
        Ok(())
    }
}
