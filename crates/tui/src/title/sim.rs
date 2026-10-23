//! The title scenes that remember: a simulation advanced to the second asked for and kept between
//! frames. A second that runs backwards, or a new screen size, starts it again from nothing.

use super::distance;

/// Steps a second: the coral takes 1500 of them by the time the name lands.
const GROWTH_STEPS_PER_SEC: f64 = 500.0;
const GROWTH_COLS: usize = 80;
const GROWTH_ROWS: usize = 32;

/// What the stateful scenes keep between frames; one per console, empty until a scene asks.
#[derive(Default)]
pub struct Cache {
    growth: Option<Growth>,
}

/// Gray–Scott coral seeded on the strokes, with the feed and kill rates the lab settled on.
pub(super) struct Growth {
    size: (u16, u16),
    steps: u32,
    u: Vec<f32>,
    v: Vec<f32>,
}

impl Growth {
    fn new(size: (u16, u16)) -> Self {
        let aspect = f64::from(size.0) / f64::from(size.1.max(1));
        let mut growth = Self {
            size,
            steps: 0,
            u: vec![1.0; GROWTH_COLS * GROWTH_ROWS],
            v: vec![0.0; GROWTH_COLS * GROWTH_ROWS],
        };
        for j in 0..GROWTH_ROWS {
            for i in 0..GROWTH_COLS {
                let x = ((i as f64 + 0.5) / GROWTH_COLS as f64 - 0.5) * aspect;
                let y = 1.0 - 2.0 * (j as f64 + 0.5) / GROWTH_ROWS as f64;
                if distance(x, y) < 0.02 {
                    growth.u[j * GROWTH_COLS + i] = 0.5;
                    growth.v[j * GROWTH_COLS + i] = 0.25;
                }
            }
        }
        growth
    }

    fn step(&mut self) {
        const DU: f32 = 0.2097;
        const DV: f32 = 0.105;
        const FEED: f32 = 0.0545;
        const KILL: f32 = 0.062;
        let (u, v) = (self.u.clone(), self.v.clone());
        for j in 1..GROWTH_ROWS - 1 {
            for i in 1..GROWTH_COLS - 1 {
                let at = j * GROWTH_COLS + i;
                let lap = |g: &[f32]| {
                    g[at - 1] + g[at + 1] + g[at - GROWTH_COLS] + g[at + GROWTH_COLS] - 4.0 * g[at]
                };
                let react = u[at] * v[at] * v[at];
                self.u[at] = u[at] + DU * lap(&u) - react + FEED * (1.0 - u[at]);
                self.v[at] = v[at] + DV * lap(&v) + react - (FEED + KILL) * v[at];
            }
        }
        self.steps += 1;
    }

    /// The coral's concentration at plane point `(u, v)`, in `0.0..=1.0`.
    pub(super) fn at(&self, u: f64, v: f64) -> f64 {
        let aspect = f64::from(self.size.0) / f64::from(self.size.1.max(1));
        let col = ((u / aspect + 0.5) * GROWTH_COLS as f64).floor();
        let row = ((1.0 - v) / 2.0 * GROWTH_ROWS as f64).floor();
        let inside =
            (0.0..GROWTH_COLS as f64).contains(&col) && (0.0..GROWTH_ROWS as f64).contains(&row);
        if !inside {
            return 0.0;
        }
        let cell = row as usize * GROWTH_COLS + col as usize;
        f64::from((self.v[cell] * 3.2).clamp(0.0, 1.0))
    }
}

impl Cache {
    /// The coral `t` seconds in on a screen of `size`, advanced from where the last frame left it.
    pub(super) fn growth(&mut self, size: (u16, u16), t: f64) -> &Growth {
        let want = (t * GROWTH_STEPS_PER_SEC) as u32;
        let fresh = self
            .growth
            .take()
            .filter(|g| g.size == size && g.steps <= want);
        let growth = self
            .growth
            .insert(fresh.unwrap_or_else(|| Growth::new(size)));
        while growth.steps < want {
            growth.step();
        }
        growth
    }
}
