//! The title scenes that remember: a simulation advanced to the second asked for and kept between
//! frames. A second that runs backwards, or a new screen size, starts it again from nothing.

use super::{distance, lattice};

/// Steps a second: the coral takes 1500 of them by the time the name lands.
const GROWTH_STEPS_PER_SEC: f64 = 500.0;
const GROWTH_COLS: usize = 80;
const GROWTH_ROWS: usize = 32;
const SPIRAL_STATES: u8 = 12;
const SPIRAL_STEPS_PER_SEC: f64 = 55.0;
const SPIRAL_WARMUP: u32 = 70;

/// What the stateful scenes keep between frames; one per console, empty until a scene asks.
#[derive(Default)]
pub struct Cache {
    growth: Option<Growth>,
    spirals: Option<Spirals>,
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
        grid_cell(self.size, (GROWTH_COLS, GROWTH_ROWS), u, v)
            .map_or(0.0, |at| f64::from((self.v[at] * 3.2).clamp(0.0, 1.0)))
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

/// The grid cell under plane point `(u, v)` on a screen of `size`, none outside the screen.
fn grid_cell(size: (u16, u16), (cols, rows): (usize, usize), u: f64, v: f64) -> Option<usize> {
    let aspect = f64::from(size.0) / f64::from(size.1.max(1));
    let col = ((u / aspect + 0.5) * cols as f64).floor();
    let row = ((1.0 - v) / 2.0 * rows as f64).floor();
    let inside = (0.0..cols as f64).contains(&col) && (0.0..rows as f64).contains(&row);
    inside.then(|| row as usize * cols + col as usize)
}

/// Cyclic cellular automaton on a torus: a cell takes the next of twelve states when a neighbour
/// already holds it, and spirals organise out of the noise it starts as.
pub(super) struct Spirals {
    size: (u16, u16),
    grid: (usize, usize),
    steps: u32,
    cells: Vec<u8>,
}

impl Spirals {
    fn new(size: (u16, u16)) -> Self {
        let grid = (
            usize::from(size.0.clamp(1, 120)),
            usize::from(size.1.clamp(1, 50)),
        );
        let cells = (0..grid.0 * grid.1)
            .map(|at| {
                let (x, y) = ((at % grid.0) as i32, (at / grid.0) as i32);
                (lattice(x, y + 90) * f64::from(SPIRAL_STATES)) as u8 % SPIRAL_STATES
            })
            .collect();
        let mut spirals = Self {
            size,
            grid,
            steps: 0,
            cells,
        };
        for _ in 0..SPIRAL_WARMUP {
            spirals.step();
        }
        spirals.steps = 0;
        spirals
    }

    fn step(&mut self) {
        let (cols, rows) = self.grid;
        let old = self.cells.clone();
        for y in 0..rows {
            for x in 0..cols {
                let next = (old[y * cols + x] + 1) % SPIRAL_STATES;
                let taken = (0..9).any(|k| {
                    let at = ((y + rows + k / 3 - 1) % rows) * cols + (x + cols + k % 3 - 1) % cols;
                    k != 4 && old[at] == next
                });
                if taken {
                    self.cells[y * cols + x] = next;
                }
            }
        }
        self.steps += 1;
    }

    /// The cell's place in the cycle at plane point `(u, v)`, in `0.0..1.0`.
    pub(super) fn at(&self, u: f64, v: f64) -> f64 {
        grid_cell(self.size, self.grid, u, v).map_or(0.0, |at| {
            f64::from(self.cells[at]) / f64::from(SPIRAL_STATES)
        })
    }
}

impl Cache {
    /// The spirals `t` seconds in on a screen of `size`, advanced from where the last frame left them.
    pub(super) fn spirals(&mut self, size: (u16, u16), t: f64) -> &Spirals {
        let want = (t * SPIRAL_STEPS_PER_SEC) as u32;
        let fresh = self
            .spirals
            .take()
            .filter(|s| s.size == size && s.steps <= want);
        let spirals = self
            .spirals
            .insert(fresh.unwrap_or_else(|| Spirals::new(size)));
        while spirals.steps < want {
            spirals.step();
        }
        spirals
    }
}
