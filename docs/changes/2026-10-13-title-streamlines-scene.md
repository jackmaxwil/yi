---
issue: Refs #1385
growth: +50 the Streamlines scene, one function
raise: comments +2, crate tui +50, tests +20
---
A ninth title scene, `streamlines`: lines of constant stream function seeded on a jittered grid, so none cross, with a pulse of light riding each along the flow. The lines stand still and only the light moves, which keeps the screen calm where the fold is busy. Each line is traced both ways from its seed through a value-noise stream function and the orbit is cheap enough to rerun each frame (about 500 lines of 50 steps on a 200 by 50 screen), so it needs no cache. The strokes rise through it from 1.2 s as in the fold. It joins the default draw at weight 2 and `console.titleWeights` can name it as `streamlines`.
