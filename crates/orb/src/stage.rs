use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use crate::core::{Dot, OrbFrame, finalize_frame, radius_scale};
use crate::states::{
    MARK_SPAN, OrbState, Point, V, add, dot, exit_after, pose, rot_x, smooth, spec,
};

const CANVAS: f64 = 64.0;
const RADIUS: f64 = CANVAS / 2.0 * 0.78;
const TILT: f64 = 0.3;
const MORPH: f64 = 0.5;
/// Seconds a request waits at least, so the matching can run on its own thread first.
const LEAD: f64 = 0.1;
/// Incident: a surface that stops repainting hands its next frame an unbounded gap, and one
/// unclamped step consumed a whole animation. The orb's clock advances at most this per frame.
const MAX_STEP: f64 = 0.1;

type Key = (Option<OrbState>, u64, Option<OrbState>);
/// Empty while the matching thread solves; `None` inside means no pairing exists for the poses.
type Pairing = Arc<OnceLock<Option<Vec<usize>>>>;

/// Invariant: the lock covers a lookup or an insert, never a solve; the matching thread
/// publishes through its cell, so no avatar waits on another's pairing and none solves it twice.
static PAIRINGS: OnceLock<Mutex<HashMap<Key, Pairing>>> = OnceLock::new();

fn pairings() -> MutexGuard<'static, HashMap<Key, Pairing>> {
    PAIRINGS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

static SOLVED: AtomicUsize = AtomicUsize::new(0);

/// Pairings solved in this process, on a matching thread or the frame loop's fallback.
pub fn pairings_solved() -> usize {
    SOLVED.load(Ordering::Relaxed)
}

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
    job: Option<Pairing>,
}

struct Morph {
    to: Option<OrbState>,
    from: Vec<Point>,
    target: Vec<Point>,
    perm: Vec<usize>,
    t: f64,
}

impl Orb {
    pub fn showing(state: Option<OrbState>) -> Self {
        Self {
            now: state,
            ..Self::default()
        }
    }

    pub fn at_rest(&self, want: Option<OrbState>) -> bool {
        self.now.is_none() && want.is_none() && self.next.is_none() && self.morph.is_none()
    }

    pub fn holding(&self, state: OrbState) -> bool {
        self.now == Some(state) && self.next.is_none() && self.morph.is_none()
    }

    pub fn frame(&mut self, dt: Duration, want: Option<OrbState>) -> OrbFrame {
        let dt = dt.as_secs_f64().min(MAX_STEP);
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
                    // Invariant: the loop never waits on the pairing; the exit pose holds until it lands.
                    if next.job.as_ref().is_some_and(|cell| cell.get().is_none()) {
                        self.clock = next.at;
                        self.next = Some(next);
                        return render(&pose(self.now, self.clock));
                    }
                    let from = pose(self.now, next.at);
                    let target = pose(next.to, 0.0);
                    let perm = next
                        .job
                        .and_then(|cell| cell.get().cloned().flatten())
                        .filter(|perm| perm.len() == target.len())
                        .unwrap_or_else(|| {
                            SOLVED.fetch_add(1, Ordering::Relaxed);
                            matching(&from, &target)
                        });
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
        let (secs, exits) = spec(self.now);
        let phase = if exits.is_empty() {
            0
        } else {
            ((at / secs).rem_euclid(1.0) * 1e6).round() as u64
        };
        let key = (self.now, phase, want);
        let mut table = pairings();
        let job = match table.get(&key) {
            Some(cell) => Some(Arc::clone(cell)),
            None => {
                let (now, cell) = (self.now, Arc::new(OnceLock::new()));
                let out = Arc::clone(&cell);
                std::thread::Builder::new()
                    .name("orb-matching".into())
                    .spawn(move || {
                        let _ = out.set(pairing(&pose(now, at), &pose(want, 0.0)));
                    })
                    .ok()
                    .map(|_| {
                        SOLVED.fetch_add(1, Ordering::Relaxed);
                        table.insert(key, Arc::clone(&cell));
                        cell
                    })
            }
        };
        drop(table);
        self.next = Some(Next { to: want, at, job });
    }
}

/// Where a [`crate::states::MARK_STROKES`] point lands in the rendered image, as fractions of its
/// width and height.
pub fn mark_at(x: f64, y: f64) -> (f64, f64) {
    let reach = |v: f64| v * MARK_SPAN * RADIUS;
    (
        (CANVAS / 2.0 + reach(x)) / CANVAS,
        (CANVAS / 2.0 - reach(y)) / CANVAS,
    )
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
    finalize_frame(dots, 0.3)
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

/// Invariant: in the least-squares pairing (Hungarian method) no two points meet mid-morph;
/// a meeting would make the swapped pairing shorter.
pub fn matching(from: &[Point], target: &[Point]) -> Vec<usize> {
    pairing(from, target).unwrap_or_else(|| (0..from.len().min(target.len())).collect())
}

fn pairing(from: &[Point], target: &[Point]) -> Option<Vec<usize>> {
    let n = from.len().min(target.len());
    let cost: Vec<f64> = target
        .iter()
        .take(n)
        .flat_map(|b| from.iter().take(n).map(move |a| gap(a.at, b.at)))
        .collect();
    if cost.iter().any(|c| !c.is_finite()) {
        return None;
    }
    let (mut u, mut v) = (vec![0.0; n + 1], vec![0.0; n + 1]);
    let (mut owner, mut way) = (vec![0usize; n + 1], vec![0usize; n + 1]);
    let (mut least, mut used) = (vec![f64::INFINITY; n + 1], vec![false; n + 1]);
    for row in 1..=n {
        owner[0] = row;
        least.fill(f64::INFINITY);
        used.fill(false);
        let mut col = 0;
        loop {
            used[col] = true;
            let r = owner[col];
            let base = r.checked_sub(1)?.checked_mul(n)?;
            let costs = cost.get(base..base + n)?;
            let ur = u[r];
            let (mut delta, mut next) = (f64::INFINITY, 0);
            let lanes = costs
                .iter()
                .zip(v.get(1..)?)
                .zip(least.get_mut(1..)?)
                .zip(way.get_mut(1..)?)
                .zip(used.get(1..)?);
            for (j, ((((&c, &vj), lj), wj), &taken)) in lanes.enumerate() {
                if taken {
                    continue;
                }
                let reduced = c - ur - vj;
                if reduced < *lj {
                    *lj = reduced;
                    *wj = col;
                }
                if *lj < delta {
                    delta = *lj;
                    next = j + 1;
                }
            }
            if next == 0 {
                return None;
            }
            for (j, (&taken, lj)) in used.iter().zip(least.iter_mut()).enumerate() {
                if taken {
                    u[owner[j]] += delta;
                    v[j] -= delta;
                } else {
                    *lj -= delta;
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
    let mut perm: Vec<usize> = (0..n).collect();
    for (j, &r) in owner.iter().enumerate().skip(1) {
        if let Some(slot) = perm.get_mut(r.wrapping_sub(1)) {
            *slot = j - 1;
        }
    }
    Some(perm)
}

fn gap(a: V, b: V) -> f64 {
    let d = add(a, b, -1.0);
    dot(d, d)
}
