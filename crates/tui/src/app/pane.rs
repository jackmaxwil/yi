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

    /// Each position of the composer's idle improve control on the last paint, as a rectangle.
    pub fn rewrite_slots(&self) -> Vec<(Rect, Option<yi_types::config::RewriteLength>)> {
        let Some((left, row)) = self.composer.foot_at() else {
            return Vec::new();
        };
        let found = self.composer.rewrite.slots(self.width);
        found
            .into_iter()
            .map(|(at, wide, position)| {
                (Rect::new(left.saturating_add(at), row, wide, 1), position)
            })
            .collect()
    }

    /// Everything on the last paint a press or a hover can land on: run chips and the improve
    /// control's positions.
    pub fn chat_hits(&self) -> crate::hits::ChatHits {
        crate::hits::ChatHits {
            runs: self.run_chips(),
            improve: self.rewrite_slots(),
        }
    }

    pub fn pick_rewrite(&mut self, position: Option<yi_types::config::RewriteLength>) {
        self.composer.rewrite.pick(position);
        self.scheduler.request();
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

    /// A press on transcript cell `index` opens it whole where it stands, or folds it back.
    pub fn toggle_open(&mut self, index: usize) {
        self.history.toggle(index);
        self.scheduler.request();
    }

    /// The shell block the nth run chip of `cell` stands for, parsed from the answer once.
    pub fn shell_block(&self, cell: usize, ordinal: usize) -> Option<String> {
        crate::markdown::shell_blocks(self.history.source(cell)?)
            .get(ordinal)
            .cloned()
    }
}
