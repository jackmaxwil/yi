//! Anti-aliased rounded rectangles on a straight-alpha RGBA canvas: the chrome a kitty image
//! draws under terminal text, so the text itself stays the terminal's own.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rounded {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

impl Rounded {
    pub fn inset(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            width: (self.width - 2.0 * dx).max(0.0),
            height: (self.height - 2.0 * dy).max(0.0),
            radius: self.radius,
        }
    }

    pub fn shifted(self, dy: f32) -> Self {
        Self {
            y: self.y + dy,
            ..self
        }
    }

    /// Signed distance from a pixel centre to the edge: negative inside.
    fn distance(&self, x: f32, y: f32) -> f32 {
        let (half_w, half_h) = (self.width / 2.0, self.height / 2.0);
        let radius = self.radius.min(half_w).min(half_h).max(0.0);
        let qx = (x - (self.x + half_w)).abs() - (half_w - radius);
        let qy = (y - (self.y + half_h)).abs() - (half_h - radius);
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
    }
}

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height).saturating_mul(4);
        Self {
            width,
            height,
            pixels: vec![0; len],
        }
    }

    pub fn fill(&mut self, shape: Rounded, color: Rgba) {
        self.cover(shape, 1.0, color, |d| (0.5 - d).clamp(0.0, 1.0));
    }

    /// A ring `line` pixels wide just inside the shape's edge.
    pub fn stroke(&mut self, shape: Rounded, line: f32, color: Rgba) {
        self.cover(shape, 1.0, color, |d| {
            (0.5 - d).clamp(0.0, 1.0) - (0.5 - d - line).clamp(0.0, 1.0)
        });
    }

    /// A soft shadow falling `blur` pixels past the edge; nothing inside, where a translucent
    /// panel above it would show it through.
    pub fn shadow(&mut self, shape: Rounded, blur: f32, color: Rgba) {
        self.cover(shape, blur, color, |d| match d > 0.0 {
            true => (1.0 - d / blur).clamp(0.0, 1.0).powi(2),
            false => 0.0,
        });
    }

    fn cover(&mut self, shape: Rounded, reach: f32, color: Rgba, coverage: impl Fn(f32) -> f32) {
        let span = |from: f32, to: f32, limit: usize| {
            let start = (from - reach - 1.0).floor().max(0.0) as usize;
            let end = ((to + reach + 1.0).ceil().max(0.0) as usize).min(limit);
            start..end
        };
        let columns = span(shape.x, shape.x + shape.width, self.width);
        for y in span(shape.y, shape.y + shape.height, self.height) {
            for x in columns.clone() {
                let amount = coverage(shape.distance(x as f32 + 0.5, y as f32 + 0.5));
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
    fn blend(&mut self, at: usize, color: Rgba, coverage: f32) {
        let Some([r, g, b, a]) = self
            .pixels
            .get_mut(at..at.saturating_add(4))
            .and_then(|px| <&mut [u8; 4]>::try_from(px).ok())
        else {
            return;
        };
        let src = f32::from(color.3) / 255.0 * coverage;
        let dst = f32::from(*a) / 255.0;
        let out = src + dst * (1.0 - src);
        if out <= 0.0 {
            return;
        }
        let mix = |s: u8, d: u8| {
            ((f32::from(s) * src + f32::from(d) * dst * (1.0 - src)) / out)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        (*r, *g, *b) = (mix(color.0, *r), mix(color.1, *g), mix(color.2, *b));
        *a = (out * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}
