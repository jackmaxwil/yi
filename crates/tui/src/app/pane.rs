use ratatui::layout::Rect;

use super::App;
use crate::render::Chip;
use crate::transcript::Said;

/// What the last pane paint drew: its top row and left column, and for each row its transcript
/// cell (`None` on the live turn) and run chip.
#[derive(Default)]
pub(crate) struct PaneRows {
    pub(crate) top: u16,
    pub(crate) left: u16,
    pub(crate) rows: Vec<Option<(usize, Chip)>>,
    /// Rows above the composer when no transcript row was drawn, the room a start screen has.
    pub(crate) blank: Option<u16>,
}

impl App {
    /// What a pinned view's rows were measured at: the width, and the card budget heights read.
    pub(crate) fn pane_key(&self) -> (usize, usize) {
        (
            self.content_width(),
            crate::budget::card_rows(self.rows, false),
        )
    }

    pub fn pane_row(&self, y: u16) -> Option<Option<(usize, Option<&str>)>> {
        let rows = &self.pane_rows;
        let row = rows.rows.get(usize::from(y.checked_sub(rows.top)?))?;
        Some(row.as_ref().map(|row| (row.0, self.history.source(row.0))))
    }

    /// Each run chip on the last paint: where it stands, and the cell and ordinal naming the
    /// shell block it runs; the block itself is read once, on the press.
    pub fn run_chips(&self) -> Vec<(Rect, usize, usize)> {
        let rows = &self.pane_rows;
        rows.rows
            .iter()
            .enumerate()
            .filter_map(|(at, row)| {
                let (cell, chip) = row.as_ref()?;
                let (ordinal, from, wide) = (*chip)?;
                let y = u16::try_from(at).ok()?.checked_add(rows.top)?;
                let x = u16::try_from(from).ok()?.checked_add(rows.left)?;
                let wide = u16::try_from(wide).ok()?;
                Some((Rect::new(x, y, wide, 1), *cell, ordinal))
            })
            .collect()
    }

    /// Each folded thought on the last paint: its fold, the last row its cell drew, and the cell,
    /// so a press can open the whole thought where a scrollback row cannot.
    pub fn folded_thoughts(&self) -> Vec<(Rect, usize)> {
        if self.mode == crate::cell::TranscriptMode::Verbose {
            return Vec::new();
        }
        let rows = &self.pane_rows;
        let width = u16::try_from(self.width).unwrap_or(u16::MAX);
        let mut out: Vec<(Rect, usize)> = Vec::new();
        for (at, row) in rows.rows.iter().enumerate() {
            let Some((cell, _)) = row else { continue };
            let thought = matches!(self.history.said(*cell), Some((Said::Thought, _)));
            let Some(y) = u16::try_from(at)
                .ok()
                .and_then(|at| at.checked_add(rows.top))
            else {
                continue;
            };
            if out.last().is_some_and(|(_, last)| last == cell) {
                out.pop();
            }
            if thought {
                out.push((Rect::new(rows.left, y, width, 1), *cell));
            }
        }
        out
    }

    /// The shell block the nth run chip of `cell` stands for, parsed from the answer once.
    pub fn shell_block(&self, cell: usize, ordinal: usize) -> Option<String> {
        crate::markdown::shell_blocks(self.history.source(cell)?)
            .get(ordinal)
            .cloned()
    }
}
