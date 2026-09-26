use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::core::{Dot, OrbFrame, finalize_frame, radius_scale};
use crate::states::{OrbState, Point, V, add, dot, exit_after, pose, rot_x, smooth, spec};

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

#[derive(Default)]
pub struct Orb {
    now: Option<OrbState>,
    clock: f64,
    next: Option<Next>,
    morph: Option<Morph>,
    pairings: HashMap<Key, Vec<usize>>,
}

enum Job {
    Ready(Vec<usize>),
    Running(JoinHandle<Option<Vec<usize>>>, Arc<AtomicBool>),
    Failed,
}

struct Next {
    to: Option<OrbState>,
    at: f64,
    key: Key,
    job: Job,
}

impl Drop for Next {
    fn drop(&mut self) {
        if let Job::Running(_, stop) = &self.job {
            stop.store(true, Ordering::Relaxed);
        }
    }
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
                Some(mut next) if self.clock + dt >= next.at => {
                    // Invariant: the loop never waits on the pairing; the exit pose holds until it lands.
                    if let Job::Running(handle, _) = &next.job
                        && !handle.is_finished()
                    {
                        self.clock = next.at;
                        self.next = Some(next);
                        return render(&pose(self.now, self.clock));
                    }
                    let from = pose(self.now, next.at);
                    let target = pose(next.to, 0.0);
                    let perm = match std::mem::replace(&mut next.job, Job::Failed) {
                        Job::Ready(perm) => Some(perm),
                        Job::Running(handle, _) => handle.join().ok().flatten(),
                        Job::Failed => None,
                    }
                    .filter(|perm| perm.len() == target.len())
                    .unwrap_or_else(|| matching(&from, &target));
                    self.pairings.insert(next.key, perm.clone());
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
        let job = match self.pairings.get(&key) {
            Some(perm) => Job::Ready(perm.clone()),
            None => {
                let (from, target) = (pose(self.now, at), pose(want, 0.0));
                let stop = Arc::new(AtomicBool::new(false));
                let flag = Arc::clone(&stop);
                std::thread::Builder::new()
                    .name("orb-matching".into())
                    .spawn(move || pairing(&from, &target, &flag))
                    .map_or(Job::Failed, |handle| Job::Running(handle, stop))
            }
        };
        self.next = Some(Next {
            to: want,
            at,
            key,
            job,
        });
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

/// Invariant: in the least-squares pairing (Hungarian method) no two points meet mid-morph;
/// a meeting would make the swapped pairing shorter.
pub fn matching(from: &[Point], target: &[Point]) -> Vec<usize> {
    pairing(from, target, &AtomicBool::new(false))
        .unwrap_or_else(|| (0..from.len().min(target.len())).collect())
}

fn pairing(from: &[Point], target: &[Point], stop: &AtomicBool) -> Option<Vec<usize>> {
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
        if stop.load(Ordering::Relaxed) {
            return None;
        }
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
