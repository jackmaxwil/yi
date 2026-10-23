---
issue: Closes #1432
raise: crate runtime +14, crate tui +141, comments +15, tests +148
growth: +155 the status row now carries a hue per role and the dirty-count read; deletion weighed first: the `fmt_exact` thousands formatter went with the exact token count, and `tree_state`'s status loop became the shared `tally`
---
The status row moved onto the composer's top border and was restyled after Omp's. While the border is free it reads `╭─ model › ◉ effort › path › ⎇ branch +N ──── $0.12 › 2.9% / 1M › name ╮`, one row shorter than before; a reverse-i-search title, a reply title or an open popup send it back to a row of its own. Segments are joined by `›`, and each role has its own hue: the place cyan, the branch orange with its changed-path count in warning, the landing purple, spend green, cache teal, subagents magenta. The context meter shows its load as a percentage of the window (`2.9% / 1M`) and turns warning at 60% and alarm at 85%, where it used to print an exact token count nothing coloured. The branch carries `+N` for changed and untracked paths, read by a short-lived thread after each reply so the render thread never waits on git; a read that times out leaves the badge off. The composer's cursor is a block in the theme's accent colour.

The initial viewport is three rows, the composer alone, where it was four with the status row. A drive script that waited on `0 / 128K` waits on `0.0% / 128K`.
