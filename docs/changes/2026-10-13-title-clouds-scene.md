---
issue: Refs #1385
growth: +39 the Clouds scene: value noise, a three-octave fractal sum and the warp, in title.rs; weighed reusing the Kali fold's folds for texture, and kept the noise: the owner ranked domain-warped clouds third of his favourites and a fold cannot look like weather
raise: comments +2, crate tui +39, tests +28
---
A fifth title scene, `clouds`: domain-warped value noise, the weather behind the name. It fades up over 0.3 s, drifts a lap every six seconds, and the strokes rise through it from 1.2 s as in the fold, then shrink onto the small mark. The noise is three octaves of integer-hashed value noise, warped once by two more noise reads (`f(p + 3.2 q(p))`, three fractal sums a cell, nine lattice reads each octave pass), so it needs no table or state and is a pure function of the screen and the second like the others. It joins the default draw at weight 2 and `console.titleWeights` can name it as `clouds`. Tests pin that it is empty at the first instant, covers the screen, moves hundreds of cells in half a second, lets the Y stem through solid, and lands on the small mark like the rest.
