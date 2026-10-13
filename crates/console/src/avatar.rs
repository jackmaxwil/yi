//! A session's avatar: a 5×5 mirrored identicon seeded by the session id, rastered for
//! the kitty graphics protocol, with the initials tile every terminal draws under it.

use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Art {
    Identicon,
    Gear {
        width: usize,
        height: usize,
    },
    Closer {
        width: usize,
        height: usize,
    },
    Cap {
        width: usize,
        height: usize,
        left: bool,
    },
}

fn gear_distance(x: f64, y: f64) -> f64 {
    let r = x.hypot(y);
    let step = std::f64::consts::FRAC_PI_4;
    let theta = y.atan2(x);
    let off = theta - (theta / step).round() * step;
    let (round, half) = (0.05, (0.16, 0.15));
    let qx = (r * off.cos() - 0.82).abs() - half.0 + round;
    let qy = (r * off.sin()).abs() - half.1 + round;
    let tooth = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - round;
    (r - 0.7).min(tooth)
}

/// Coverage comes from the distance to the outline, so every edge is one pixel of blend.
pub fn gear_rgba(fg: (u8, u8, u8), width: usize, height: usize) -> Vec<u8> {
    let radius = (width as f64).min(height as f64 * 0.5) / 2.0;
    let stroke = (radius * 0.09).max(1.5) / 2.0;
    let (cx, cy) = (width as f64 / 2.0, height as f64 / 2.0);
    let mut out = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for py in 0..height {
        for px in 0..width {
            let x = (px as f64 + 0.5 - cx) / radius;
            let y = (py as f64 + 0.5 - cy) / radius;
            let edge = gear_distance(x, y).abs().min((x.hypot(y) - 0.28).abs());
            let alpha = (stroke - edge * radius + 0.5).clamp(0.0, 1.0);
            out.extend_from_slice(&[fg.0, fg.1, fg.2, (alpha * 255.0).round() as u8]);
        }
    }
    out
}

fn rounded_box((x, y): (f64, f64), (half_w, half_h): (f64, f64), round: f64) -> f64 {
    let (qx, qy) = (x.abs() - half_w + round, y.abs() - half_h + round);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - round
}

/// One rounded end of a pill whose body is cell background, so the far side runs off the image.
pub fn cap_rgba(fill: (u8, u8, u8), width: usize, height: usize, left: bool) -> Vec<u8> {
    let (w, h) = (width as f64, height as f64);
    let centre = if left { 1.5 * w } else { -0.5 * w };
    let mut out = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for py in 0..height {
        for px in 0..width {
            let at = (px as f64 + 0.5 - centre, py as f64 + 0.5 - h / 2.0);
            let edge = rounded_box(at, (1.5 * w, h / 2.0), (h * 0.32).min(w));
            let alpha = ((0.5 - edge).clamp(0.0, 1.0) * 255.0).round() as u8;
            out.extend_from_slice(&[fill.0, fill.1, fill.2, alpha]);
        }
    }
    out
}

fn segment((x, y): (f64, f64), a: f64) -> f64 {
    let t = ((x + y) / 2.0).clamp(-a, a);
    (x - t).hypot(y - t)
}

/// The X is knocked out of the chip, so the terminal's own ground shows through the strokes.
pub fn closer_rgba(fill: (u8, u8, u8), width: usize, height: usize) -> Vec<u8> {
    let (w, h) = (width as f64, height as f64);
    let (round, arm, stroke) = (h * 0.22, h * 0.2, (h * 0.05).max(0.9));
    let mut out = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for py in 0..height {
        for px in 0..width {
            let (x, y) = (px as f64 + 0.5 - w / 2.0, py as f64 + 0.5 - h / 2.0);
            let chip = (0.5 - rounded_box((x, y), (w / 2.0, h / 2.0), round)).clamp(0.0, 1.0);
            let cross = segment((x, y), arm).min(segment((x, -y), arm));
            let cut = (stroke - cross + 0.5).clamp(0.0, 1.0);
            let alpha = (chip * (1.0 - cut) * 255.0).round() as u8;
            out.extend_from_slice(&[fill.0, fill.1, fill.2, alpha]);
        }
    }
    out
}

fn pixels(place: &Placement) -> (Vec<u8>, (usize, usize)) {
    match place.art {
        Art::Identicon => (rgba(&grid(&place.seed), place.accent), (PX, PX)),
        Art::Gear { width, height } => (gear_rgba(place.accent, width, height), (width, height)),
        Art::Closer { width, height } => {
            (closer_rgba(place.accent, width, height), (width, height))
        }
        Art::Cap {
            width,
            height,
            left,
        } => (cap_rgba(place.accent, width, height, left), (width, height)),
    }
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
    pub state: Option<yi_tui::orb::OrbState>,
    pub art: Art,
}

/// Incident: kitty images draw above text, so an avatar under an open popup showed through it;
/// a placement a popup covers is left out until the popup closes.
pub fn uncovered(rows: Vec<Placement>, covers: &[ratatui::layout::Rect]) -> Vec<Placement> {
    rows.into_iter()
        .filter(|place| {
            let at = ratatui::layout::Rect::new(place.col, place.row, place.cols, place.rows);
            !covers.iter().any(|cover| cover.intersects(at))
        })
        .collect()
}

struct Motion {
    orb: yi_tui::orb::Orb,
    state: yi_tui::orb::OrbState,
    last: Instant,
    painted: bool,
}

impl Motion {
    /// A session waiting on the person draws one still frame: animating it woke the console
    /// fifteen times a second for a row whose words already say it waits.
    fn still(&self) -> bool {
        self.painted && self.state == yi_tui::orb::OrbState::Listening
    }
}

const MOTION_FRAME: Duration = Duration::from_millis(66);

/// Incident: kitty deletes an id's placements when new data arrives for it and Ghostty keeps
/// them, so an animating avatar vanished in kitty alone; new pixels are placed again (#1335).
fn retransmit(
    out: &mut impl Write,
    image: ImageId,
    rgba: &[u8],
    at: Option<&(u16, u16, u16, u16)>,
) {
    let _ = kitty::transmit(out, image.raw(), rgba, (PX, PX));
    if let Some(&(col, row, cols, rows)) = at {
        let _ = kitty::place(out, image.raw(), col, row, cols, rows);
    }
}

/// One placement per rail row on screen; moved rows re-place, gone rows delete.
#[derive(Default)]
pub struct Avatars {
    pool: ImageIds,
    ids: HashMap<String, ImageId>,
    placed: HashMap<String, (u16, u16, u16, u16)>,
    motion: HashMap<String, Motion>,
    forgotten: bool,
}

impl Avatars {
    pub fn sync(&mut self, out: &mut impl Write, rows: &[Placement]) {
        if std::mem::take(&mut self.forgotten) {
            self.hide_all(out);
        }
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
            self.motion.remove(&key);
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
                    let (data, size) = pixels(place);
                    let _ = kitty::transmit(out, image.raw(), &data, size);
                    self.ids.insert(place.key.clone(), image);
                    image
                }
            };
            match (place.state, self.motion.get_mut(&place.key)) {
                (Some(state), Some(motion)) => {
                    motion.painted &= motion.state == state;
                    motion.state = state;
                }
                (Some(state), None) => {
                    let motion = Motion {
                        orb: yi_tui::orb::Orb::default(),
                        state,
                        last: Instant::now(),
                        painted: false,
                    };
                    self.motion.insert(place.key.clone(), motion);
                }
                (None, Some(_)) => {
                    self.motion.remove(&place.key);
                    let identicon = rgba(&grid(&place.seed), place.accent);
                    retransmit(out, image, &identicon, self.placed.get(&place.key));
                }
                (None, None) => {}
            }
            if self.placed.get(&place.key) == Some(&(col, row, cols, rows)) {
                continue;
            }
            if kitty::place(out, image.raw(), col, row, cols, rows).is_ok() {
                self.placed
                    .insert(place.key.clone(), (col, row, cols, rows));
            }
        }
    }

    pub fn animate(&mut self, out: &mut impl Write) {
        for (key, motion) in &mut self.motion {
            let (Some(image), elapsed) = (self.ids.get(key), motion.last.elapsed()) else {
                continue;
            };
            if elapsed < MOTION_FRAME || motion.still() {
                continue;
            }
            motion.last = Instant::now();
            motion.painted = true;
            let frame = motion.orb.frame(elapsed, Some(motion.state));
            let pixels = kitty::paint_rgba(&frame, 64.0, PX, PX);
            retransmit(out, *image, &pixels, self.placed.get(key));
        }
    }

    pub fn wake(&self) -> Option<Duration> {
        self.motion
            .values()
            .filter(|motion| !motion.still())
            .map(|motion| MOTION_FRAME.saturating_sub(motion.last.elapsed()))
            .min()
    }

    /// Incident: a ledger that outlived a resize left tiles unplaced, and one that dropped its ids
    /// left their images on screen; the next sync deletes what was placed, then places again.
    pub fn forget(&mut self) {
        self.forgotten = true;
    }

    pub fn hide_all(&mut self, out: &mut impl Write) {
        for key in self.placed.keys() {
            if let Some(image) = self.ids.get(key) {
                let _ = kitty::delete_id(out, image.raw());
            }
        }
        for key in std::mem::take(&mut self.placed).into_keys() {
            self.ids.remove(&key);
            self.motion.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(state: Option<yi_tui::orb::OrbState>) -> Placement {
        let key = "s-alpha".to_owned();
        let (col, row, cols, rows) = (2, 0, 4, 2);
        Placement {
            col,
            row,
            cols,
            rows,
            seed: key.clone(),
            key,
            accent: (1, 2, 3),
            state,
            art: Art::Identicon,
        }
    }

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
    fn the_gear_is_a_thin_outline_with_a_hub_ring_and_tooth_gaps() {
        let (w, h) = (64, 96);
        let gear = gear_rgba((1, 2, 3), w, h);
        assert_eq!(gear.len(), w * h * 4);
        let alpha = |x: f64, y: f64| {
            let (x, y) = (32.0 + x * 24.0, 48.0 + y * 24.0);
            gear.get((y as usize * w + x as usize) * 4 + 3).copied()
        };
        assert_eq!(alpha(0.0, 0.0), Some(0), "the hub is open");
        assert_eq!(alpha(0.28, 0.0), Some(255), "the hub ring is drawn");
        assert_eq!(alpha(0.5, 0.0), Some(0), "the body is outlined, not filled");
        assert_eq!(alpha(0.97, 0.0), Some(255), "a tooth's tip is drawn");
        assert_eq!(
            alpha(0.79, 0.33),
            Some(0),
            "the gap between two teeth is open"
        );
        assert_eq!(
            alpha(0.0, -1.6),
            Some(0),
            "the rect above the gear is clear"
        );
    }

    #[test]
    fn a_left_cap_rounds_its_outer_corners_and_meets_the_body_flush() {
        let (w, h) = (16, 32);
        let cap = cap_rgba((1, 2, 3), w, h, true);
        let alpha = |x: usize, y: usize| cap.get((y * w + x) * 4 + 3).copied();
        assert_eq!(alpha(0, 0), Some(0), "the outer top corner is cut round");
        assert_eq!(
            alpha(0, h - 1),
            Some(0),
            "the outer bottom corner is cut round"
        );
        assert_eq!(
            alpha(w - 1, 0),
            Some(255),
            "the inner edge is square to meet the body"
        );
        assert_eq!(
            alpha(1, h / 2),
            Some(255),
            "the middle is filled to the outer edge"
        );
        let right = cap_rgba((1, 2, 3), w, h, false);
        assert_eq!(
            right.get(3).copied(),
            Some(255),
            "a right cap's inner edge is the left one"
        );
    }

    /// Kitty images draw above text: an avatar under the open palette showed through it.
    #[test]
    fn a_popup_hides_the_avatars_it_covers_and_keeps_the_rest() {
        let at = |row| Placement {
            col: 2,
            row,
            cols: 2,
            rows: 1,
            key: format!("s-{row}"),
            seed: String::new(),
            accent: (0, 0, 0),
            state: None,
            art: Art::Identicon,
        };
        let popup = ratatui::layout::Rect::new(0, 3, 30, 4);
        let kept = uncovered(vec![at(1), at(4), at(9)], &[popup]);
        let rows: Vec<u16> = kept.iter().map(|place| place.row).collect();
        assert_eq!(rows, vec![1, 9]);
    }

    #[test]
    fn a_forgotten_ledger_deletes_what_it_placed() {
        let place = alpha(None);
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
        let after = String::from_utf8_lossy(&after);
        assert!(after.contains("a=p,i="), "placed again after a clear");
        // Incident: a resize that changed no size drew no clear, and the dropped ids
        // left their images on screen with nothing left to delete them.
        assert!(
            after.contains(&format!("a=d,d=I,i={FIRST_ID},")),
            "the image placed before the forget is deleted: {after:?}"
        );
    }

    /// Sweeps the clock past a frame and returns what one animate pass wrote.
    fn pass(avatars: &mut Avatars) -> Vec<u8> {
        for motion in avatars.motion.values_mut() {
            motion.last = Instant::now()
                .checked_sub(MOTION_FRAME * 2)
                .unwrap_or_else(Instant::now);
        }
        let mut out = Vec::new();
        avatars.animate(&mut out);
        out
    }

    #[test]
    fn a_session_waiting_on_the_person_paints_one_still_frame() {
        let mut avatars = Avatars::default();
        let waiting = alpha(Some(yi_tui::orb::OrbState::Listening));
        avatars.sync(&mut Vec::new(), std::slice::from_ref(&waiting));
        assert!(!pass(&mut avatars).is_empty(), "the first frame is painted");
        assert!(pass(&mut avatars).is_empty(), "a waiting avatar animated");
        assert_eq!(avatars.wake(), None, "a still avatar keeps the loop waking");
        let working = alpha(Some(yi_tui::orb::OrbState::Working));
        avatars.sync(&mut Vec::new(), std::slice::from_ref(&working));
        assert!(!pass(&mut avatars).is_empty());
        assert!(!pass(&mut avatars).is_empty(), "a working avatar stopped");
    }

    #[test]
    fn a_row_that_scrolls_back_in_transmits_again() {
        let place = alpha(None);
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
    fn every_retransmit_of_a_placed_avatar_is_placed_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut place = alpha(Some(yi_tui::orb::OrbState::Thinking));
        let mut avatars = Avatars::default();
        avatars.sync(&mut Vec::new(), std::slice::from_ref(&place));
        let motion = avatars.motion.get_mut("s-alpha").ok_or("no motion")?;
        motion.last = Instant::now().checked_sub(MOTION_FRAME).ok_or("clock")?;
        let mut frame = Vec::new();
        avatars.animate(&mut frame);
        place.state = None;
        let mut settled = Vec::new();
        avatars.sync(&mut settled, std::slice::from_ref(&place));
        for (name, bytes) in [("animation frame", frame), ("settle to identicon", settled)] {
            let written = String::from_utf8_lossy(&bytes);
            let transmit = written
                .rfind(&format!("a=t,i={FIRST_ID},"))
                .ok_or(format!("{name}: no transmit in {written:?}"))?;
            let placed = written.get(transmit..).is_some_and(|tail| {
                tail.contains(&format!("\x1b[1;3H\x1b_Ga=p,i={FIRST_ID},p=1,"))
            });
            assert!(
                placed,
                "{name}: transmit not placed again at row 1 col 3: {written:?}"
            );
        }
        Ok(())
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
