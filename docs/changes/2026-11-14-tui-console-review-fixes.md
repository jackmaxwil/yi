---
version: 0.1070.0
issue: #2180
growth: +105 correctness repairs for six merged PRs, weighed against deleting the paths they patch (none removable)
cut: none crates/console/src/sidebar.rs
cut: none crates/tui/src/result_paint.rs
cut: none crates/tui/src/rows.rs
raise: crate cli +24, crate console +33, crate tui +53, tests +167, comments +8
---
A summary-only answer that the session held back now keeps the draft it streamed, so the resubmission no longer draws the answer twice. Replay clears the answer track at every user turn, so a mail-started turn can no longer take back the previous turn's draft. The ctrl+o hint shows only where one press reaches the verbose view (Thinking mode). Search results colour grep context rows, keep their `-` separator in the gutter and read path-prefixed rows past the tag cap, with one shared line parser in rows. A pane with no rows left for the command list draws none instead of forcing one. The console keeps the saved place of a project the session store does not list when another is moved, and the saved project order reaches the console from config.json. The tool-result highlighting entry should read that file paths take the hue prose gives a path, which is the colour the code uses.
