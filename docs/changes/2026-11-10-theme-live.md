---
ledger+: TUI | the theme list includes the themes shipped with Yi, scrolling previews the colors that file sets, and borders, the orb, identicons, and the settings value column keep their own colors until a restart
growth: +708 weighed dropping the copied theme list and the extra cache keys; the list stays because the picker is empty without ~/.yi/themes, and the cache keys stay because transcript, markdown, diff, notebook, and kitty images otherwise keep the previous palette until restart
raise: crate cli +34, crate console +286, crate tui +1134, crate types +10, comments +38, tests +350
---

The theme list offers the omp themes and the four Yi themes shipped in the binary, and a file of the same name in `~/.yi/themes` still wins. Eight-digit hex keeps its red, green, and blue. The file can set the markdown, status, thinking, and comment colors Yi already paints.

Scrolling still does not repaint every element. Pane borders, title chips, and scroll thumbs stay on the session hue. Identicons stay on that hue. The orb and the logos keep their fixed ink. The settings value column stays teal. A role the file leaves unset, including a text color of reset, stays on the built-in color. Those elements look the same after a restart, because the theme file never owned them.

The repaint path is larger than the result. History rows, growing answer rows, markdown, diffs, review, notebook, question cards, and kitty images each grew a palette check so a preview would not wait for a restart. That check is what moves the colors the file does set. It does not move the elements above.
