use std::thread::JoinHandle;

use crate::core::{Dot, OrbFrame, finalize_frame, radius_scale};
use crate::states::{OrbState, Point, V, add, dot, exit_after, pose, rot_x, smooth};

const CANVAS: f64 = 64.0;
const RADIUS: f64 = CANVAS / 2.0 * 0.78;
const TILT: f64 = 0.3;
const MORPH: f64 = 0.5;
/// Seconds a request waits at least, so the matching can run on its own thread first.
const LEAD: f64 = 0.1;
/// Incident: a surface that stops repainting hands its next frame an unbounded gap, and one
/// unclamped step consumed a whole animation. The orb's clock advances at most this per frame.
const MAX_STEP: f64 = 0.1;

#[derive(Default)]
pub struct Orb {
    now: Option<OrbState>,
    clock: f64,
    next: Option<Next>,
    morph: Option<Morph>,
}

struct Next {
    to: Option<OrbState>,
    at: f64,
    job: Option<JoinHandle<Vec<usize>>>,
}

struct Morph {
    to: Option<OrbState>,
    from: Vec<Point>,
    target: Vec<Point>,
    perm: Vec<usize>,
    t: f64,
}

impl Orb {
    pub fn at_rest(&self, want: Option<OrbState>) -> bool {
        self.now.is_none() && want.is_none() && self.next.is_none() && self.morph.is_none()
    }

    pub fn frame(&mut self, dt: f64, want: Option<OrbState>) -> OrbFrame {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, MAX_STEP)
        } else {
            0.0
        };
        if let Some(morph) = &mut self.morph {
            morph.t += dt;
            if morph.t < MORPH {
                return render(&blend(
                    &morph.from,
                    &morph.target,
                    &morph.perm,
                    morph.t / MORPH,
                ));
            }
            self.now = morph.to;
            self.clock = morph.t - MORPH;
            self.morph = None;
        } else {
            self.plan(want);
            match self.next.take() {
                Some(next) if self.clock + dt >= next.at => {
                    let from = pose(self.now, next.at);
                    let target = pose(next.to, 0.0);
                    let perm = next
                        .job
                        .and_then(|job| job.join().ok())
                        .filter(|perm| perm.len() == target.len())
                        .unwrap_or_else(|| matching(&from, &target));
                    let t = self.clock + dt - next.at;
                    let frame = render(&blend(&from, &target, &perm, t / MORPH));
                    self.morph = Some(Morph {
                        to: next.to,
                        from,
                        target,
                        perm,
                        t,
                    });
                    return frame;
                }
                pending => {
                    self.next = pending;
                    self.clock += dt;
                }
            }
        }
        render(&pose(self.now, self.clock))
    }

    fn plan(&mut self, want: Option<OrbState>) {
        if want == self.now {
            self.next = None;
            return;
        }
        if self.next.as_ref().is_some_and(|next| next.to == want) {
            return;
        }
        let at = exit_after(self.now, self.clock + LEAD);
        let (from, target) = (pose(self.now, at), pose(want, 0.0));
        let job = std::thread::Builder::new()
            .name("orb-matching".into())
            .spawn(move || matching(&from, &target))
            .ok();
        self.next = Some(Next { to: want, at, job });
    }
}

pub fn render(points: &[Point]) -> OrbFrame {
    let rs = radius_scale(CANVAS, 0.6);
    let centre = CANVAS / 2.0;
    let dots = points
        .iter()
        .map(|point| {
            let at = rot_x(point.at, TILT);
            let depth = ((at[2] + 1.0) / 2.0).clamp(0.0, 1.0);
            let mix = |body: f64, ghost: f64| body + (ghost - body) * point.ghost;
            Dot {
                x: centre + at[0] * RADIUS,
                y: centre - at[1] * RADIUS,
                z: at[2],
                r: mix(
                    (0.935 + 1.445 * depth) * (1.0 - 0.25 * point.edge) * rs,
                    0.8 * rs,
                ),
                white: mix(0.52 - 0.44 * depth + 0.18 * point.edge, 0.78),
                a: mix(0.4 + 0.6 * depth, 0.1 + 0.22 * depth) * point.alpha,
            }
        })
        .collect();
    finalize_frame(dots, Vec::new(), 0.3)
}

pub fn blend(from: &[Point], target: &[Point], perm: &[usize], f: f64) -> Vec<Point> {
    let f = smooth(f);
    let lerp = |a: f64, b: f64| a + (b - a) * f;
    target
        .iter()
        .zip(perm)
        .map(|(b, &j)| {
            let a = from.get(j).copied().unwrap_or(*b);
            Point {
                at: add(a.at, add(b.at, a.at, -1.0), f),
                edge: lerp(a.edge, b.edge),
                alpha: lerp(a.alpha, b.alpha),
                ghost: lerp(a.ghost, b.ghost),
            }
        })
        .collect()
}

/// Invariant: straight paths of the least-squares pairing (Hungarian method) never cross;
/// swapping a crossing pair would shorten the total.
pub fn matching(from: &[Point], target: &[Point]) -> Vec<usize> {
    let n = from.len().min(target.len());
    let identity: Vec<usize> = (0..n).collect();
    let cost: Vec<f64> = target
        .iter()
        .take(n)
        .flat_map(|b| from.iter().take(n).map(move |a| gap(a.at, b.at)))
        .collect();
    if cost.iter().any(|c| !c.is_finite()) {
        return identity;
    }
    let mut u = vec![0.0; n + 1];
    let mut v = vec![0.0; n + 1];
    let mut owner = vec![0usize; n + 1];
    let mut way = vec![0usize; n + 1];
    for row in 1..=n {
        owner[0] = row;
        let mut col = 0;
        let mut least = vec![f64::INFINITY; n + 1];
        let mut used = vec![false; n + 1];
        loop {
            used[col] = true;
            let r = owner[col];
            let (mut delta, mut next) = (f64::INFINITY, 0);
            for j in 1..=n {
                if used[j] {
                    continue;
                }
                let reduced = cost[(r - 1) * n + j - 1] - u[r] - v[j];
                if reduced < least[j] {
                    least[j] = reduced;
                    way[j] = col;
                }
                if least[j] < delta {
                    delta = least[j];
                    next = j;
                }
            }
            if next == 0 {
                return identity;
            }
            for j in 0..=n {
                if used[j] {
                    u[owner[j]] += delta;
                    v[j] -= delta;
                } else {
                    least[j] -= delta;
                }
            }
            col = next;
            if owner[col] == 0 {
                break;
            }
        }
        while col != 0 {
            let prev = way[col];
            owner[col] = owner[prev];
            col = prev;
        }
    }
    let mut perm = identity;
    for (j, &r) in owner.iter().enumerate().skip(1) {
        if let Some(slot) = perm.get_mut(r.wrapping_sub(1)) {
            *slot = j - 1;
        }
    }
    perm
}

fn gap(a: V, b: V) -> f64 {
    let d = add(a, b, -1.0);
    dot(d, d)
}
