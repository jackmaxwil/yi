---
cut: none scripts/pr_queue.py
cut: none scripts/pr_autofix.py
---
The merge queue's tick can report where every open pull request stands. With the repo variable `STATE_REPORT` set to `on`, it writes one `yi/state` check result on each open head, whether the queue itself is on, in shadow or off (off, the tick writes the report and does nothing else): the state, who is due to act, since when, and `parked` once that owner has held it past the state's clock. The state is the first matching row of a 21-row table in `scripts/pr_queue.py`, and each clock is a measured latency from the forge record of 2026-10-20 to 11-24. The result is always a success, is written only when the state, the owner or `parked` changes, and nothing reads it to decide anything. `just pr parked` lists the open pull requests past their clock, oldest first, and writes nothing.
