// thinking-orbs `engine/core.ts`, port verbatim (A.13): shared primitives
// for the dotted 3D thought-orbs — rotated, depth-shaded, z-sorted; depth
// carried by dot size and ink weight alone.

#[derive(Debug, Clone, Copy)]
pub struct Dot {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub r: f64,
    /// Ink value: 0 = darkest ink on paper. Mirrored on dark themes.
    pub white: f64,
    pub a: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct OrbLine {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub white: f64,
    pub a: f64,
    pub w: f64,
}

/// One rendered instant: dots z-sorted into draw order, radius-clamped;
/// lines draw first. Nothing needs further interpretation.
#[derive(Debug, Clone, Default)]
pub struct OrbFrame {
    pub dots: Vec<Dot>,
    pub lines: Vec<OrbLine>,
}

pub struct Proj {
    st: f64,
    ct: f64,
    sy: f64,
    cyw: f64,
    cx: f64,
    cy: f64,
    scale: f64,
}

impl Proj {
    pub fn point(&self, x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        let x1 = x * self.cyw + z * self.sy;
        let z1 = -x * self.sy + z * self.cyw;
        let y1 = y * self.ct - z1 * self.st;
        let z2 = y * self.st + z1 * self.ct;
        (self.cx + x1 * self.scale, self.cy - y1 * self.scale, z2)
    }
}

pub fn lerp(a: f64, b: f64, f: f64) -> f64 {
    a + (b - a) * f
}

pub fn frac(x: f64) -> f64 {
    x - x.floor()
}

/// Deterministic hash in [0, 1).
pub fn hash_d(a: f64, b: f64) -> f64 {
    let h = (a * 12.9898 + b * 78.233).sin() * 43758.5453;
    h - h.floor()
}

/// Value noise on a 2D lattice — smooth, deterministic, cheap.
pub fn vnoise(x: f64, y: f64) -> f64 {
    let xi = x.floor();
    let yi = y.floor();
    let mut fx = x - xi;
    let mut fy = y - yi;
    fx = fx * fx * (3.0 - 2.0 * fx);
    fy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash_d(xi, yi);
    let b = hash_d(xi + 1.0, yi);
    let c = hash_d(xi, yi + 1.0);
    let d = hash_d(xi + 1.0, yi + 1.0);
    a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy
}

/// Stable directions on a unit sphere (Fibonacci lattice).
pub fn fib_dir(i: f64, n: f64) -> (f64, f64, f64) {
    let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let y = 1.0 - (2.0 * (i + 0.5)) / n;
    let rad = (1.0 - y * y).sqrt();
    let a = i * golden;
    (rad * a.cos(), y, rad * a.sin())
}

/// Shortest signed angular distance, wrapped to (-π, π].
pub fn angle_delta(a: f64, b: f64) -> f64 {
    (a - b).sin().atan2((a - b).cos())
}

/// Shared spin + tilt + orthographic projection.
pub fn make_proj(yaw: f64, tilt: f64, cx: f64, cy: f64, scale: f64) -> Proj {
    Proj {
        st: tilt.sin(),
        ct: tilt.cos(),
        sy: yaw.sin(),
        cyw: yaw.cos(),
        cx,
        cy,
        scale,
    }
}

/// Drop invisible marks, clamp radii to the mode floor, z-sort far→near.
pub fn finalize_frame(dots: Vec<Dot>, lines: Vec<OrbLine>, r_min: f64) -> OrbFrame {
    let mut visible: Vec<Dot> = dots
        .into_iter()
        .filter(|d| d.a >= 0.02)
        .map(|mut d| {
            d.r = d.r.max(r_min);
            d
        })
        .collect();
    visible.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
    OrbFrame {
        dots: visible,
        lines: lines.into_iter().filter(|l| l.a >= 0.02).collect(),
    }
}

/// Dot radii were tuned for a 300pt frame; sub-linear scaling keeps small
/// spinners legible.
pub fn radius_scale(size: f64, pow: f64) -> f64 {
    (size / 300.0).powf(pow)
}
