---
issue: Closes #1352
growth: +616 the gear, close button and keycap rasters, the keycap dock and hover, the settings tab strip and the shared rail line; weighed for deletion: the per-hint outline images (deleted for the keycaps), measuring cell pixels (kept, a stretched 64 px gear read soft in Ghostty), and a second image ledger (not added, every control rides the avatar ledger)
raise: crate console +616, tests +64, comments +10
---
The console's controls are drawn as kitty images at the terminal's own cell pixels now, rather than as glyphs (Closes #1352). The settings gear in the sidebar's corner is a thin-line gear, its hub ring and eight rounded teeth anti-aliased from a distance field; a second press on it closes the settings it opened. Each pane's close button is a rounded chip with the X knocked out, so the terminal's ground shows through the strokes. Terminals without kitty graphics keep `⚙ ⌥,` and an inverted `✕` chip.

The hint row is a dock of keycaps, one theme colour per key: palette blue, new session green, sidebar cyan, notebook orange, diff red, keys purple. Ghostty 1.3.1 draws every image above the text whatever its `z`, so a keycap's body is a cell background and its chord stays text; only the rounded ends, cells with no text in them, are images. A press anywhere on a key's segment runs its key, and the key under the pointer lightens. The row still drops whole keys by rank, and the wider keycaps mean the sidebar key now drops beside the full sidebar at 100 columns.

The settings tabs are a strip with the current tab as a chip, scrolled to keep it in view with an arrow at each cut end. Framed panes stand on the rail's own column, so one line parts the rail from the panes, and that line runs through the gear's rows to the bottom of the screen. The image controls moved from `render.rs` into `chrome.rs`, which took `render.rs` back under the 1,200-line file cap.
