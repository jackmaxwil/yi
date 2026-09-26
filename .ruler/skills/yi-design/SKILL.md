---
name: yi-design
description: Yi's design philosophy and visual language — what makes a Yi surface feel like Yi, the owner's taste rules for motion and form, and how to run a taste round. Load before designing or reviewing any Yi surface (TUI chrome, the orb, console panes, a new visual), before proposing a visual direction, and whenever a design "looks off" and nobody can say why.
---

# Yi design

Yi is one binary that does real work in a terminal. Its surfaces are instruments, not
decoration: every pixel either tells the user what the agent is doing or gets out of the way
of the text they are reading. The engineering law (docs/YI_DESIGN.md, .ruler/) and the visual
language below are the same philosophy seen from two sides.

## The philosophy in eight lines

1. **Few primitives, everything a composition.** A new noun must absorb an existing mechanism.
   The orb is one medium (dots in a unit ball) and one engine (poses + morphs); a new state is
   a function, never a new renderer.
2. **Show the concept literally.** A plan is layers built bottom-up; delegation is one body
   splitting into children; reading is a scan down a page. If the picture needs a caption to
   mean anything, it is decoration. Owner, verbatim: "the visual representation of the concept".
3. **Deterministic and exact.** State is data, triggers read data, loops close exactly,
   transitions start and end on fixed poses. Nothing is random, nothing is judged at runtime.
4. **Least motion.** One primary motion per state; transitions take the shortest paths and
   never cross. Owner: "the movement looks most natural and least movement".
5. **Calm, not slow.** Motion never pulls the eye off the answer, but a surface that drags
   reads as broken. Half speed was "all too slow"; some loops were "slightly too fast".
6. **Loud about limits.** A cut, a cap, a fallback names itself where it happens (045).
7. **Deletion first, growth priced.** A byte earns its place or goes; budgets are ratchets.
8. **Cold-reader prose.** Names are plain CS terms, never analogies; comments carry only the
   incident or invariant; a PR reads without the session that wrote it.

## The visual language

**Medium.** Dots with depth: radius, ink and opacity all rise toward the viewer (the depth
cue in `yi_orb::stage::render`). One ink, TokyoNight lavender `#c8d3f5`, over the terminal's
own ground; depth is brightness, never hue. Straight alpha, rendered at the terminal's own
cell pixels. A faint shell of 36 ghost dots gives any open figure a volume.

**Lattice.** Surfaces are Fibonacci spheres: each point 137.5° (the golden angle) round from
the last, the way sunflower seeds pack. Even spacing is what makes a lattice read as mass.

**Proportion and time.** φ where a ratio is needed: breath inhale : exhale = 1 : φ; nested
radii 1, 1/φ, 1/φ²; an area change of 1/φ. Loop lengths are Fibonacci seconds (5, 8, 13, 21).
Counts are Fibonacci where they are free (8 petals, 13 s tours).

**Silhouette.** Mostly one sphere, so the orb stays one object. A state earns its own
silhouette only when the meaning demands it: the trefoil for editing, the cube for
computing, the moons for delegating.

## Taste rules (from graded rounds)

Graded 2026-09-26: octants splitting and rejoining 10/10, layers building 9.5, the one-to-many
split 9.5, the reading scanline 9; a wireframe globe with flight arcs 5, a typing cursor on the
ribbon 6, random sparks 7. The grades follow five rules:

| rule | wins | loses |
|---|---|---|
| the whole form changes | splits, fills, breaths | a few dots lighting on a static form |
| ordered rhythm with rests | apart, turn, together, rest | random or continuous motion |
| mass | lattices, filled volumes | thin strokes, wireframes |
| large lit/dim regions | half the form lit, a wide band | small highlights; clustered highlights |
| literal concept | layers = plan, split = delegate | an image that needs a caption |

Seen and rejected, with the reason: too many simultaneous motions (twist + wave + flow +
tilt); dot paths crossing mid-morph; a shape dissolving into dust halfway; a symmetric
4-second 0.6↔1.0 pulse ("unnerving": it looms); lobes too small or viewed from the wrong side;
a band edge-on reading as a stick; a resting mark restyled without asking.

## Exactness is a design value

- A loop is exact: every motion runs whole cycles per loop, so the last frame flows into the
  first. "Reading goes from top to bottom, then bottom to top smoothly" — never a snap back.
- A state has one entry pose and declared exit phases. A change waits for the next exit
  (at most one beat) and morphs from that exact pose to the next entry pose.
- Resets become ping-pongs (a cosine), closed tours, or turns by the shape's own symmetry.
- Births and deaths fade (sin² of their own phase); nothing pops.

## Running a taste round

1. Render every candidate through the real painter (`yi_orb::kitty::paint_rgba`), never a
   mock, at the terminal's size and at 2x.
2. Show labelled tiles in one HTML page: an ID (S3, M5), the name, the trigger, one line of
   what it depicts. Unlabelled grids were unreadable to the owner.
3. Before sending, review your own frames across a whole minute, every N frames. Misshapen
   phases hide between the frames a short clip shows (a camera that looked from below hid a
   whole brain's activity for a round).
4. Ask at most four questions, each with at most four options, recommendation first. When
   the owner cannot name what is off, offer the candidate causes as options.
5. When the owner grades, find the rule the grades follow and state it back as a table,
   then redesign the losers by the rule.
6. "Previous" and "revert" mean the immediately prior version. A liked version is kept
   verbatim; a principle is not a template to copy onto other states.

## Where the language already lives

- `crates/orb/src/states.rs`: the rest mark and the thirteen state loops.
- `crates/orb/src/stage.rs`: the depth cue, the least-travel morph, the exit-point machine.
- `crates/tui/src/app.rs` `orb_state`: which agent state is on screen.
- The `yi-orb-choreography` skill is the how-to for new states.
