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
    pub fn retain(&mut self, cell: Cell) {
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
