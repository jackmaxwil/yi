---
issue: Closes #1289
raise: tests +167
---
The in-process test terminal now keeps region-scrolled rows in its scrollback (Closes #1289). vt100 saves a row to scrollback only when the whole screen scrolls, but kitty and Ghostty also save what a scroll region topped at row 0 pushes off, which is every history commit the TUI makes through `insert_before`. `VT100Backend` therefore lost every committed row, and no test could check that a streamed answer reached scrollback whole. `SharedParser` now follows `CSI t;b r` and `CSI n S` across crossterm's split writes, snapshots the rows a scroll-up pushes off the top, and `VT100Backend::transcript` returns them oldest first followed by the visible screen. A new test streams thirty paragraphs through the real app on an 80x24 screen and asserts each lands in the transcript once, in order, and never on a row with the composer's border.
