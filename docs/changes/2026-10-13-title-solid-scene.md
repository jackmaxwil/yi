---
issue: Refs #1385
growth: +65 the Solid scene: the sphere trace and its swing, 65 lines of title.rs; weighed folding it into the Echo field and dropping the swing, and kept both: the owner ranked the sphere-traced name first of the scenes he had not yet seen drawn
raise: tests +42, comments +3, crate tui +65
---
A third title scene, `solid`: the `Yi` strokes extruded and sphere-traced, lit from above left, over faint echo rings. The solid grows its thickness from 0.3 s to 1.2 s, a camera swings once across it while it holds and returns, and from 2.2 s it shrinks onto the small mark with its thickness and swing going to nothing, so it lands as the other scenes do. Only cells within 0.45 of a stroke are traced, so a 200x50 frame traces a few hundred rays. It joins the default draw at weight 2 and `console.titleWeights` can name it as `solid`. Tests pin that it is absent before it has thickness, that its lit side covers 150 cells at the hold, that the swing moves the frame, and that it lands on the small mark's cells and leaves the screen back, the same test the Kali fold passes.
