---
issue: Closes #2046
decision: Keep a text-only OSC 2 title for the focused foreground run | the terminal tab should distinguish working, attention and ready without sending the Kitty orb image through the title | keep the full orb in terminal content and background work from pulsing the parent title
growth: +358 title mark table, the 250 ms pulse scheduler, and the console and solo title wiring; weighed deleting the wake scheduler, but without it a working tab freezes between events
shares: #2149 another lane edits the same draw path in render.rs
shares: #2139 another lane edits the console, app and design doc near the title
shares: #2138 another lane edits the console and app near the title
shares: #2137 stacked on the same imported title code
shares: #2136 superseded by this change, to be closed
shares: #2135 superseded by this change, to be closed
shares: #2134 another lane edits the design doc
ledger=: terminal tab title | 17.3, 17.4 | gated tui (default feature) | M | M | the tab reads `Yi <mark> <label>`: `◌ ◍ ◉ ◍` every 250 ms while the focused foreground run works, `!` when it needs the user, `>` when ready, from one `TabMark` table the solo TUI and the console share; the label is the session's own name, the latest prompt before the summarizer names it; full Kitty orb stays in terminal content
raise: crate console +183, crate runtime +93, crate tui +82, tests +322, comments +9
---
The solo TUI and the console name the terminal tab `Yi`, a state mark and the session label. A four-frame text pulse runs every 250 ms while the focused foreground run works, `!` means it needs you and `>` means ready; background jobs and child sessions do not keep the parent title pulsing once its run ends. The label is whatever the session is already called: the latest prompt in the solo TUI, the summarizer's title in the console once it has one. Session naming itself is unchanged.

Seen red: `title::tests::a_card_shaped_reply_is_not_reformatted_into_a_card` stored `🧪 FLAKY: Fix flaky park tests` against the previous head. It now stores `<title emoji="🧪" code="FLAKY">Fix flaky park`, proving a card-shaped reply is no longer parsed as a card.
