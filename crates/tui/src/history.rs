use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use ratatui::text::Line;

use crate::cell::{Cell, TranscriptMode};
use crate::colors::{ColorTier, Theme};

/// The source a resize rebuild renders from (§17.3). The bound is the reflow row cap, not a
/// cell count, since cells differ in height by two orders of magnitude.
#[derive(Default)]
pub struct History {
    cells: VecDeque<Cell>,
    /// Cells before this index are closed: a new message never merges into the last one's.
    sealed: usize,
    rendered: Mutex<Rendered>,
}

/// Incident: a scrolled pane re-rendered every cell each frame, 114 ms deep in a session.
#[derive(Default)]
struct Rendered {
    key: Option<(usize, TranscriptMode, ColorTier, bool)>,
    start: usize,
    rows: VecDeque<Vec<Line<'static>>>,
}

pub(crate) fn is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

/// A blank row separates two blocks of more than one row; one rule for scrollback and rebuild.
pub(crate) fn separated(rows_above: usize, blank_above: bool, next: &[Line<'_>]) -> bool {
    rows_above > 1 && next.len() > 1 && !blank_above && !next.first().is_some_and(is_blank)
}

/// Incident: a rebuild dropped every separator row scrollback had, so a resize moved rows.
fn join(cells: &[Vec<Line<'static>>]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut above: Option<&Vec<Line<'static>>> = None;
    for rows in cells.iter().filter(|rows| !rows.is_empty()) {
        if let Some(above) = above
            && separated(above.len(), above.last().is_some_and(is_blank), rows)
        {
            out.push(Line::default());
        }
        out.extend(rows.iter().cloned());
        above = Some(rows);
    }
    squeeze_blanks(out)
}

/// Every cell pads its own seam, so two blocks met across two or three empty rows; one
/// blank row is the separator, wherever the padding came from.
pub fn squeeze_blanks(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::with_capacity(lines.len());
    for line in lines {
        if is_blank(&line) && out.last().is_some_and(is_blank) {
            continue;
        }
        out.push(line);
    }
    out
}

impl History {
    pub fn clear(&mut self) {
        self.cells.clear();
        self.sealed = 0;
        *self
            .rendered
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner) = Rendered::default();
    }

    fn forget_last(&mut self, rows: Option<(usize, &[Line<'static>])>) {
        let rendered = self
            .rendered
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        if rendered.start.saturating_add(rendered.rows.len()) != self.cells.len() {
            return;
        }
        let width = rendered.key.map(|key| key.0);
        match (rows, rendered.rows.back_mut()) {
            (Some((at, rows)), Some(last)) if Some(at) == width => last.extend_from_slice(rows),
            _ => {
                rendered.rows.pop_back();
            }
        }
    }

    pub fn seal(&mut self) {
        self.sealed = self.cells.len();
    }

    fn rendered<R>(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        (cap, back_to): (usize, usize),
        read: impl FnOnce(usize, &[Vec<Line<'static>>]) -> R,
    ) -> R {
        let mut rendered = self.rendered.lock().unwrap_or_else(PoisonError::into_inner);
        let key = Some((width, mode, theme.tier, theme.dark));
        if rendered.key != key {
            *rendered = Rendered {
                key,
                start: self.cells.len(),
                rows: VecDeque::new(),
            };
        }
        let done = rendered.start.saturating_add(rendered.rows.len());
        for cell in self.cells.iter().skip(done) {
            rendered.rows.push_back(cell.lines(width, theme, mode, 0));
        }
        let mut held: usize = rendered.rows.iter().map(Vec::len).sum();
        while (held <= cap || rendered.start > back_to) && rendered.start > 0 {
            rendered.start -= 1;
            let Some(cell) = self.cells.get(rendered.start) else {
                break;
            };
            let rows = cell.lines(width, theme, mode, 0);
            held = held.saturating_add(rows.len());
            rendered.rows.push_front(rows);
        }
        let start = rendered.start;
        read(start, rendered.rows.make_contiguous())
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn last(&self) -> Option<&Cell> {
        self.cells.back()
    }

    pub fn retain(&mut self, cell: Cell) {
        self.retain_slice(cell, None);
    }

    /// Consecutive slices merge back into their message: alone, each took a fresh bullet gutter.
    /// `rows` extend the cached rows, so a pane does not re-render the message per slice.
    pub fn retain_slice(&mut self, cell: Cell, rows: Option<(usize, &[Line<'static>])>) {
        let open = self.cells.len() > self.sealed;
        if let Cell::Assistant { markdown } = &cell
            && open
            && let Some(Cell::Assistant { markdown: head }) = self.cells.back_mut()
        {
            head.push_str(markdown);
            return self.forget_last(rows);
        }
        // Thought merges for the same reason plus one of its own: `normal` renders it as a
        // line count, and a per-slice count would name the last paragraph, not the thought.
        if let Cell::Thought { markdown } = &cell
            && open
            && let Some(Cell::Thought { markdown: head }) = self.cells.back_mut()
        {
            head.push_str(markdown);
            return self.forget_last(rows);
        }
        if let Cell::Assistant { markdown } | Cell::Thought { markdown } = &cell
            && markdown.trim().is_empty()
        {
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
            return self.forget_last(None);
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
        self.rendered(width, theme, mode, (cap, usize::MAX), |_, cells| {
            let mut rows = 0usize;
            let mut from = cells.len();
            for cell in cells.iter().rev() {
                rows = rows.saturating_add(cell.len());
                from = from.saturating_sub(1);
                if rows > cap {
                    break;
                }
            }
            let tail = cells.get(from..).unwrap_or_default();
            join(tail)
        })
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn tail(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        rows: usize,
        at_most: usize,
    ) -> (usize, Vec<Line<'static>>) {
        self.rendered(width, theme, mode, (rows, at_most), |start, cells| {
            let mut held = 0usize;
            let mut from = start.saturating_add(cells.len());
            for cell in cells.iter().rev() {
                from = from.saturating_sub(1);
                held = held.saturating_add(cell.len());
                if from <= at_most && held > rows {
                    break;
                }
            }
            let tail = cells.get(from.saturating_sub(start)..).unwrap_or_default();
            (from, join(tail))
        })
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
