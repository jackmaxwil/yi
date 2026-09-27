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
    rendered: Mutex<Rendered>,
}

/// Incident: a scrolled pane re-rendered every cell each frame, 114 ms deep in a session.
#[derive(Default)]
struct Rendered {
    key: Option<(usize, TranscriptMode, ColorTier, bool)>,
    start: usize,
    rows: VecDeque<Vec<Line<'static>>>,
    growing: Option<Growing>,
}

/// Incident: every slice merged into a streaming answer re-rendered and re-highlighted
/// the whole answer, 40-90 ms a commit. Its rows now grow past the settled prefix.
struct Growing {
    index: usize,
    settled: crate::markdown::Settled,
    stash: Vec<Line<'static>>,
    kept: usize,
    marked: bool,
}

impl Rendered {
    fn cell_rows(
        &mut self,
        (cells, index): (&VecDeque<Cell>, usize),
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
    ) -> Vec<Line<'static>> {
        let Some(cell) = cells.get(index) else {
            return Vec::new();
        };
        let (Cell::Assistant { markdown }, true) = (cell, index + 1 == cells.len()) else {
            return cell.lines(width, theme, mode, 0);
        };
        let _span = yi_types::trace::span("tui.cell_grow").arg("bytes", markdown.len());
        let grow = match self.growing.take() {
            Some(grow) if grow.index == index && !grow.stash.is_empty() => {
                self.growing.insert(grow)
            }
            _ => self.growing.insert(Growing {
                index,
                settled: crate::markdown::Settled::default(),
                stash: Vec::new(),
                kept: 0,
                marked: true,
            }),
        };
        let inner = width.saturating_sub(crate::cell::GUTTER.len());
        let Some((settled, tail)) = grow.settled.advance(markdown, inner, theme) else {
            self.growing = None;
            return cell.lines(width, theme, mode, 0);
        };
        let mut rows = std::mem::take(&mut grow.stash);
        rows.truncate(grow.kept.max(1));
        if rows.is_empty() {
            rows.push(Line::default());
        }
        let marked = grow.marked && settled.iter().all(is_blank);
        rows.extend(crate::cell::gutter(settled, grow.marked, theme));
        (grow.kept, grow.marked) = (rows.len(), marked);
        rows.extend(crate::cell::gutter(tail, marked, theme));
        rows
    }
}

fn is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

/// Every cell pads its own seam, so one blank row is the separator. Only the last `keep`
/// are cloned: a pane shows a screenful of an answer thousands of rows long.
fn squeeze_blanks(
    cells: &[Vec<Line<'static>>],
    first: usize,
    keep: usize,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut out: Vec<(usize, &Line<'static>)> = Vec::new();
    for (owner, line) in (first..)
        .zip(cells)
        .flat_map(|(owner, rows)| rows.iter().map(move |line| (owner, line)))
    {
        if is_blank(line) && out.last().is_some_and(|(_, last)| is_blank(last)) {
            continue;
        }
        out.push((owner, line));
    }
    let skip = out.len().saturating_sub(keep);
    out.into_iter()
        .skip(skip)
        .map(|(owner, line)| (line.clone(), owner))
        .unzip()
}

impl History {
    pub fn clear(&mut self) {
        self.cells.clear();
        *self
            .rendered
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner) = Rendered::default();
    }

    fn forget_last(&mut self) {
        let rendered = self
            .rendered
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        if rendered.start.saturating_add(rendered.rows.len()) == self.cells.len()
            && let Some(rows) = rendered.rows.pop_back()
            && let Some(grow) = &mut rendered.growing
            && grow.index + 1 == self.cells.len()
        {
            grow.stash = rows;
        }
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
                ..Rendered::default()
            };
        }
        // Appended cells render newest-first up to the cap, never a whole replay chunk.
        let done = rendered.start.saturating_add(rendered.rows.len());
        let mut fresh = VecDeque::new();
        let (mut index, mut held) = (self.cells.len(), 0_usize);
        while index > done && held <= cap {
            index -= 1;
            let rows = rendered.cell_rows((&self.cells, index), width, theme, mode);
            held = held.saturating_add(rows.len());
            fresh.push_front(rows);
        }
        if index > done {
            rendered.start = index;
            rendered.rows = fresh;
        } else {
            rendered.rows.extend(fresh);
        }
        let mut held: usize = rendered.rows.iter().map(Vec::len).sum();
        while (held <= cap || rendered.start > back_to) && rendered.start > 0 {
            rendered.start -= 1;
            let start = rendered.start;
            let rows = rendered.cell_rows((&self.cells, start), width, theme, mode);
            held = held.saturating_add(rows.len());
            rendered.rows.push_front(rows);
        }
        let start = rendered.start;
        read(start, rendered.rows.make_contiguous())
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
            return self.forget_last();
        }
        // Thought merges for the same reason plus one of its own: `normal` renders it as a
        // line count, and a per-slice count would name the last paragraph, not the thought.
        if let Cell::Thought { markdown } = &cell
            && let Some(Cell::Thought { markdown: head }) = self.cells.back_mut()
        {
            head.push_str(markdown);
            return self.forget_last();
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
            return self.forget_last();
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
        self.replay_last(width, theme, mode, cap, usize::MAX).0
    }

    fn replay_last(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        cap: usize,
        keep: usize,
    ) -> (Vec<Line<'static>>, Vec<usize>) {
        self.rendered(width, theme, mode, (cap, usize::MAX), |start, cells| {
            let mut rows = 0usize;
            let mut from = cells.len();
            for cell in cells.iter().rev() {
                rows = rows.saturating_add(cell.len());
                from = from.saturating_sub(1);
                if rows > cap {
                    break;
                }
            }
            let first = start.saturating_add(from);
            squeeze_blanks(cells.get(from..).unwrap_or_default(), first, keep)
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
    ) -> (usize, Vec<Line<'static>>, Vec<usize>) {
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
            let (lines, owners) = squeeze_blanks(tail, from, usize::MAX);
            (from, lines, owners)
        })
    }

    pub fn source(&self, index: usize) -> Option<&str> {
        match self.cells.get(index)? {
            Cell::User { text } => Some(text),
            Cell::Assistant { markdown } | Cell::Thought { markdown } => Some(markdown),
            _ => None,
        }
    }

    pub fn lines(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        rows: usize,
    ) -> (Vec<Line<'static>>, Vec<usize>) {
        self.replay_last(width, theme, mode, rows.max(1), rows)
    }
}
