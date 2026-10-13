---
issue: Closes #1354
decision: The dist binary's hard cap moves from 8 MiB to 9 MiB, still checked independently of the measured ratchet | the binary is 8,372,272 bytes, 16,336 under 8 MiB, and the owner-approved console workbench (a terminal pane, a browser pane, sidebar groups, a header row) is estimated at about 1,450 lines, roughly 435 KB at the recent 300 bytes per src line, while other lanes grew the binary about 300 KB in three days | set `HARD_CAP_BYTES` back to 8,388,608 once the ratchet sits under it again
---
The dist binary's hard cap moves from 8 MiB to 9 MiB (Closes #1354). The owner's answer was "9 MiB (Recommended)". The ratchet itself is unchanged: `binary_size_budget.json` still holds the measured 8,372,272 bytes and still moves only by a measured `--update` in its own commit. `check_binary_size.py --selfcheck` now pins the shipped cap at 9 MiB, and `.ruler/060-deps.md` and YI_DESIGN.md §18.6 state the new cap.
