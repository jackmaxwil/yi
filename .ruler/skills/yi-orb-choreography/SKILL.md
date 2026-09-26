---
name: yi-orb-choreography
description: How to design, build and verify a new orb state loop or transition in yi-orb — the pose/loop/exit model, the loop-closure and geometry toolkit (Fibonacci lattices, bands, loxodromes, torus knots, harmonics, symmetry turns), the depth cue, the least-travel morph, the seam test, and the gallery workflow for a taste round. Load before touching crates/orb/src/states.rs or stage.rs, or when adding an agent state to the orb.
---

# Orb choreography

Read `yi-design` first: it says what a state should look like. This says how to make one.

## The model

- A **pose** is `POINTS` (540) points in the unit ball, before the camera's 0.3 rad tilt:
  `Point { at, edge, alpha, ghost }`. `edge` 0 is mid-band, 1 a rim (smaller, dimmer),
  below 0 larger and brighter. `ghost` 1 is the faint shell (`ghosts()`, 36 points).
- A **state** is `pose(Some(state), t)`: a pure function of seconds into its loop. The phase
  `th = t / secs` wraps in [0, 1). `spec(state)` gives `(secs, exits)`.
- **Invariant:** every parameter of a pose runs a whole number of cycles per loop, so
  `pose(t + secs) == pose(t)` as a picture. The seam test enforces it.
- **Exits** are the phases a state may be left from. `Orb::frame` holds a requested change
  until the next exit at least 0.1 s ahead (`LEAD`), then morphs 0.5 s (`MORPH`) from that
  exact pose to the new state's entry pose `pose(new, 0)`. Both shapes hold still while the
  points travel. The pairing runs on its own thread during the lead.
- The orb's clock advances at most 0.1 s a frame (`MAX_STEP`): a surface that stopped
  repainting hands its next frame minutes, and an unclamped step used to consume a whole
  animation.
- `pad` fills a short pose with invisible copies of visible points, so spares grow out of
  the shape in a morph instead of out of the centre.

## Adding a state

1. Write the concept as one literal image: "a plan is layers built bottom-up".
2. Choose the silhouette (the sphere unless meaning demands another), the one primary
   motion, and the lit/dim contrast. Check it against the five taste rules in `yi-design`.
3. Write `fn name(th: f64) -> Vec<Point>` in `states.rs` using the toolkits below. Every
   `sin`/`cos` of time takes `th * TAU * k` with integer `k`. Keep 400+ points visible.
4. Add the variant to `OrbState`, its label, its `spec` arm (Fibonacci seconds; quarters
   or eighths for exits, at poses that are rests of the rhythm), and its `pose` arm.
5. Map it in `App::orb_state` (crates/tui/src/app.rs) from data the app already holds:
   the running tool's name, running children, streamed text, streamed thought, the todo
   list's `progress()`. Data that changes the shape (a child count) rides in the variant, so
   a change is an ordinary transition.
6. Add the state to `every_state()` in crates/orb/tests/states.rs and run
   `cargo test -p yi-orb --test states`. Watch the seam test fail on a deliberately broken
   loop before you trust it.
7. Run a taste round (below) before the state is wired or landed.

## Loop-closure toolkit

| motion that resets | exact form |
|---|---|
| a sweep that snaps back | ping-pong: `scan = A * (th * TAU).cos()`, dwells at both ends |
| a pulse born and dying | own phase `ph = (th * n + lag).fract()`, amplitude `(ph * PI).sin().powi(2)` |
| a drift or spin | turn by the figure's symmetry per loop: `th * TAU / k` for a k-fold figure |
| per-copy phase offsets under a symmetry turn | tie the offset to the copy's current angle: `head = th * n + (i as f64 + th) / k` |
| a random walk | a closed tour: a fixed visiting order whose last hop lands on the first |
| a counter | accumulate in whole units per loop: `(th * laps).rem_euclid(1.0)` with integer laps |
| two waves | integer ratios, e.g. 5 and 3 cycles per loop, never 1.7 and 1.1 |
| a Möbius band | lane `i` swaps with lane `n-1-i` each circuit: give both the same stagger |
| an octant or copy turning in place | turn about its own centre, so a quarter turn lands it on itself |

## Geometry toolkit

- **Fibonacci sphere:** `fib_dir(i, n)`: `y = 1 - 2(i + 0.5)/n`, azimuth `i * PI * (3 - sqrt 5)`.
- **Band on a sphere:** `norm(u cos a + v sin a + (u x v) * off)`; lanes are `off` offsets,
  waves add `amp * sin(k a - m * th * TAU)` to `off`.
- **Loxodrome:** longitude `0.9 * ln tan(PI/4 + lat/2)` crosses every meridian at one angle.
- **Torus knot T(2, q):** `r = 0.58 + 0.36 cos(q f)`, point `(r cos 2f, 0.36 sin q f, r sin 2f)`;
  sample by arc length (`trefoil()`), or the pen slows in the tight inner loops.
- **Spherical harmonics:** `Y32 ~ sin^2 cos cos 2phi`, `Y43 ~ sin^3 cos cos 3phi`; a radius
  or a brightness field.
- **Clifford torus, Hopf fibres:** stereographic `xyz / (1 - w)` of `(cos u, sin u, cos v, sin v)/sqrt 2`;
  beautiful, but mush at terminal size — prototype at 96 px before committing to one.
- **Volume-conserving split:** child radius `cbrt(share)`, orbit `rp + rc + gap`, then scale
  the family so its reach is 0.98.
- **Cube lattice:** 6x6x6 at half-size `0.8 / sqrt 3`, so the corners stay inside the ball.

## Rendering

`render` rotates by the tilt, then per point: depth `d = (z + 1) / 2`,
radius `(0.935 + 1.445 d)(1 - 0.25 edge) * (64/300)^0.6`, white `0.52 - 0.44 d + 0.18 edge`,
opacity `(0.4 + 0.6 d) * alpha`; ghosts use radius 0.8, white 0.78, opacity 0.1 + 0.22 d.
The painter (`kitty::paint_rgba`) supersamples 4x4, composites premultiplied and writes
straight alpha at the terminal's own cell pixels. A pose under the tilt is seen from
slightly above: to face a flat figure at the viewer, pre-rotate it by `-TILT`; to view a
figure from above, rotate its top toward +z (+z is nearer).

## Verification

- `every_state_loops_without_a_seam`: the step from the last frame to the first, measured
  as a viewer sees it (each visible point's nearest visible point, position and opacity),
  must be no larger than the loop's own largest step.
- `a_change_waits_for_the_next_exit_then_lands_on_the_entry_pose`, the long-gap clamp test,
  the mark test and the pairing test pin the engine. A new mechanism gets its own contract
  test, seen red first (080).
- The faux provider answers instantly, so no end-to-end run animates a state; prove the
  kitty path ran with `scripts/tui_pty.py --term xterm-kitty --raw` and count the `a=t`
  transmits, and test the per-frame math in-process.

## Prototyping and the taste round

Prototype outside the tree (the session scratchpad), never in `crates/`:

1. A throwaway `crates/orb/examples/<name>.rs` that calls `yi_orb::pose`, `render` and
   `kitty::paint_rgba`, and writes each tile's frames as raw RGBA to `<dir>/<id>.rgba`.
   Delete the example before committing; `just commit` stages everything.
2. `python3 .ruler/skills/yi-orb-choreography/gallery.py <dir> <px> <tiles.json> <out.html>`
   builds looping APNG tiles composited on the terminal ground, with IDs and descriptions.
3. Review a filmstrip of every state over a minute before sending; send the page; ask
   graded or multiple-choice questions by tile ID.

## Pitfalls already paid for

- A `sin(time * 1.7)` never closes a loop; `(th * TAU * 5).sin()` does.
- A pose that looked right in one still was upside down: check the camera sign.
- A beam or comet whose brightness is a hard step jumps dot to dot and reads as lag: give
  the head a gaussian a few points wide.
- Greedy nearest pairing crosses paths late in the pairing; the Hungarian pairing does not.
- Too many motions at once reads as misshapen; one primary motion per state.
- Stored `$VAR` commands do not word-split in zsh; call the binary directly.
