use ratatui::layout::Rect;

use super::App;
use crate::render::Chip;

/// What the last pane paint drew: its top row and left column, and for each row its transcript
/// cell (`None` on the live turn) and run chip.
#[derive(Default)]
pub(crate) struct PaneRows {
    pub(crate) top: u16,
    pub(crate) left: u16,
    pub(crate) cells: Vec<Option<usize>>,
    pub(crate) chips: Vec<Chip>,
}

impl App {
    pub fn pane_row(&self, y: u16) -> Option<Option<(usize, Option<&str>)>> {
        let rows = &self.pane_rows;
        let owner = *rows.cells.get(usize::from(y.checked_sub(rows.top)?))?;
        Some(owner.map(|index| (index, self.history.source(index))))
    }

    /// Each run chip on the last paint: where it stands and the shell block it runs.
    pub fn run_chips(&self) -> Vec<(Rect, String)> {
        let rows = &self.pane_rows;
        let mut blocks: Option<(usize, Vec<String>)> = None;
        let mut found = Vec::new();
        for (at, (cell, chip)) in rows.cells.iter().zip(&rows.chips).enumerate() {
            let (Some(cell), Some((ordinal, from, wide))) = (*cell, *chip) else {
                continue;
            };
            if blocks.as_ref().is_none_or(|(held, _)| *held != cell) {
                let source = self.history.source(cell).unwrap_or_default();
                blocks = Some((cell, crate::markdown::shell_blocks(source)));
            }
            let code = blocks.as_ref().and_then(|(_, held)| held.get(ordinal));
            let (Some(code), Ok(y), Ok(x), Ok(wide)) = (
                code,
                u16::try_from(at).map(|at| rows.top.saturating_add(at)),
                u16::try_from(from).map(|from| rows.left.saturating_add(from)),
                u16::try_from(wide),
            ) else {
                continue;
            };
            found.push((Rect::new(x, y, wide, 1), code.clone()));
        }
        found
    }
}
