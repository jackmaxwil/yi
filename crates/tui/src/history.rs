use std::collections::VecDeque;

use ratatui::text::Line;

use crate::cell::{Cell, TranscriptMode};
use crate::colors::Theme;

/// The source a resize rebuild renders from (U36). The bound is the reflow row cap, not a
/// cell count, since cells differ in height by two orders of magnitude.
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

    /// Consecutive slices merge back into their message: each re-rendered alone takes a fresh
    /// bullet gutter, so a reflow grew one bullet per paragraph instead of per message.
    pub fn retain(&mut self, cell: Cell) {
        if let Cell::Assistant { markdown } = &cell
            && let Some(Cell::Assistant { markdown: head }) = self.cells.back_mut()
        {
            head.push_str(markdown);
            return;
        }
        // Thought merges for the same reason plus one of its own: `normal` renders it as a
        // line count, and a per-slice count would name the last paragraph, not the thought.
        if let Cell::Thought { markdown } = &cell
            && let Some(Cell::Thought { markdown: head }) = self.cells.back_mut()
        {
            head.push_str(markdown);
            return;
        }
        if let Cell::Advisory { source, text } = &cell
            && let Some(Cell::Advisory {
                source: head_source,
                text: head,
            }) = self.cells.back_mut()
            && head_source == source
        {
            head.push('\n');
            head.push_str(text);
            return;
        }
        self.cells.push_back(cell);
    }

    /// Newest-first until the row cap is exceeded, enforced here while rendering from source:
    /// rows the terminal will not retain are rows nobody can scroll back to.
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
