---
issue: Closes #1504, Refs #1442
raise: crate console +4, crate tui +18, tests +189
---
This PR hardens drive and tool-cell tests in two units, in commit order: HD-V05-console-oracle and HD-V03-real-producer-fixtures.

HD-V05-console-oracle. Console `wait-frame` needles now match within one rendered row instead of across the frame's serialized rows. The new `wait-row` step matches a whole trimmed row. A failed solo `wait-frame` or `wait-row` stops the script instead of running later input on the wrong screen.

HD-V03-real-producer-fixtures. The ceiling-nudge test replays metadata recorded from real bash calls. A yi-cli integration test feeds real read, grep and edit results to the TUI's cell rendering, and console permission drives use scrubbed frames recorded from a real worker for reject, close-with-ask and a second ask.
