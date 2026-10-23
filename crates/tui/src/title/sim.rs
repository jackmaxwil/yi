//! The title scenes that remember: a simulation advanced to the second asked for and kept between
//! frames. A second that runs backwards, or a new screen size, starts it again from nothing.

use super::{distance, lattice};

/// Steps a second: the coral takes 1500 of them by the time the name lands.
const GROWTH_STEPS_PER_SEC: f64 = 500.0;
const GROWTH_COLS: usize = 80;
const GROWTH_ROWS: usize = 32;
const SPIRAL_STATES: u8 = 12;
const SPIRAL_STEPS_PER_SEC: f64 = 55.0;

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

impl Sim for Growth {
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

    fn kept(&self) -> ((u16, u16), u32) {
        (self.size, self.steps)
    }

    /// The coral's concentration at plane point `(u, v)`, in `0.0..=1.0`.
    fn at(&self, u: f64, v: f64) -> f64 {
        grid_cell(self.size, (GROWTH_COLS, GROWTH_ROWS), u, v)
            .map_or(0.0, |at| f64::from((self.v[at] * 3.2).clamp(0.0, 1.0)))
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

impl Sim for Spirals {
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
        Self {
            size,
            grid,
            steps: 0,
            cells,
        }
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

    fn kept(&self) -> ((u16, u16), u32) {
        (self.size, self.steps)
    }

    /// The cell's place in the cycle at plane point `(u, v)`, in `0.0..1.0`.
    fn at(&self, u: f64, v: f64) -> f64 {
        grid_cell(self.size, self.grid, u, v).map_or(0.0, |at| {
            f64::from(self.cells[at]) / f64::from(SPIRAL_STATES)
        })
    }
}

/// A simulation the cache can keep: made for a screen size, stepped, and read at a plane point.
pub(super) trait Sim {
    fn new(size: (u16, u16)) -> Self
    where
        Self: Sized;
    fn kept(&self) -> ((u16, u16), u32);
    fn step(&mut self);
    fn at(&self, u: f64, v: f64) -> f64;
}

/// Invariant: a kept run is only continued forwards on the size it was made for, so it always
/// equals a fresh run to the same step and a frozen clock replays any frame.
fn advance<S: Sim>(slot: &mut Option<S>, size: (u16, u16), want: u32) -> &S {
    let fresh = slot
        .take()
        .filter(|sim| sim.kept().0 == size && sim.kept().1 <= want);
    let sim = slot.insert(fresh.unwrap_or_else(|| S::new(size)));
    while sim.kept().1 < want {
        sim.step();
    }
    sim
}

impl Cache {
    /// The stateful scene's simulation `t` seconds in on a screen of `size`, if `scene` has one.
    pub(super) fn sim(
        &mut self,
        scene: super::Scene,
        size: (u16, u16),
        t: f64,
    ) -> Option<&dyn Sim> {
        match scene {
            super::Scene::Growth => Some(advance::<Growth>(
                &mut self.growth,
                size,
                (t * GROWTH_STEPS_PER_SEC) as u32,
            )),
            super::Scene::Spirals => Some(advance::<Spirals>(
                &mut self.spirals,
                size,
                (t * SPIRAL_STEPS_PER_SEC) as u32,
            )),
            _ => None,
        }
    }
}
