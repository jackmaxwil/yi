//! Anti-aliased rounded rectangles on a straight-alpha RGBA canvas: the chrome a kitty image
//! draws under terminal text, so the text itself stays the terminal's own.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Rgba(pub(crate) u8, pub(crate) u8, pub(crate) u8, pub(crate) u8);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rounded {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) radius: f64,
}

impl Rounded {
    pub(crate) fn inset(self, dx: f64, dy: f64) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            width: (self.width - 2.0 * dx).max(0.0),
            height: (self.height - 2.0 * dy).max(0.0),
            radius: self.radius,
        }
    }

    pub(crate) fn shifted(self, dy: f64) -> Self {
        Self {
            y: self.y + dy,
            ..self
        }
    }

    fn distance(&self, x: f64, y: f64) -> f64 {
        let (half_w, half_h) = (self.width / 2.0, self.height / 2.0);
        let radius = self.radius.min(half_w).min(half_h).max(0.0);
        let at = (x - (self.x + half_w), y - (self.y + half_h));
        rounded_box(at, (half_w, half_h), radius)
    }
}

/// Signed distance from `(x, y)`, relative to the box's centre, to its rounded edge: negative
/// inside.
pub fn rounded_box((x, y): (f64, f64), (half_w, half_h): (f64, f64), round: f64) -> f64 {
    let (qx, qy) = (x.abs() - half_w + round, y.abs() - half_h + round);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - round
}

pub fn rgb(color: ratatui::style::Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    }
}

/// One cell's size in the terminal's pixels; `None` when the terminal does not report it.
pub fn cell_pixels() -> Option<(usize, usize)> {
    let size = ratatui::crossterm::terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return None;
    }
    Some((
        usize::from(size.width / size.columns),
        usize::from(size.height / size.rows),
    ))
}

#[derive(Clone)]
pub(crate) struct Canvas {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) pixels: Vec<u8>,
    /// Columns and rows a paint may touch; the whole canvas until [`Canvas::restore`].
    clip: [Range<usize>; 2],
}

impl Canvas {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height).saturating_mul(4);
        Self {
            width,
            height,
            pixels: vec![0; len],
            clip: [0..width, 0..height],
        }
    }

    /// Puts `base`'s pixels back inside `rect` and clips every later paint to it.
    pub(crate) fn restore(&mut self, base: &Canvas, [x, y]: [Range<usize>; 2]) {
        let x = x.start.min(self.width)..x.end.min(self.width);
        let y = y.start.min(self.height)..y.end.min(self.height);
        for row in y.clone() {
            let at = |col: usize| {
                row.saturating_mul(self.width)
                    .saturating_add(col)
                    .saturating_mul(4)
            };
            let span = at(x.start)..at(x.end);
            if let (Some(to), Some(from)) =
                (self.pixels.get_mut(span.clone()), base.pixels.get(span))
                && to.len() == from.len()
            {
                to.copy_from_slice(from);
            }
        }
        self.clip = [x, y];
    }

    pub(crate) fn fill(&mut self, shape: Rounded, color: Rgba) {
        self.cover(shape, 1.0, color, |d| (0.5 - d).clamp(0.0, 1.0));
    }

    /// A ring `line` pixels wide just inside the shape's edge.
    pub(crate) fn stroke(&mut self, shape: Rounded, line: f64, color: Rgba) {
        self.cover(shape, 1.0, color, |d| {
            (0.5 - d).clamp(0.0, 1.0) - (0.5 - d - line).clamp(0.0, 1.0)
        });
    }

    /// A soft shadow falling `blur` pixels past the edge; nothing inside, where a translucent
    /// panel above it would show it through.
    pub(crate) fn shadow(&mut self, shape: Rounded, blur: f64, color: Rgba) {
        self.cover(shape, blur, color, |d| match d > 0.0 {
            true => (1.0 - d / blur).clamp(0.0, 1.0).powi(2),
            false => 0.0,
        });
    }

    fn cover(&mut self, shape: Rounded, reach: f64, color: Rgba, coverage: impl Fn(f64) -> f64) {
        let span = |from: f64, to: f64, clip: &Range<usize>| {
            let start = ((from - reach - 1.0).floor().max(0.0) as usize).max(clip.start);
            let end = ((to + reach + 1.0).ceil().max(0.0) as usize).min(clip.end);
            start..end
        };
        let [clip_x, clip_y] = &self.clip;
        let columns = span(shape.x, shape.x + shape.width, clip_x);
        for y in span(shape.y, shape.y + shape.height, clip_y) {
            for x in columns.clone() {
                let amount = coverage(shape.distance(x as f64 + 0.5, y as f64 + 0.5));
                if amount > 0.0 {
                    let at = y
                        .saturating_mul(self.width)
                        .saturating_add(x)
                        .saturating_mul(4);
                    self.blend(at, color, amount);
                }
            }
        }
    }

    /// Source-over in straight alpha, the form kitty's `f=32` reads.
    fn blend(&mut self, at: usize, color: Rgba, coverage: f64) {
        let Some([r, g, b, a]) = self
            .pixels
            .get_mut(at..at.saturating_add(4))
            .and_then(|px| <&mut [u8; 4]>::try_from(px).ok())
        else {
            return;
        };
        let src = f64::from(color.3) / 255.0 * coverage;
        let dst = f64::from(*a) / 255.0;
        let out = src + dst * (1.0 - src);
        if out <= 0.0 {
            return;
        }
        let mix = |s: u8, d: u8| {
            ((f64::from(s) * src + f64::from(d) * dst * (1.0 - src)) / out)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        (*r, *g, *b) = (mix(color.0, *r), mix(color.1, *g), mix(color.2, *b));
        *a = (out * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}
