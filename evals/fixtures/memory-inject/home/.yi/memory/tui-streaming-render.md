---
name: tui-streaming-render
description: Streaming renders commit by byte cursor over standalone slices, never by line counts from re-rendering a growing prefix.
type: feedback
---
Renderers trim trailing blanks, so a line count drifts and the list duplicates.

**Why:** the duplicated-list bug shipped exactly this way.

**How to apply:** a markdown or story that streams changes bytes, not rows; drive it with `yi tui --headless`.
