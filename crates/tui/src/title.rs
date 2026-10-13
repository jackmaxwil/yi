//! The title animation: a scalar over a whole pane, sampled into glyph cells, a pure function of
//! size, scene and seconds since the console started. The name lands on the orb's own mark.

use std::f64::consts::{PI, TAU};
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::app::App;
use crate::colors::Theme;

mod sim;

pub use sim::Cache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    Kali,
    Echo,
    Solid,
    Mobius,
    Clouds,
    Growth,
}

impl Scene {
    pub const ALL: [(&'static str, Scene); 6] = [
        ("kali", Scene::Kali),
        ("echo", Scene::Echo),
        ("solid", Scene::Solid),
        ("mobius", Scene::Mobius),
        ("clouds", Scene::Clouds),
        ("growth", Scene::Growth),
    ];

    pub fn named(name: &str) -> Option<Scene> {
        Self::ALL
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, scene)| *scene)
    }
}

/// Used when the config names no weights: the Kali fold eight parts in twelve.
pub const DEFAULT_WEIGHTS: [(Scene, u32); 6] = [
    (Scene::Kali, 8),
    (Scene::Echo, 2),
    (Scene::Solid, 2),
    (Scene::Mobius, 2),
    (Scene::Clouds, 2),
    (Scene::Growth, 2),
];

/// One weighted draw; the same seed picks the same scene, and no weight at all picks none.
pub fn pick(weights: &[(Scene, u32)], seed: u64) -> Option<Scene> {
    let total = weights.iter().fold(0_u64, |sum, (_, weight)| {
        sum.saturating_add(u64::from(*weight))
    });
    let mut left = mix(seed).checked_rem(total)?;
    for &(scene, weight) in weights {
        let weight = u64::from(weight);
        if left < weight {
            return Some(scene);
        }
        left -= weight;
    }
    None
}

/// The splitmix64 finalizer: neighbouring seeds, such as clock nanoseconds, land far apart.
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const RAMP: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
const TOP: u8 = 9;
/// Half a stroke's width, in units of half the rows' height.
const THICK: f64 = 0.06;
const SCALE: f64 = 0.95;
const RING_FREQ: f64 = 4.0;
/// Seconds a ring takes to reach the place of the next: the echo's exact loop.
pub const RING_SECS: f64 = 1.8;
const KALI_SECS: f64 = 16.0;
/// The fold opens from a deep zoom over this long, then the name rises over the next second.
const OPEN_SECS: f64 = 1.6;
const NAME_FROM: f64 = 1.2;
const NAME_TO: f64 = 1.9;
/// The name holds in full until here, then lands on the small mark over `LAND_SECS`.
const HOLD_SECS: f64 = 2.2;
const LAND_SECS: f64 = 0.8;
/// After this the animation is over and draws nothing.
pub const OVER: Duration = Duration::from_millis(3000);
const KALI_FOLDS: u8 = 11;
const KALI_SAMPLES: u8 = 3;
/// The fold's accumulated change of length at the 5th and 97th percentile over a loop.
const KALI_LO: f64 = 0.56;
const KALI_HI: f64 = 4.5;
pub const FRAME: Duration = Duration::from_millis(50);

fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

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

type Strokes = [([f64; 2], [f64; 2]); 5];

fn distance_to(strokes: &Strokes, thick: f64, u: f64, v: f64) -> f64 {
    let near = strokes
        .iter()
        .map(|&(a, b)| to_segment(u, v, a, b))
        .fold(f64::INFINITY, f64::min);
    near - thick
}

/// Signed distance to the `Yi` strokes, negative inside one, with the mark centred on the
/// origin; `u` and `v` run along the width and up the height in units of half the height.
pub fn distance(u: f64, v: f64) -> f64 {
    distance_to(&strokes(), THICK, u, v)
}

/// The strokes `leave` of the way from the centre onto the small mark in `landing`, and their
/// half-width: a column's half at the end, so the shrunk name is still a line of glyphs.
fn landed(area: Rect, landing: Option<Rect>, leave: f64) -> (Strokes, f64) {
    let centred = strokes();
    let Some(land) = landing else {
        return (centred, THICK);
    };
    let h = f64::from(area.height);
    let at = |(x, y): (f64, f64)| {
        let col = f64::from(land.x) + x * f64::from(land.width);
        let row = f64::from(land.y) + y * f64::from(land.height);
        [
            (col - f64::from(area.x) - f64::from(area.width) / 2.0) / h,
            (h - 2.0 * (row - f64::from(area.y))) / h,
        ]
    };
    let mark = |x: f64, y: f64| at(yi_orb::mark_at(x, y));
    let mut out = centred;
    for (stroke, &((x1, y1), (x2, y2))) in out.iter_mut().zip(&yi_orb::MARK_STROKES) {
        let (a, b) = (mark(x1, y1), mark(x2, y2));
        let lerp = |from: [f64; 2], to: [f64; 2]| {
            [
                from[0] + (to[0] - from[0]) * leave,
                from[1] + (to[1] - from[1]) * leave,
            ]
        };
        *stroke = (lerp(stroke.0, a), lerp(stroke.1, b));
    }
    (out, THICK + (0.5 / h - THICK) * leave)
}

fn echo(d: f64, t: f64) -> f64 {
    let ring = 0.5 + 0.5 * (TAU * (d.max(0.0) * RING_FREQ - (t % RING_SECS) / RING_SECS)).cos();
    ring * ring * (-0.9 * d.max(0.0)).exp()
}

fn fold(mut x: f64, mut y: f64, cx: f64, cy: f64) -> f64 {
    let mut prev = x.hypot(y);
    let mut total = 0.0;
    for _ in 0..KALI_FOLDS {
        let r2 = x * x + y * y + 1e-9;
        x = x.abs() / r2 - cx;
        y = y.abs() / r2 - cy;
        let len = x.hypot(y);
        total += (len - prev).abs();
        prev = len;
    }
    total / f64::from(KALI_FOLDS)
}

/// The Kali fold at `(u, v)`, each cell averaged over `du` by `dv` of samples: the plane turns
/// once a loop while `c` circles and the scale breathes; the first seconds zoom out of the fold.
fn kali(u: f64, v: f64, (du, dv): (f64, f64), t: f64) -> f64 {
    let a = TAU * t / KALI_SECS;
    let (sin, cos) = a.sin_cos();
    let (cx, cy) = (0.80 + 0.03 * sin, 0.55 + 0.03 * cos);
    let scale = 0.5 * (1.0 + 0.18 * sin) * (0.3 + 0.7 * smooth(t / OPEN_SECS));
    let n = f64::from(KALI_SAMPLES);
    let mut sum = 0.0;
    for j in 0..KALI_SAMPLES {
        for i in 0..KALI_SAMPLES {
            let su = u + ((f64::from(i) + 0.5) / n - 0.5) * du;
            let sv = v + ((f64::from(j) + 0.5) / n - 0.5) * dv;
            sum += fold(
                (su * cos - sv * sin) * scale,
                (su * sin + sv * cos) * scale,
                cx,
                cy,
            );
        }
    }
    let raw = sum / (n * n);
    ((raw - KALI_LO) / (KALI_HI - KALI_LO))
        .clamp(0.0, 1.0)
        .powf(1.1)
        * smooth(t / 0.3)
}

fn lattice(x: i32, y: i32) -> f64 {
    let n = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    let n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    f64::from((n ^ (n >> 16)) & 0xFFFF) / 65_535.0
}

/// Value noise, smoothly interpolated between lattice points.
fn noise(x: f64, y: f64) -> f64 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (ix as i32, iy as i32);
    let top = lattice(ix, iy) + (lattice(ix + 1, iy) - lattice(ix, iy)) * sx;
    let bottom = lattice(ix, iy + 1) + (lattice(ix + 1, iy + 1) - lattice(ix, iy + 1)) * sx;
    top + (bottom - top) * sy
}

fn fbm(mut x: f64, mut y: f64) -> f64 {
    let (mut sum, mut amp) = (0.0, 0.5);
    for _ in 0..3 {
        sum += amp * noise(x, y);
        (x, y, amp) = (x * 2.02 + 3.1, y * 2.02 + 1.7, amp * 0.5);
    }
    sum
}

/// Domain-warped noise: weather that drifts a lap every six seconds, in `0.0..=1.0`.
fn clouds(u: f64, v: f64, t: f64) -> f64 {
    let (sin, cos) = (TAU * t / 6.0).sin_cos();
    let (x, y) = (u * 1.7, v * 1.7);
    let qx = fbm(x + 0.4 * cos, y + 0.4 * sin);
    let qy = fbm(x + 5.2, y + 1.3 + 0.4 * sin);
    (fbm(x + 3.2 * qx, y + 3.2 * qy) * 1.6 - 0.2).clamp(0.0, 1.0) * smooth(t / 0.3)
}

/// Where the plane is sampled for the name: a loxodromic Möbius flow with fixed points at `u = ±1`
/// that unwinds to the identity by 2 s, so the name comes out of a vortex and settles.
fn mobius(u: f64, v: f64, t: f64) -> (f64, f64) {
    let s = 1.7 * (1.0 - smooth(t / 2.0));
    if s < 1e-3 {
        return (u, v);
    }
    let (re, im) = (s * 0.9, s * 1.6);
    let lam = (re.exp() * im.cos(), re.exp() * im.sin());
    let div = |a: (f64, f64), b: (f64, f64)| {
        let n = (b.0 * b.0 + b.1 * b.1).max(1e-12);
        ((a.0 * b.0 + a.1 * b.1) / n, (a.1 * b.0 - a.0 * b.1) / n)
    };
    let q = div(div((u + 1.0, v), (u - 1.0, v)), lam);
    div((-1.0 - q.0, -q.1), (1.0 - q.0, -q.1))
}

/// Rings of distance that drift outward, solid inside the name: the vortex's own texture.
fn rings(d: f64, t: f64) -> f64 {
    let d = d.max(0.0);
    let ring = 0.5 + 0.5 * (TAU * (d * 5.0 - 0.3 * t)).cos();
    ring.powi(4) * (-1.3 * d).exp()
}

/// Cells farther than this from every stroke cannot see the extruded name.
const SOLID_NEAR: f64 = 0.45;

/// The name as a lit extrusion, sphere-traced from a camera that swings once while it holds;
/// `None` where the ray misses. It shrinks onto the small mark with the same strokes.
fn solid(strokes: &Strokes, thick: f64, (u, v): (f64, f64), t: f64, leave: f64) -> Option<f64> {
    let half_z = 0.17 * smooth((t - 0.3) / 0.9) * (1.0 - 0.85 * leave);
    if half_z < 0.01 {
        return None;
    }
    let fat = 0.05 * (1.0 - leave);
    let (sin, cos) = (0.55 * (PI * t / HOLD_SECS).sin() * (1.0 - leave)).sin_cos();
    let f =
        |x: f64, y: f64, z: f64| (distance_to(strokes, thick, x, y) - fat).max(z.abs() - half_z);
    let eye = [-3.0 * sin, 0.0, -3.0 * cos];
    let aim = [u * cos + 3.0 * sin, v, -u * sin + 3.0 * cos];
    let len = (aim[0] * aim[0] + aim[1] * aim[1] + aim[2] * aim[2]).sqrt();
    let dir = aim.map(|c| c / len);
    let mut at = 0.0;
    for _ in 0..40 {
        let p = [
            eye[0] + dir[0] * at,
            eye[1] + dir[1] * at,
            eye[2] + dir[2] * at,
        ];
        let d = f(p[0], p[1], p[2]);
        if d < 0.006 {
            let e = 0.012;
            let n = [
                f(p[0] + e, p[1], p[2]) - f(p[0] - e, p[1], p[2]),
                f(p[0], p[1] + e, p[2]) - f(p[0], p[1] - e, p[2]),
                f(p[0], p[1], p[2] + e) - f(p[0], p[1], p[2] - e),
            ];
            let size = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
            let lum = (n[0] * -0.45 + n[1] * 0.65 + n[2] * -0.6) / size;
            return Some(0.30 + 0.70 * lum.max(0.0));
        }
        at += d * 0.9;
        if at > 6.0 {
            return None;
        }
    }
    None
}

/// Dim the field near the strokes and fill them: `rise` 0 leaves the field, 1 is the name.
fn name(field: f64, d: f64, rise: f64) -> f64 {
    if d < 0.0 {
        field + (1.0 - field) * rise
    } else {
        field * (1.0 - rise * 0.85 * (-9.0 * d).exp())
    }
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

/// A cell's seed in `0.0..1.0`, fixed per place: which cells give the screen back first.
fn dissolve(x: u16, y: u16) -> f64 {
    let h = (u32::from(x).wrapping_mul(73_856_093) ^ u32::from(y).wrapping_mul(19_349_663))
        .wrapping_mul(2_654_435_761);
    f64::from(h >> 8) / 16_777_216.0
}

/// Cover every cell of `area` with `scene` until [`OVER`]. From `HOLD_SECS` the name shrinks onto
/// `landing` and the cells around it give the screen back a few at a time.
pub fn cover(
    buffer: &mut Buffer,
    area: Rect,
    (scene, elapsed): (Scene, Duration),
    landing: Option<Rect>,
    theme: &Theme,
    cache: &mut Cache,
) {
    if elapsed >= OVER {
        return;
    }
    let t = elapsed.as_secs_f64();
    let h = f64::from(area.height);
    let cell = (1.0 / h, 2.0 / h);
    let rise = match scene {
        Scene::Kali | Scene::Clouds | Scene::Growth => {
            smooth((t - NAME_FROM) / (NAME_TO - NAME_FROM))
        }
        Scene::Echo | Scene::Mobius => 1.0,
        Scene::Solid => 0.0,
    };
    let leave = smooth((t - HOLD_SECS) / LAND_SECS);
    let fade = 1.0 - smooth(leave * 1.25);
    let reveal = smooth((leave - 0.1) / 0.9);
    let (strokes, thick) = landed(area, landing, leave);
    let coral =
        (scene == Scene::Growth && fade > 0.0).then(|| cache.growth((area.width, area.height), t));
    for y in 0..area.height {
        for x in 0..area.width {
            let (u, v) = at(x, y, area.width, area.height);
            let (wu, wv) = match scene {
                Scene::Mobius => mobius(u, v, t),
                _ => (u, v),
            };
            let d = distance_to(&strokes, thick, wu, wv);
            let solid_hit = match scene {
                Scene::Solid if d < SOLID_NEAR => solid(&strokes, thick, (u, v), t, leave),
                _ => None,
            };
            let in_name = match scene {
                Scene::Solid => solid_hit.is_some(),
                _ => d < 0.0,
            };
            if !in_name && dissolve(x, y) < reveal {
                continue;
            }
            let field = match scene {
                _ if fade <= 0.0 => 0.0,
                Scene::Kali => kali(u, v, cell, t) * fade,
                Scene::Echo => echo(d, t) * fade,
                Scene::Solid => echo(d.max(0.1), t) * fade * 0.6,
                Scene::Mobius => rings(d, t) * fade,
                Scene::Clouds => clouds(u, v, t) * fade,
                Scene::Growth => coral.map_or(0.0, |coral| coral.at(u, v)) * fade,
            };
            let value = match solid_hit {
                Some(lit) => lit,
                None => name(field, d, rise),
            }
            .clamp(0.0, 1.0);
            let step = usize::from((value * f64::from(TOP)).round() as u8);
            let place = (area.x.saturating_add(x), area.y.saturating_add(y));
            if let Some(target) = buffer.cell_mut(place) {
                target.reset();
                if let (Some(glyph), true) = (RAMP.get(step), step > 0) {
                    target.set_char(*glyph).set_style(style(step, theme));
                }
            }
        }
    }
}

impl App {
    /// While the title covers the screen the orb's image stays unplaced: one logo at a time.
    pub fn set_title_cover(&mut self, on: bool) {
        self.title_cover = on;
    }

    /// Nothing sent and nothing running: the session the title may sit over.
    pub fn title_idle(&self) -> bool {
        self.history.is_empty() && !self.running
    }

    /// The small mark's cells as the last paint laid them out, whether or not it is placed.
    pub fn orb_rect(&self) -> Option<Rect> {
        let (col, row) = self.orb_slot?;
        Some(Rect::new(
            col,
            row,
            crate::app::ORB_COLS,
            crate::app::ORB_ROWS,
        ))
    }
}
