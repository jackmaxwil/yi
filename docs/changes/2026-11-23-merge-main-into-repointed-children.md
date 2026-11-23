---
cut: none scripts/pr_queue.py
---
When the queue lands a branch, the tick now merges main into each pull request it moves to main, instead of only rerunning that pull request's checks. A rerun reads the same head, so on 2026-11-22 a re-pointed pull request would have stayed red on the `url` advisory main had already patched, as #2237 did after its rerun. The tick falls back to the rerun when the forge refuses the merge or has nothing to merge, which the forge answers with success and no new head; when both fail it says so on the pull request. The lines `scripts/pr_queue.py` loses on net are the selfcheck's four cases rewritten as six on fewer, longer lines.
