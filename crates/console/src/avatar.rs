//! A session's avatar: a 5×5 mirrored identicon seeded by the session id, rastered for
//! the kitty graphics protocol, with the initials tile every terminal draws under it.

use std::collections::HashMap;
use std::io::Write;

use yi_tui::orb::kitty;

use crate::model::SessionId;

/// Pixels per side; 8 px cells, so a two-column, one-row placement is one cell per glyph.
pub const PX: usize = 40;
const CELL: usize = 6;
const MARGIN: usize = 5;
const FIRST_ID: u32 = 8000;
const CAP: usize = 64;

/// A kitty image id the console issued; never a bare number crossing a seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageId(u32);

impl ImageId {
    pub fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Debug)]
pub struct ImageIds {
    next: u32,
}

impl Default for ImageIds {
    fn default() -> Self {
        Self { next: FIRST_ID }
    }
}

impl ImageIds {
    /// `None` when the space is spent; the row keeps its text tile.
    pub fn allocate(&mut self) -> Option<ImageId> {
        let id = self.next;
        self.next = self.next.checked_add(1)?;
        Some(ImageId(id))
    }
}

/// Row-major 5×5, mirror-symmetric about the middle column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid([bool; 25]);

impl Grid {
    pub fn on(&self, row: usize, col: usize) -> bool {
        self.0
            .get(row.saturating_mul(5).saturating_add(col))
            .copied()
            .unwrap_or(false)
    }
}

fn fnv1a(seed: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Fifteen bits pick the left three columns of each row; the right two mirror them, and
/// the centre cell is always on so no grid is blank.
pub fn grid(seed: &str) -> Grid {
    let bits = fnv1a(seed);
    let mut cells = [false; 25];
    for row in 0..5_usize {
        for col in 0..3_usize {
            let bit = row.saturating_mul(3).saturating_add(col);
            let on = (bits >> u32::try_from(bit).unwrap_or(0)) & 1 == 1;
            let mirror = 4_usize.saturating_sub(col);
            if let Some(cell) = cells.get_mut(row.saturating_mul(5).saturating_add(col)) {
                *cell = on;
            }
            if let Some(cell) = cells.get_mut(row.saturating_mul(5).saturating_add(mirror)) {
                *cell = on;
            }
        }
    }
    if let Some(centre) = cells.get_mut(12) {
        *centre = true;
    }
    Grid(cells)
}

/// 40×40 RGBA on an opaque dark ground: the image covers the tile text under it whole.
pub fn rgba(grid: &Grid, fg: (u8, u8, u8)) -> Vec<u8> {
    let mut out = Vec::with_capacity(PX.saturating_mul(PX).saturating_mul(4));
    for y in 0..PX {
        for x in 0..PX {
            let inside = (MARGIN..PX - MARGIN).contains(&x) && (MARGIN..PX - MARGIN).contains(&y);
            let (row, col) = ((y - MARGIN.min(y)) / CELL, (x - MARGIN.min(x)) / CELL);
            if inside && grid.on(row, col) {
                out.extend_from_slice(&[fg.0, fg.1, fg.2, 255]);
            } else {
                out.extend_from_slice(&[0x1e, 0x1e, 0x2e, 255]);
            }
        }
    }
    out
}

/// Where one session's avatar goes this frame, and the colour its name earned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
    pub session: SessionId,
    pub accent: (u8, u8, u8),
}

/// One placement per rail row on screen; moved rows re-place, gone rows delete.
#[derive(Default)]
pub struct Avatars {
    pool: ImageIds,
    ids: HashMap<SessionId, ImageId>,
    placed: HashMap<SessionId, (u16, u16, u16, u16)>,
}

impl Avatars {
    pub fn sync(&mut self, out: &mut impl Write, rows: &[Placement]) {
        let wanted: Vec<&SessionId> = rows.iter().map(|place| &place.session).collect();
        let gone: Vec<SessionId> = self
            .placed
            .keys()
            .filter(|id| !wanted.contains(id))
            .cloned()
            .collect();
        for id in gone {
            self.placed.remove(&id);
            if let Some(image) = self.ids.get(&id) {
                let _ = kitty::delete_id(out, image.raw());
            }
            if self.ids.len() > CAP {
                self.ids.remove(&id);
            }
        }
        for place in rows {
            let (id, col, row) = (&place.session, place.col, place.row);
            let (cols, rows) = (place.cols, place.rows);
            let image = match self.ids.get(id) {
                Some(image) => *image,
                None => {
                    let Some(image) = self.pool.allocate() else {
                        continue;
                    };
                    let _ =
                        kitty::transmit(out, image.raw(), &rgba(&grid(&id.0), place.accent), PX);
                    self.ids.insert(id.clone(), image);
                    image
                }
            };
            if self.placed.get(id) == Some(&(col, row, cols, rows)) {
                continue;
            }
            if kitty::place(out, image.raw(), col, row, cols, rows).is_ok() {
                self.placed.insert(id.clone(), (col, row, cols, rows));
            }
        }
    }

    pub fn hide_all(&mut self, out: &mut impl Write) {
        for (id, image) in &self.ids {
            if self.placed.contains_key(id) {
                let _ = kitty::delete_id(out, image.raw());
            }
        }
        self.placed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids_are_deterministic_symmetric_and_distinct() {
        assert_eq!(grid("s-alpha"), grid("s-alpha"));
        let g = grid("s-alpha");
        for row in 0..5 {
            for col in 0..5 {
                assert_eq!(g.on(row, col), g.on(row, 4 - col));
            }
        }
        assert!(g.on(2, 2));
        let distinct: std::collections::HashSet<[bool; 25]> =
            (0..100).map(|n| grid(&format!("s-{n:02}")).0).collect();
        assert!(distinct.len() >= 95, "{}", distinct.len());
        assert_eq!(rgba(&g, (1, 2, 3)).len(), PX * PX * 4);
    }

    #[test]
    fn ids_run_out_without_wrapping() {
        let mut pool = ImageIds { next: u32::MAX - 1 };
        assert!(pool.allocate().is_some());
        assert!(pool.allocate().is_none());
    }
}
