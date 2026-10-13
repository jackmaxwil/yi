---
issue: Refs #1385
growth: +40 the Attractor scene, one function
raise: comments +2, crate tui +40, tests +36
---
An eighth title scene, `attractor`: a Clifford attractor drawn as 30,000 orbit points binned onto the screen's cells and shaded by the log of each cell's visits, so the filaments glow where the orbit lingers. Two of its four parameters drift slowly, so the shape breathes without leaving its box (the other two fix |x| < 2 and |y| < 1.7, which the screen maps to its middle 95%); the orbit is rerun from its seed each frame, which costs a few thousand operations and needs no cache. The strokes rise through it from 1.2 s as in the fold. It joins the default draw at weight 2 and `console.titleWeights` can name it as `attractor`.
