//! A session's avatar: a 5×5 mirrored identicon seeded by the session id, rastered for
//! the kitty graphics protocol, with the initials tile every terminal draws under it.

use std::collections::HashMap;
use std::io::Write;

use yi_tui::orb::kitty;

/// Pixels per side; 8 px cells, so a two-column, one-row placement is one cell per glyph.
pub const PX: usize = 40;
const CELL: usize = 6;
const MARGIN: usize = 6;
const FIRST_ID: u32 = 8000;

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
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    out
}

pub fn assign_accents(seeds: &[&str]) -> Vec<usize> {
    let count = yi_tui::colors::ACCENT_RGB.len();
    let mut taken = vec![false; count];
    let mut out = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let start = yi_tui::colors::accent_index(seed);
        let pick = (0..count)
            .map(|step| (start + step) % count)
            .find(|hue| !taken.get(*hue).copied().unwrap_or(true))
            .unwrap_or(start);
        if let Some(slot) = taken.get_mut(pick) {
            *slot = true;
        }
        out.push(pick);
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
    pub key: String,
    pub seed: String,
    pub accent: (u8, u8, u8),
}

/// One placement per rail row on screen; moved rows re-place, gone rows delete.
#[derive(Default)]
pub struct Avatars {
    pool: ImageIds,
    ids: HashMap<String, ImageId>,
    placed: HashMap<String, (u16, u16, u16, u16)>,
}

impl Avatars {
    pub fn sync(&mut self, out: &mut impl Write, rows: &[Placement]) {
        let wanted: Vec<&str> = rows.iter().map(|place| place.key.as_str()).collect();
        let gone: Vec<String> = self
            .placed
            .keys()
            .filter(|key| !wanted.contains(&key.as_str()))
            .cloned()
            .collect();
        // Incident: the delete frees the image data and the id stayed in the ledger, so a row
        // that scrolled back in placed a freed image and showed nothing.
        for key in gone {
            self.placed.remove(&key);
            if let Some(image) = self.ids.remove(&key) {
                let _ = kitty::delete_id(out, image.raw());
            }
        }
        for place in rows {
            let (col, row, cols, rows) = (place.col, place.row, place.cols, place.rows);
            let image = match self.ids.get(&place.key) {
                Some(image) => *image,
                None => {
                    let Some(image) = self.pool.allocate() else {
                        continue;
                    };
                    let _ = kitty::transmit(
                        out,
                        image.raw(),
                        &rgba(&grid(&place.seed), place.accent),
                        (PX, PX),
                    );
                    self.ids.insert(place.key.clone(), image);
                    image
                }
            };
            if self.placed.get(&place.key) == Some(&(col, row, cols, rows)) {
                continue;
            }
            if kitty::place(out, image.raw(), col, row, cols, rows).is_ok() {
                self.placed
                    .insert(place.key.clone(), (col, row, cols, rows));
            }
        }
    }

    /// Incident: a resize clears the screen, and kitty drops every placement with the cells
    /// under it; the ledger still said placed, so the tiles showed until the row moved.
    pub fn forget(&mut self) {
        self.placed.clear();
        self.ids.clear();
    }

    pub fn hide_all(&mut self, out: &mut impl Write) {
        for key in self.placed.keys() {
            if let Some(image) = self.ids.get(key) {
                let _ = kitty::delete_id(out, image.raw());
            }
        }
        for key in std::mem::take(&mut self.placed).into_keys() {
            self.ids.remove(&key);
        }
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
    fn a_forgotten_ledger_places_again_after_a_clear() {
        let place = Placement {
            col: 2,
            row: 0,
            cols: 4,
            rows: 2,
            key: "s-alpha".to_owned(),
            seed: "s-alpha".to_owned(),
            accent: (1, 2, 3),
        };
        let mut avatars = Avatars::default();
        let mut first = Vec::new();
        avatars.sync(&mut first, std::slice::from_ref(&place));
        assert!(
            String::from_utf8_lossy(&first).contains("a=p,i="),
            "placed once"
        );
        let mut again = Vec::new();
        avatars.sync(&mut again, std::slice::from_ref(&place));
        assert!(again.is_empty(), "an unmoved placement writes nothing");
        avatars.forget();
        let mut after = Vec::new();
        avatars.sync(&mut after, std::slice::from_ref(&place));
        assert!(
            String::from_utf8_lossy(&after).contains("a=p,i="),
            "placed again after a clear"
        );
    }

    #[test]
    fn a_row_that_scrolls_back_in_transmits_again() {
        let place = Placement {
            col: 2,
            row: 0,
            cols: 4,
            rows: 2,
            key: "s-alpha".to_owned(),
            seed: "s-alpha".to_owned(),
            accent: (1, 2, 3),
        };
        let mut avatars = Avatars::default();
        avatars.sync(&mut Vec::new(), std::slice::from_ref(&place));
        let mut gone = Vec::new();
        avatars.sync(&mut gone, &[]);
        assert!(
            String::from_utf8_lossy(&gone).contains("a=d,d=I"),
            "deleted"
        );
        let mut back = Vec::new();
        avatars.sync(&mut back, std::slice::from_ref(&place));
        let back = String::from_utf8_lossy(&back);
        assert!(
            back.contains("a=t"),
            "transmitted again, not placed over freed data"
        );
        assert!(back.contains("a=p,i="), "placed");
    }

    #[test]
    fn visible_agents_never_share_a_hue() {
        let seeds: Vec<String> = (0..14).map(|n| format!("s-{n:02}")).collect();
        let refs: Vec<&str> = seeds.iter().map(String::as_str).collect();
        let hues = assign_accents(&refs);
        let distinct: std::collections::HashSet<usize> = hues.iter().copied().collect();
        assert_eq!(distinct.len(), 14, "{hues:?}");
        let fewer = assign_accents(&refs[..5]);
        assert_eq!(
            &hues[..5],
            &fewer[..],
            "a shorter list keeps the same hues for its head"
        );
    }

    #[test]
    fn ids_run_out_without_wrapping() {
        let mut pool = ImageIds { next: u32::MAX - 1 };
        assert!(pool.allocate().is_some());
        assert!(pool.allocate().is_none());
    }
}
