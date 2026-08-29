use yi_orb::core::Dot;
use yi_orb::{OrbFrame, OrbState, evaluate};

/// The mark and the activity indicator are one object (U34). Only the dot
/// positions interpolate — the orb engine owns every frame the working state
/// renders, so the morph adds no second renderer.
pub const LOGO_COLS: u16 = 8;
pub const LOGO_ROWS: u16 = 4;
pub const MORPH_MS: u64 = 520;
pub const FRAME_MS: u64 = 33;

/// Incident: a settled mark stops the frame timer, so the gap since the last
/// paint is unbounded. Turning it straight into progress let the first animating
/// frame consume the whole morph; one frame is the most a frame can advance.
pub fn advance(phase: f64, target: f64, elapsed: std::time::Duration) -> f64 {
    let max = FRAME_MS as f64 / MORPH_MS as f64;
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let step = (elapsed_ms / MORPH_MS as f64).min(max);
    if target > phase {
        (phase + step).min(target)
    } else {
        (phase - step).max(target)
    }
}

const DOTS: usize = 132;

/// Strokes of `Y` and `i` in a centred unit square, y up. Sampled by arc
/// length so the dot spacing is even across strokes of different lengths.
const STROKES: [((f64, f64), (f64, f64)); 5] = [
    ((-0.62, 0.62), (-0.28, 0.06)),
    ((0.06, 0.62), (-0.28, 0.06)),
    ((-0.28, 0.06), (-0.28, -0.62)),
    ((0.46, 0.30), (0.46, -0.62)),
    ((0.46, 0.60), (0.46, 0.56)),
];

fn stroke_dots(size: f64) -> Vec<Dot> {
    let lengths: Vec<f64> = STROKES
        .iter()
        .map(|((x1, y1), (x2, y2))| (x2 - x1).hypot(y2 - y1).max(0.02))
        .collect();
    let total: f64 = lengths.iter().sum();
    let centre = size / 2.0;
    let span = size * 0.40;
    let radius = size * 0.020;
    let mut dots = Vec::with_capacity(DOTS);
    for (stroke, length) in STROKES.iter().zip(&lengths) {
        let ((x1, y1), (x2, y2)) = *stroke;
        let count = ((length / total) * DOTS as f64).round().max(2.0) as usize;
        for step in 0..count {
            let f = if count > 1 {
                step as f64 / (count - 1) as f64
            } else {
                0.0
            };
            dots.push(Dot {
                x: centre + (x1 + (x2 - x1) * f) * span,
                y: centre - (y1 + (y2 - y1) * f) * span,
                z: 0.0,
                r: radius,
                white: 0.04,
                a: 1.0,
            });
        }
    }
    dots
}

fn by_angle(dots: &mut [Dot], centre: f64) {
    dots.sort_by(|a, b| {
        let key = |d: &Dot| (d.y - centre).atan2(d.x - centre);
        key(a).total_cmp(&key(b))
    });
}

fn resample(dots: &[Dot], count: usize) -> Vec<Dot> {
    if dots.is_empty() || count == 0 {
        return Vec::new();
    }
    (0..count)
        .filter_map(|i| {
            let index = i.saturating_mul(dots.len()) / count;
            dots.get(index).copied()
        })
        .collect()
}

/// The target is the orb's own live frame the whole way, never an intermediate
/// preset. Pairing decides rearrangement or noise: both clouds order by angle,
/// and the orb ordering rotates to whichever offset minimises total travel.
pub fn frame(phase: f64, clock: f64, size: u32) -> Option<OrbFrame> {
    let phase = phase.clamp(0.0, 1.0);
    let canvas = f64::from(size);
    let mut mark = stroke_dots(canvas);
    if phase <= f64::EPSILON {
        return Some(OrbFrame {
            dots: mark,
            lines: Vec::new(),
        });
    }
    let orb = evaluate(OrbState::Working, size, clock)?;
    if phase >= 1.0 {
        return Some(orb);
    }
    let count = mark.len().min(orb.dots.len());
    let mut live = resample(&orb.dots, count);
    mark.truncate(count);
    let centre = canvas / 2.0;
    by_angle(&mut mark, centre);
    by_angle(&mut live, centre);
    rotate_to_shortest(&mark, &mut live);
    // Exponential ease-in (user-directed): the mark barely stirs at first and
    // then accelerates into the orb, so the eye reads a launch rather than a
    // constant slide.
    let ease = (10.0 * phase - 10.0).exp2();
    let dots = mark
        .iter()
        .zip(&live)
        .map(|(from, to)| Dot {
            x: from.x + (to.x - from.x) * ease,
            y: from.y + (to.y - from.y) * ease,
            z: from.z + (to.z - from.z) * ease,
            r: from.r + (to.r - from.r) * ease,
            white: from.white + (to.white - from.white) * ease,
            a: from.a + (to.a - from.a) * ease,
        })
        .collect();
    Some(OrbFrame {
        dots,
        lines: Vec::new(),
    })
}

fn rotate_to_shortest(mark: &[Dot], live: &mut [Dot]) {
    let n = live.len();
    if n == 0 {
        return;
    }
    let cost = |offset: usize| -> f64 {
        mark.iter()
            .enumerate()
            .map(|(i, from)| {
                live.get((i + offset) % n)
                    .map_or(0.0, |to| (to.x - from.x).powi(2) + (to.y - from.y).powi(2))
            })
            .sum()
    };
    let best = (0..n)
        .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))
        .unwrap_or(0);
    live.rotate_left(best);
}
