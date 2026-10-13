//! The title field: a scalar over a pane's free rows, sampled into glyph cells; a pure function
//! of their size and one elapsed time, so a frozen [`crate::motion::Clock`] replays any frame.

use std::f64::consts::TAU;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::app::App;
use crate::colors::Theme;

const RAMP: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
const TOP: u8 = 9;
/// Half a stroke's width, in units of half the rows' height.
const THICK: f64 = 0.06;
const SCALE: f64 = 0.95;
const RING_FREQ: f64 = 4.0;
/// Seconds a ring takes to reach the place of the next: the field's exact loop.
pub const RING_SECS: f64 = 1.8;
pub const FRAME: Duration = Duration::from_millis(50);

fn strokes() -> [([f64; 2], [f64; 2]); 5] {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for &((x1, y1), (x2, y2)) in &yi_orb::MARK_STROKES {
        lo = [lo[0].min(x1).min(x2), lo[1].min(y1).min(y2)];
        hi = [hi[0].max(x1).max(x2), hi[1].max(y1).max(y2)];
    }
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let place = |x: f64, y: f64| [(x - cx) * SCALE, (y - cy) * SCALE];
    yi_orb::MARK_STROKES.map(|((x1, y1), (x2, y2))| (place(x1, y1), place(x2, y2)))
}

fn to_segment(u: f64, v: f64, a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((u - a[0]) * dx + (v - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (u - a[0] - t * dx).hypot(v - a[1] - t * dy)
}

/// Signed distance to the `Yi` strokes, negative inside one, with the mark centred on the
/// origin; `u` and `v` run along the width and up the height in units of half the height.
pub fn distance(u: f64, v: f64) -> f64 {
    let near = strokes()
        .iter()
        .map(|&(a, b)| to_segment(u, v, a, b))
        .fold(f64::INFINITY, f64::min);
    near - THICK
}

fn echo(d: f64, seconds: f64) -> f64 {
    if d < 0.0 {
        return 1.0;
    }
    let ring = 0.5 + 0.5 * (TAU * (d * RING_FREQ - seconds / RING_SECS)).cos();
    ring * ring * (-0.9 * d).exp() * (1.0 - 0.85 * (-9.0 * d).exp())
}

/// A cell is twice as tall as wide, so a row spans two units of `v` where a column spans one
/// of `u`, both over the height.
fn at(x: u16, y: u16, width: u16, height: u16) -> (f64, f64) {
    let h = f64::from(height);
    let u = (f64::from(x) + 0.5 - f64::from(width) / 2.0) / h;
    let v = (h - 2.0 * f64::from(y) - 1.0) / h;
    (u, v)
}

fn style(step: usize, theme: &Theme) -> Style {
    match step {
        0..=4 => theme.dim_style(),
        5..=7 => theme.accent_style(),
        _ => theme.accent_style().add_modifier(Modifier::BOLD),
    }
}

/// Draw the field over `area`; a cell whose value rounds to empty keeps what it held.
pub fn paint(buffer: &mut Buffer, area: Rect, elapsed: Duration, theme: &Theme) {
    let seconds = elapsed.as_secs_f64() % RING_SECS;
    for y in 0..area.height {
        for x in 0..area.width {
            let (u, v) = at(x, y, area.width, area.height);
            let value = echo(distance(u, v), seconds).clamp(0.0, 1.0);
            let step = usize::from((value * f64::from(TOP)).round() as u8);
            let cell = (area.x.saturating_add(x), area.y.saturating_add(y));
            if let (Some(cell), Some(glyph)) = (buffer.cell_mut(cell), RAMP.get(step))
                && step > 0
            {
                cell.set_char(*glyph).set_style(style(step, theme));
            }
        }
    }
}

pub(crate) fn paint_below(app: &App, buffer: &mut Buffer, area: Rect, used: u16) {
    if app.title_visible() {
        let top = area.top().saturating_add(used).min(area.bottom());
        let free = Rect::new(
            area.left(),
            top,
            area.width,
            area.bottom().saturating_sub(top),
        );
        paint(buffer, free, app.clock.since(app.started_at), &app.theme);
    }
}

impl App {
    pub fn set_title(&mut self, on: bool) {
        self.title = on;
    }

    /// An empty, idle session is the title screen; the first prompt ends it.
    pub fn title_visible(&self) -> bool {
        self.title && self.history.is_empty() && !self.running
    }

    pub fn title_wake(&self) -> Option<Duration> {
        let into = self.clock.since(self.started_at).as_millis() % FRAME.as_millis();
        let into = Duration::from_millis(u64::try_from(into).unwrap_or(0));
        self.title_visible()
            .then(|| FRAME.saturating_sub(into).max(Duration::from_millis(1)))
    }
}
