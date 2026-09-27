// Engine core: shared primitives for the dotted 3D thought-orbs, depth-shaded and z-sorted,
// depth carried by dot size and ink.

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

/// Dots z-sorted into draw order and radius-clamped.
#[derive(Debug, Clone, Default)]
pub struct OrbFrame {
    pub dots: Vec<Dot>,
}

/// Stable directions on a unit sphere (Fibonacci lattice).
pub fn fib_dir(i: f64, n: f64) -> (f64, f64, f64) {
    let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let y = 1.0 - (2.0 * (i + 0.5)) / n;
    let rad = (1.0 - y * y).sqrt();
    let a = i * golden;
    (rad * a.cos(), y, rad * a.sin())
}

/// Drop invisible dots, clamp radii to the floor, z-sort far→near.
pub fn finalize_frame(dots: Vec<Dot>, r_min: f64) -> OrbFrame {
    let mut visible: Vec<Dot> = dots
        .into_iter()
        .filter(|d| d.a >= 0.02)
        .map(|mut d| {
            d.r = d.r.max(r_min);
            d
        })
        .collect();
    visible.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
    OrbFrame { dots: visible }
}

/// Dot radii were tuned for a 300pt frame; sub-linear scaling keeps small
/// spinners legible.
pub fn radius_scale(size: f64, pow: f64) -> f64 {
    (size / 300.0).powf(pow)
}
