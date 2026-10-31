---
issue: Closes #1923
---
A hidden console sidebar no longer draws its `⌥b sidebar` hint at the left of the header row. The hint row at the bottom already names the chord and word, so the header's copy was a second place for the same fact and it pushed the workspace tabs and the four pane buttons right by its width. The tabs and buttons now start at the header's left edge in every sidebar mode, and their hit rectangles match what is drawn.
