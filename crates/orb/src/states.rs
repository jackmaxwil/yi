use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;

use crate::core::fib_dir;

pub const POINTS: usize = 540;
const GHOSTS: usize = 36;
const TILT: f64 = 0.3;
const PHI: f64 = 1.618_033_988_749_895;
const QUARTERS: &[f64] = &[0.0, 0.25, 0.5, 0.75];
const EIGHTHS: &[f64] = &[0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875];

pub type V = [f64; 3];

/// A pose's point in the unit ball: `edge` 0 mid-band, 1 rim, below 0 larger and brighter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub at: V,
    pub edge: f64,
    pub alpha: f64,
    pub ghost: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrbState {
    Awaiting,
    Thinking,
    Composing,
    Working,
    Reading,
    Searching,
    Browsing,
    Editing,
    Executing,
    Computing,
    Planning { done: u8, total: u8 },
    Delegating { children: u8 },
    Listening,
    Stalled,
    Condensing,
    KernelBoot,
}

impl OrbState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Awaiting => "Waiting…",
            Self::Thinking => "Thinking…",
            Self::Composing => "Composing…",
            Self::Working => "Working…",
            Self::Reading => "Reading…",
            Self::Searching => "Searching…",
            Self::Browsing => "Browsing…",
            Self::Editing => "Editing…",
            Self::Executing => "Running…",
            Self::Computing => "Computing…",
            Self::Planning { .. } => "Planning…",
            Self::Delegating { .. } => "Delegating…",
            Self::Listening => "Listening…",
            Self::Stalled => "Retrying…",
            Self::Condensing => "Condensing…",
            Self::KernelBoot => "Starting the kernel…",
        }
    }
}

/// Seconds per loop and the phases a state may be left from; the still mark leaves at once.
pub fn spec(state: Option<OrbState>) -> (f64, &'static [f64]) {
    match state {
        None => (1.0, &[]),
        Some(OrbState::Awaiting) => (5.0, EIGHTHS),
        Some(OrbState::Thinking) => (13.0, EIGHTHS),
        Some(OrbState::Working) => (21.0, EIGHTHS),
        Some(OrbState::Browsing) => (5.0, QUARTERS),
        Some(OrbState::Listening) => (5.6, QUARTERS),
        Some(
            OrbState::Composing
            | OrbState::Reading
            | OrbState::Searching
            | OrbState::Editing
            | OrbState::Executing
            | OrbState::Computing
            | OrbState::Planning { .. }
            | OrbState::Delegating { .. }
            | OrbState::Stalled
            | OrbState::Condensing
            | OrbState::KernelBoot,
        ) => (8.0, QUARTERS),
    }
}

pub(crate) fn exit_after(state: Option<OrbState>, t: f64) -> f64 {
    let (secs, exits) = spec(state);
    let lap = (t / secs).floor();
    if exits.is_empty() {
        return t;
    }
    exits
        .iter()
        .map(|e| {
            let at = (lap + e) * secs;
            if at < t { at + secs } else { at }
        })
        .fold(f64::INFINITY, f64::min)
}

/// Invariant: a loop's end pose is its start: whole cycles, or a symmetry turn or lane swap.
pub fn pose(state: Option<OrbState>, t: f64) -> Vec<Point> {
    let th = (t / spec(state).0).rem_euclid(1.0);
    let points = match state {
        None => mark(),
        Some(OrbState::Awaiting) => awaiting(th),
        Some(OrbState::Thinking) => thinking(th),
        Some(OrbState::Composing) => composing(th),
        Some(OrbState::Working) => working(th),
        Some(OrbState::Reading) => reading(th),
        Some(OrbState::Searching) => searching(th),
        Some(OrbState::Browsing) => browsing(th),
        Some(OrbState::Editing) => editing(th),
        Some(OrbState::Executing) => executing(th),
        Some(OrbState::Computing) => computing(th),
        Some(OrbState::Planning { done, total }) => planning(th, done, total),
        Some(OrbState::Delegating { children }) => delegating(th, children),
        Some(OrbState::Listening) => listening(th),
        Some(OrbState::Stalled) => stalled(th),
        Some(OrbState::Condensing) => condensing(th),
        Some(OrbState::KernelBoot) => kernel_boot(th),
    };
    pad(points)
}

pub(crate) fn add(a: V, b: V, s: f64) -> V {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

pub(crate) fn scale(a: V, s: f64) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn len(a: V) -> f64 {
    dot(a, a).sqrt()
}

fn norm(a: V) -> V {
    scale(a, 1.0 / len(a).max(1e-9))
}

fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn rot_x(v: V, a: f64) -> V {
    let (s, c) = a.sin_cos();
    [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c]
}

fn rot_y(v: V, a: f64) -> V {
    let (s, c) = a.sin_cos();
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}

pub(crate) fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

fn fib(i: usize, n: usize) -> V {
    let d = fib_dir(i as f64, n as f64);
    [d.0, d.1, d.2]
}

fn slerp(a: V, b: V, f: f64) -> V {
    let omega = dot(a, b).clamp(-1.0, 1.0).acos();
    if omega < 1e-4 {
        return a;
    }
    let s = omega.sin();
    add(
        scale(a, ((1.0 - f) * omega).sin() / s),
        b,
        (f * omega).sin() / s,
    )
}

fn body(at: V, edge: f64, alpha: f64) -> Point {
    Point {
        at,
        edge,
        alpha,
        ghost: 0.0,
    }
}

fn ghosts() -> Vec<Point> {
    let mut out = Vec::with_capacity(POINTS);
    out.extend((0..GHOSTS).map(|i| Point {
        at: fib(i, GHOSTS),
        edge: 1.0,
        alpha: 1.0,
        ghost: 1.0,
    }));
    out
}

/// Invariant: a pose has [`POINTS`] points; spares sit invisibly on visible ones and grow out.
fn pad(mut points: Vec<Point>) -> Vec<Point> {
    points.truncate(POINTS);
    let visible = points.len().max(1);
    let mut i = 0;
    while points.len() < POINTS {
        let spare = points.get(i % visible).copied().unwrap_or(Point {
            at: [0.0; 3],
            edge: 1.0,
            alpha: 0.0,
            ghost: 0.0,
        });
        points.push(Point {
            alpha: 0.0,
            ..spare
        });
        i += 1;
    }
    points
}

fn on_band(u: V, v: V, a: f64, off: f64) -> V {
    norm(add(add(scale(u, a.cos()), v, a.sin()), cross(u, v), off))
}

fn mark() -> Vec<Point> {
    const STROKES: [((f64, f64), (f64, f64)); 5] = [
        ((-0.62, 0.62), (-0.28, 0.06)),
        ((0.06, 0.62), (-0.28, 0.06)),
        ((-0.28, 0.06), (-0.28, -0.62)),
        ((0.46, 0.30), (0.46, -0.62)),
        ((0.46, 0.60), (0.46, 0.56)),
    ];
    let span = 0.40 / 0.39;
    let lengths = STROKES.map(|((x1, y1), (x2, y2))| (x2 - x1).hypot(y2 - y1).max(0.02));
    let total: f64 = lengths.iter().sum();
    let mut out = Vec::with_capacity(POINTS);
    for (((x1, y1), (x2, y2)), length) in STROKES.iter().zip(lengths) {
        let count = ((length / total) * 132.0).round().max(2.0) as usize;
        for step in 0..count {
            let f = step as f64 / (count - 1) as f64;
            let at = rot_x(
                [
                    (x1 + (x2 - x1) * f) * span,
                    (y1 + (y2 - y1) * f) * span,
                    0.0,
                ],
                -TILT,
            );
            out.push(body(at, -3.8, 1.0 / 0.7));
        }
    }
    out
}

fn awaiting(th: f64) -> Vec<Point> {
    let mut out = ghosts();
    let front = norm(rot_x([0.0, 0.0, 1.0], -TILT));
    let n = POINTS - GHOSTS;
    for i in 0..n {
        let d = fib(i, n);
        let angle = dot(d, front).clamp(-1.0, 1.0).acos();
        let near = [0.0, 0.5]
            .iter()
            .map(|lag| {
                let ph = (th * 2.0 + lag).fract();
                (-(angle - ph * PI * 0.62).powi(2) * 18.0).exp() * (ph * PI).sin().powi(2)
            })
            .fold(0.0, f64::max);
        out.push(body(
            scale(d, 0.92 + 0.08 * near),
            1.0 - near,
            0.3 + 0.7 * near,
        ));
    }
    out
}

fn thinking(th: f64) -> Vec<Point> {
    const REGIONS: [V; 8] = [
        [-0.45, 0.55, 0.7],
        [-0.5, 0.8, -0.35],
        [-0.35, 0.3, -0.9],
        [-0.9, -0.35, 0.15],
        [0.45, 0.55, 0.7],
        [0.5, 0.8, -0.35],
        [0.35, 0.3, -0.9],
        [0.9, -0.35, 0.15],
    ];
    const TOUR: [usize; 8] = [0, 4, 5, 1, 2, 6, 7, 3];
    const SLOT: [f64; 8] = [0.0, 3.0, 4.0, 7.0, 1.0, 2.0, 5.0, 6.0];
    let regions = REGIONS.map(norm);
    let beats = th * 8.0;
    let hop = beats.floor() as usize;
    let ph = beats.fract();
    let region = |k: usize| {
        let r = TOUR.get(k % 8).copied().unwrap_or(0);
        regions.get(r).copied().unwrap_or([0.0, 0.0, 1.0])
    };
    let packet = slerp(region(hop), region(hop + 1), smooth((ph - 0.3) / 0.55));
    let carried = ((ph - 0.25) / 0.1).clamp(0.0, 1.0) * ((0.9 - ph) / 0.1).clamp(0.0, 1.0);
    let glow = SLOT.map(|slot| {
        let age = (beats - (slot - 1.0 + 0.85)).rem_euclid(8.0);
        (age / 0.25).min(1.0) * (-age * 0.75).exp()
    });
    (0..POINTS)
        .map(|i| {
            let d = fib(i, POINTS);
            let near = |c: V, w: f64| (-(len(add(d, c, -1.0)) / w).powi(2)).exp();
            let lit_region = regions
                .iter()
                .zip(glow)
                .map(|(c, g)| near(*c, 0.5) * g)
                .fold(0.0, f64::max);
            let pk = near(packet, 0.17) * carried;
            let groove = (-(d[0] / 0.1).powi(2)).exp();
            let ridge = (d[1] * 9.0 + (d[2] * 3.0).sin() * 1.4 + d[0].signum() * 0.8).sin();
            let crest = 0.5 + 0.5 * ridge;
            let at = scale(
                [d[0] * 0.78, d[1] * 0.7, d[2] * 0.98],
                1.0 + 0.03 * ridge - 0.05 * groove,
            );
            let lit = lit_region.max(pk);
            body(
                rot_x(rot_y(at, PI), 1.0),
                0.75 - 0.15 * crest + 0.25 * groove - 2.0 * pk - 0.9 * lit_region,
                (0.36 + 0.08 * crest + 0.56 * lit) * (1.0 - 0.85 * groove),
            )
        })
        .collect()
}

fn composing(th: f64) -> Vec<Point> {
    let mut out = ghosts();
    let w = th * TAU;
    let (u, v) = ([1.0, 0.0, 0.0], [0.0, 0.55f64.cos(), 0.55f64.sin()]);
    for lane_i in 0..12u8 {
        let lane = (f64::from(lane_i) - 5.5) / 5.5;
        for k in 0..42u8 {
            let a = f64::from(k) / 42.0 * TAU;
            let wave = 0.16 * (3.0 * a - 5.0 * w + 0.22 * f64::from(lane_i)).sin()
                + 0.07 * (5.0 * a + 3.0 * w).sin();
            out.push(body(on_band(u, v, a, lane * 0.41 + wave), lane.abs(), 1.0));
        }
    }
    out
}

fn working(th: f64) -> Vec<Point> {
    let mut out = ghosts();
    let w = th * TAU;
    let tilt = 0.5 + 0.3 * w.sin();
    let u = rot_y([1.0, 0.0, 0.0], -w);
    let v = rot_y([0.0, tilt.cos(), tilt.sin()], -w);
    let width = 0.58 * (0.85 + 0.15 * (3.0 * w).sin());
    for lane_i in 0..14u8 {
        let lane = (f64::from(lane_i) - 6.5) / 6.5;
        let stagger = f64::from(lane_i.min(13 - lane_i) % 2) * 0.5;
        for k in 0..36u8 {
            let a = (f64::from(k) + stagger) / 36.0 * TAU + w;
            let off = lane * width * (a / 2.0 - 2.0 * w).cos() + 0.08 * (2.0 * a - 3.0 * w).sin();
            out.push(body(on_band(u, v, a, off), lane.abs(), 1.0));
        }
    }
    out
}

fn reading(th: f64) -> Vec<Point> {
    let mut out = ghosts();
    let lats: Vec<f64> = (0..11u8)
        .map(|k| ((f64::from(k) + 0.5) / 11.0 * 2.0 - 1.0) * 1.3)
        .collect();
    let weight: f64 = lats.iter().map(|l| l.cos()).sum();
    let budget = POINTS - GHOSTS;
    let mut counts: Vec<usize> = lats
        .iter()
        .map(|l| (l.cos() / weight * budget as f64) as usize)
        .collect();
    let short = budget.saturating_sub(counts.iter().sum());
    counts.iter_mut().take(short).for_each(|c| *c += 1);
    let scan = 1.05 * (th * TAU).cos();
    for (lat, count) in lats.iter().zip(counts) {
        let y = -lat.sin();
        let near = (-(y - scan).powi(2) * 30.0).exp();
        for i in 0..count {
            let lon = i as f64 / count as f64 * TAU;
            out.push(body(
                [lat.cos() * lon.cos(), y, lat.cos() * lon.sin()],
                1.0 - near,
                0.45 + 0.55 * near,
            ));
        }
    }
    out
}

fn searching(th: f64) -> Vec<Point> {
    let mut out = ghosts();
    let (arms, per, reach) = (6u8, 42u8, 1.38);
    for arm in 0..arms {
        let turn = (f64::from(arm) + th) / f64::from(arms);
        // Invariant: a beam rides its arm's angle, so arms turning into each other carry beams.
        let head = (th * 3.0 + turn).rem_euclid(1.0);
        for i in 0..per {
            let f = f64::from(i) / f64::from(per);
            let lat = (f * 2.0 - 1.0) * reach;
            let behind = (head - f).rem_euclid(1.0);
            let lit = (-(behind / 0.05).powi(2))
                .exp()
                .max((-((1.0 - behind) / 0.035).powi(2)).exp())
                .max((-behind * 7.0).exp() * 0.55);
            let taper = 1.0 - (lat.abs() / reach).powi(4);
            for lane in [-0.055, 0.055] {
                let lon = 0.9 * (PI / 4.0 + lat / 2.0).tan().ln() + turn * TAU + lane;
                out.push(body(
                    [lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin()],
                    1.0 - lit,
                    taper * (0.3 + 0.7 * lit),
                ));
            }
        }
    }
    out
}

fn browsing(th: f64) -> Vec<Point> {
    let (petals, core) = (8.0, 70);
    let shell = POINTS - core;
    let ph = th * 5.0;
    let mut open_sum = 0.0;
    let mut out: Vec<Point> = (0..shell)
        .map(|i| {
            let d = fib(i, shell);
            let petal = (d[2].atan2(d[0]).rem_euclid(TAU) / TAU * petals).floor();
            let mid = (petal + 0.5) / petals * TAU;
            let q = ph - petal / petals * 0.8;
            let open = if q < 0.6 {
                smooth(q / 0.6)
            } else if q < 1.3 {
                1.0
            } else {
                1.0 - smooth((q - 1.3) / 0.6)
            };
            open_sum += open;
            body(
                add(scale(d, 0.76), [mid.cos(), 0.0, mid.sin()], 0.28 * open),
                0.3,
                0.9,
            )
        })
        .collect();
    let open = open_sum / shell as f64;
    out.extend((0..core).map(|i| {
        body(
            scale(fib(i, core), 0.34 * (0.7 + 0.3 * open)),
            1.0 - 1.8 * open,
            0.2 + 0.8 * open,
        )
    }));
    out
}

fn trefoil() -> &'static [(f64, V, V)] {
    static SAMPLES: OnceLock<Vec<(f64, V, V)>> = OnceLock::new();
    SAMPLES.get_or_init(|| {
        let curve = |f: f64| {
            let rr = 0.58 + 0.36 * (3.0 * f).cos();
            [
                rr * (2.0 * f).cos(),
                0.36 * (3.0 * f).sin(),
                rr * (2.0 * f).sin(),
            ]
        };
        let fine: Vec<V> = (0..=4000)
            .map(|i| curve(f64::from(i) / 4000.0 * TAU))
            .collect();
        let mut run = vec![0.0];
        for pair in fine.windows(2) {
            if let [a, b] = pair {
                run.push(run.last().copied().unwrap_or(0.0) + len(add(*b, *a, -1.0)));
            }
        }
        let total = run.last().copied().unwrap_or(1.0).max(1e-9);
        let at = |target: f64| {
            let j = run
                .partition_point(|x| *x < target)
                .saturating_sub(1)
                .min(fine.len().saturating_sub(2));
            let (a, b) = (
                fine.get(j).copied().unwrap_or([0.0; 3]),
                fine.get(j + 1).copied().unwrap_or([0.0; 3]),
            );
            let (ra, rb) = (
                run.get(j).copied().unwrap_or(0.0),
                run.get(j + 1).copied().unwrap_or(1.0),
            );
            add(
                a,
                add(b, a, -1.0),
                ((target - ra) / (rb - ra).max(1e-12)).clamp(0.0, 1.0),
            )
        };
        (0..168)
            .map(|i| {
                let u = f64::from(i) / 168.0;
                let (p, q) = (at(u * total), at(((u + 1.0 / 168.0) % 1.0) * total));
                let tangent = norm(add(q, p, -1.0));
                let radial = norm([p[0], 0.0, p[2]]);
                (u, p, norm(cross(tangent, cross(radial, tangent))))
            })
            .collect()
    })
}

fn editing(th: f64) -> Vec<Point> {
    let mut out = ghosts();
    let pen = (th * 2.0).fract();
    for &(u, p, side) in trefoil() {
        let behind = (pen - u).rem_euclid(1.0);
        let lit = (1.0 - behind)
            .powi(5)
            .max((-((1.0 - behind) / 0.012).powi(2)).exp());
        for lane in [-1.0f64, 0.0, 1.0] {
            let at = rot_x(rot_y(add(p, side, lane * 0.06), 0.3), 1.15);
            out.push(body(
                at,
                (1.0 - lit).max(lane.abs() * 0.4),
                0.35 + 0.65 * lit,
            ));
        }
    }
    out
}

fn executing(th: f64) -> Vec<Point> {
    let mut out = ghosts();
    let per = (POINTS - GHOSTS) / 3;
    for (radius, laps, tilt, yaw) in [
        (0.98, 2.0, 0.35, 0.0),
        (0.86, -3.0, 1.2, 1.1),
        (0.74, 4.0, 1.2, -1.1),
    ] {
        let pos = (th * laps).rem_euclid(1.0);
        for i in 0..per {
            let (k, track) = (i / 3, i % 3);
            let a = (k as f64 + track as f64 / 3.0) / (per / 3) as f64 * TAU;
            let x = (a / TAU * 2.0).rem_euclid(1.0);
            let behind = if laps > 0.0 { pos - x } else { x - pos }.rem_euclid(1.0);
            let lit = (-behind * 9.0)
                .exp()
                .max((-(1.0 - behind).powi(2) * 900.0).exp());
            let at = rot_y(
                rot_x(
                    [
                        radius * a.cos(),
                        (track as f64 - 1.0) * 0.065,
                        radius * a.sin(),
                    ],
                    tilt,
                ),
                yaw,
            );
            out.push(body(at, 1.0 - lit, 0.3 + 0.7 * lit));
        }
    }
    out
}

fn computing(th: f64) -> Vec<Point> {
    let half = 0.8 / 3f64.sqrt();
    let g = |c: usize| (c as f64 / 5.0 * 2.0 - 1.0) * half;
    let cycle = th * 2.0;
    let ph = cycle.fract();
    let open = smooth(ph / 0.3) * (1.0 - smooth((ph - 0.6) / 0.3));
    let spin = (smooth((ph - 0.3) / 0.3) + cycle.floor()) * PI / 2.0;
    (0..216)
        .map(|i| {
            let v = [g(i / 36), g(i / 6 % 6), g(i % 6)];
            let centre = v.map(|c| c.signum() * half * 0.6);
            let at = add(
                scale(centre, 1.0 + 0.9 * open),
                rot_y(add(v, centre, -1.0), spin),
                1.0,
            );
            body(
                rot_x(rot_y(at, th * PI / 2.0 + 0.6), 0.45),
                0.3 - 0.3 * open,
                0.8 + 0.2 * open,
            )
        })
        .collect()
}

fn planning(th: f64, done: u8, total: u8) -> Vec<Point> {
    let layers = f64::from(total.clamp(1, 9));
    let done = f64::from(done.min(total)) * layers / f64::from(total.max(1));
    (0..POINTS)
        .map(|i| {
            let d = fib(i, POINTS);
            let layer = (((d[1] + 1.0) / 2.0 * layers).floor()).min(layers - 1.0);
            let lon = d[2].atan2(d[0]).rem_euclid(TAU) / TAU;
            let x = (lon - th).rem_euclid(1.0);
            let sweep = (-(x.min(1.0 - x) / 0.08).powi(2)).exp();
            let lit = if layer < done {
                1.0
            } else if layer == done {
                0.4 + 0.6 * sweep
            } else {
                0.0
            };
            let alpha = if layer <= done {
                0.35 + 0.65 * lit
            } else {
                0.2
            };
            body(scale(d, 0.95), 0.9 - 0.9 * lit, alpha)
        })
        .collect()
}

fn delegating(th: f64, children: u8) -> Vec<Point> {
    let mut out = ghosts();
    let kids = usize::from(children.min(8));
    let budget = POINTS - GHOSTS;
    let share = budget / (kids + 2);
    let parent = budget - share * kids;
    let rp = (parent as f64 / budget as f64).cbrt();
    let rc = (share as f64 / budget as f64).cbrt();
    let orbit = if kids == 0 { 0.0 } else { rp + rc + 0.12 };
    let fit = 0.98 / if kids == 0 { rp } else { (orbit + rc).max(rp) };
    out.extend((0..parent).map(|i| body(scale(fib(i, parent), rp * fit), 0.2, 1.0)));
    for m in 0..kids {
        let phase = (m as f64 + th) / kids as f64 * TAU;
        let centre = scale(
            rot_x([phase.cos(), phase.sin(), 0.3 * phase.sin()], -TILT),
            orbit * fit,
        );
        out.extend((0..share).map(|i| body(add(centre, fib(i, share), rc * fit), 0.0, 1.0)));
    }
    out
}

fn listening(th: f64) -> Vec<Point> {
    let inhale = 1.0 / (PHI * PHI);
    let breath = if th < inhale {
        smooth(th / inhale)
    } else {
        1.0 - smooth((th - inhale) / (1.0 - inhale))
    };
    let low = (1.0 / PHI).sqrt();
    let radius = 0.95 * (low + (1.0 - low) * breath);
    (0..POINTS)
        .map(|i| {
            body(
                scale(fib(i, POINTS), radius),
                0.45 - 0.3 * breath,
                0.7 + 0.3 * breath,
            )
        })
        .collect()
}

fn stalled(th: f64) -> Vec<Point> {
    const SAND: usize = 233;
    const GLASS: usize = 89;
    let flip = |at: V| [-at[0], -at[1], at[2]];
    let golden = PI * (3.0 - 5f64.sqrt());
    let cone = |y: f64| 0.05 + 0.62 * y / 0.92;
    let mut out = Vec::with_capacity(POINTS);
    for j in 0..GLASS {
        let y = 0.04 + 0.88 * (j as f64 + 0.5) / GLASS as f64;
        let a = j as f64 * golden;
        let rim = [cone(y) * a.cos(), y, cone(y) * a.sin()];
        out.push(body(rim, 0.7, 0.5));
        out.push(body(flip(rim), 0.7, 0.5));
    }
    let drain = 1.0 / PHI;
    for k in 0..SAND {
        let rank = (k as f64 + 0.5) / SAND as f64;
        let y = 0.05 + 0.5 * rank.cbrt();
        let r = cone(y) * 0.9 * ((k * 89 % SAND) as f64 / SAND as f64).sqrt();
        let a = k as f64 * golden;
        let slot = [r * a.cos(), y, r * a.sin()];
        let start = (1.0 - rank) * drain * 0.8;
        let f = ((th - start) / (drain * 0.2)).clamp(0.0, 1.0);
        let at = if f < 0.4 {
            scale(slot, 1.0 - smooth(f / 0.4))
        } else {
            scale(flip(slot), smooth((f - 0.4) / 0.6))
        };
        out.push(body(at, -0.6, 1.0));
    }
    let turn = smooth((th - 0.7) / 0.25) * PI;
    let (sin, cos) = turn.sin_cos();
    out.into_iter()
        .map(|point| {
            let [x, y, z] = point.at;
            Point {
                at: [x * cos - y * sin, x * sin + y * cos, z],
                ..point
            }
        })
        .collect()
}

fn condensing(th: f64) -> Vec<Point> {
    let pull = if th < 0.5 {
        smooth(th / 0.5)
    } else {
        1.0 - smooth((th - 1.0 / PHI) / (1.0 - 1.0 / PHI))
    };
    let mut out = ghosts();
    let n = POINTS - GHOSTS;
    for i in 0..n {
        let d = fib(i, n);
        let radius = 0.95 * (1.0 - pull * (1.0 - 1.0 / (PHI * PHI)));
        let twist = pull * (PI / PHI) * (1.0 - d[1].abs());
        let kept = i % 2 == 0;
        out.push(body(
            scale(rot_y(d, twist), radius),
            0.4 - pull,
            if kept {
                0.75 + 0.25 * pull
            } else {
                0.75 - 0.45 * pull
            },
        ));
    }
    out
}

fn kernel_boot(th: f64) -> Vec<Point> {
    let half = 0.8 / 3f64.sqrt();
    let g = |c: usize| (c as f64 / 5.0 * 2.0 - 1.0) * half;
    let level = 6.0 * smooth(th * PHI);
    let rest = 1.0 - smooth((th - 0.8) / 0.2);
    (0..216)
        .map(|i| {
            let v = [g(i / 36), g(i / 6 % 6), g(i % 6)];
            let layer = (i / 6 % 6) as f64;
            let lit = smooth(level - layer) * rest;
            body(
                rot_x(rot_y(v, 0.6), 0.45),
                0.3 - 0.5 * lit,
                0.45 + 0.55 * lit,
            )
        })
        .collect()
}
