---
issue: Refs #1385
growth: +91 the Spirals scene, a twelve-state cyclic automaton in the cache module
raise: comments +5, crate tui +91, tests +31
---
A seventh title scene, `spirals`: a cyclic cellular automaton on a torus of at most 120 by 50 cells, twelve states, 55 steps a second. A cell takes the next state when a neighbour already holds it, and from the noise it starts as, spiral waves organise and turn; the strokes rise through them from 1.2 s as in the fold. It shares the `Cache` the growth scene introduced and the same replay rule: a fresh cache and a kept one agree, and a second that runs backwards or a new screen size starts it again. It joins the default draw at weight 2 and `console.titleWeights` can name it as `spirals`.
