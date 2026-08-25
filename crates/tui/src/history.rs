use std::collections::VecDeque;

use ratatui::text::Line;

use crate::cell::{Cell, TranscriptMode};
use crate::colors::Theme;

/// Transcript cells kept so the rows above the viewport can be rebuilt at the
/// new width after a resize — codex's resize reflow rebuilds those rows from
/// transcript source rather than trusting what the emulator's re-wrap left
/// there. Only the tail that fits above the viewport is ever rendered.
const CAPACITY: usize = 256;

#[derive(Default)]
pub struct History {
    cells: VecDeque<Cell>,
}

impl History {
    pub fn clear(&mut self) {
        self.cells.clear();
    }

    /// Streaming commits one slice per stable blank line, and each slice
    /// re-rendered on its own would take a fresh bullet gutter — a reflowed
    /// repaint grew one bullet per paragraph where the live paint had one per
    /// message. Consecutive slices merge back into the message they came from.
    pub fn retain(&mut self, cell: Cell) {
        if let Cell::Assistant { markdown } = &cell
            && let Some(Cell::Assistant { markdown: head }) = self.cells.back_mut()
        {
            head.push_str(markdown);
            return;
        }
        if self.cells.len() >= CAPACITY {
            self.cells.pop_front();
        }
        self.cells.push_back(cell);
    }

    /// The last `rows` lines of the transcript re-rendered at `width`.
    pub fn lines(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        rows: usize,
    ) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for cell in &self.cells {
            lines.extend(cell.lines(width, theme, mode, 0));
        }
        let skip = lines.len().saturating_sub(rows);
        lines.split_off(skip)
    }
}
