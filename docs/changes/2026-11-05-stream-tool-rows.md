---
decision: "Stream tool rows, hold none" | "a running tool card reserved four blank rows before its result existed, which read as empty whitespace under the call" | reversible via padding the card body to PREVIEW_ROWS plus one in ToolCell::lines
---
A running tool card no longer reserves blank rows for the result it has not received. The card is its header alone until output arrives, then grows one row at a time as the result streams in, and a finished card ends on its last result row instead of padding to four. This replaces the bounded collapsed-card reservation of D516, which held the card's height to spare the screen a jump when the result landed.
