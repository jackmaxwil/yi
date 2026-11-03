use ratatui::layout::Rect;

use super::App;
use crate::render::Chip;

/// What the last pane paint drew: its top row and left column, and for each row its transcript
/// cell (`None` on the live turn) and run chip.
#[derive(Default)]
pub(crate) struct PaneRows {
    pub(crate) top: u16,
    pub(crate) left: u16,
    pub(crate) rows: Vec<Option<(usize, Chip)>>,
}

impl App {
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

    /// The shell block the nth run chip of `cell` stands for, parsed from the answer once.
    pub fn shell_block(&self, cell: usize, ordinal: usize) -> Option<String> {
        crate::markdown::shell_blocks(self.history.source(cell)?)
            .get(ordinal)
            .cloned()
    }
}
