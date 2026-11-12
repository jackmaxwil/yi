---
issue: Closes #2046
decision: Keep a text-only OSC 2 title for the focused foreground run | the terminal tab should distinguish working, attention and ready without sending the Kitty orb image through the title | keep the full orb in terminal content and background work from pulsing the parent title
growth: +426 title card parser, tab mark table, console and solo title wiring; weighed deleting the emoji card and the 250 ms wake scheduler, but the card is the issue and without the wake a working tab freezes between events
raise: tests +287, comments +13, crate console +189, crate runtime +145, crate tui +92
shares: #2149 another lane edits the same draw path in render.rs
shares: #2139 another lane edits the console, app and design doc near the title
shares: #2138 another lane edits the console and app near the title
shares: #2137 stacked on the same imported title code
shares: #2136 superseded by this change, to be closed
shares: #2135 superseded by this change, to be closed
shares: #2134 another lane edits the design doc
ledger=: terminal tab title | 17.3, 17.4 | gated tui (default feature) | M | M | the tab reads `Yi <mark> <label>`: `◌ ◍ ◉ ◍` every 250 ms while the focused foreground run works, `!` when it needs the user, `>` when ready, from one `TabMark` table the solo TUI and the console share; the console label is the summarizer's emoji card, the solo label is the latest prompt; full Kitty orb stays in terminal content
---
The solo TUI and the console name the terminal tab `Yi`, a state mark and the session label. A four-frame text pulse runs every 250 ms while the focused foreground run works, `!` means it needs you and `>` means ready; background jobs and child sessions do not keep the parent title pulsing once its run ends. The summarizer, which runs after the first turn, asks the model for an emoji, a short code and a title, stored as `🧪 FLAKY: Fix flaky park tests`; a reply whose markup does not parse is declined instead of stored, and an emoji carrying an invisible or bidi-override character is refused. Only the console shows the card; the solo TUI shows the latest prompt.
