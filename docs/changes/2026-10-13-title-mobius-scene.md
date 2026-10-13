---
issue: Refs #1385
growth: +35 the Mobius scene: the flow, its rings and its arm in the cover loop, in title.rs; weighed a texture lookup and a precomputed warp table, and kept the flow in closed form, which is smaller and exact
raise: comments +3, crate tui +35, tests +25
---
A fourth title scene, `mobius`: the name sampled through a loxodromic Möbius flow with fixed points at the screen's left and right of centre, so it starts as a vortex of rings and unwinds to the identity by 2 s, settling where every other scene draws it. Drifting rings of distance to the name are its texture and the name stays solid inside them. The flow is closed-form complex arithmetic, a divide per cell, and exact: at 2 s and after, the plane is sampled untouched. It joins the default draw at weight 2 and `console.titleWeights` can name it as `mobius`. Its tests pin that the vortex covers the screen, that it has moved by hundreds of cells before the hold, that the held name is where the other scenes put it, and that it lands on the small mark's cells like the rest.
