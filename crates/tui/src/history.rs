use std::collections::VecDeque;

use ratatui::text::Line;

use crate::cell::{Cell, TranscriptMode};
use crate::colors::Theme;

/// Transcript cells kept as the source a resize rebuild renders from (U36).
/// The bound is the reflow row cap, not a cell count: what matters is how many
/// rows a rebuild is willing to write, and cells differ in height by two orders
/// of magnitude. Cells are dropped from the front only once the rows they would
/// render are already past that cap.
#[derive(Default)]
pub struct History {
    cells: VecDeque<Cell>,
}

impl History {
    pub fn clear(&mut self) {
        self.cells.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
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
        self.cells.push_back(cell);
    }

    /// Drop the cells a rebuild would never reach. Called with the same cap the
    /// rebuild uses, after a commit, so the retained set tracks what is
    /// actually replayable instead of growing without bound.
    pub fn trim_to_rows(&mut self, width: usize, theme: &Theme, mode: TranscriptMode, cap: usize) {
        let mut rows = 0usize;
        let mut keep = 0usize;
        for cell in self.cells.iter().rev() {
            rows = rows.saturating_add(cell.lines(width, theme, mode, 0).len());
            keep = keep.saturating_add(1);
            if rows > cap {
                break;
            }
        }
        while self.cells.len() > keep {
            self.cells.pop_front();
        }
    }

    /// Every retained cell re-rendered at `width`, newest-first until the row
    /// cap is exceeded (codex `render_transcript_lines_for_reflow`). The cap is
    /// enforced here, while rendering from source — never after writing to the
    /// terminal, because rows the terminal will not retain are rows nobody can
    /// scroll back to.
    pub fn replay(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        cap: usize,
    ) -> Vec<Line<'static>> {
        let mut blocks: VecDeque<Vec<Line<'static>>> = VecDeque::new();
        let mut rows = 0usize;
        for cell in self.cells.iter().rev() {
            let rendered = cell.lines(width, theme, mode, 0);
            rows = rows.saturating_add(rendered.len());
            blocks.push_front(rendered);
            if rows > cap {
                break;
            }
        }
        blocks.into_iter().flatten().collect()
    }

    /// The last `rows` lines of the transcript re-rendered at `width`.
    pub fn lines(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        rows: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = self.replay(width, theme, mode, rows.max(1));
        let skip = lines.len().saturating_sub(rows);
        lines.split_off(skip)
    }
}
