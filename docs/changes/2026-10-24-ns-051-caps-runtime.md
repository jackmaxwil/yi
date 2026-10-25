---
issue: Refs #1487, Refs #1442
raise: tests +574, crate runtime +96
---
This is Bet 2's loud-caps part for the runtime's own lists (unit CAPS-runtime). Seven cuts that shrank what the model reads without saying so now end in the row the reducer already uses, `[… kept of total what (CAP limit); the rest: next]`. `yi_tools::cut` builds that row and is crate-private, and the context-budget change turns it into a public `Cut` struct, so the runtime builds the same row from one function of its own, `listed::cut_row`. Swapping its body for `Cut` is the whole follow-up, and #1487 stays open for it.

`rlm.find_models(query, limit)` returned the first `limit` matches and nothing else. The host reply now carries a `cut` row when more matched, naming the matches it held back and the call that lists them all (`await rlm.find_models("", limit=31)`). The `rlm` shim in `python/yi_runtime` reads only `models`, so a cell does not print the row yet. That edit moves the runtime identity hash, so it ships with the one Python release; #1487 stays open until the shim prints `cut`, and this change only Refs it.

The `children:` environment row named every member of every state. It names at most eight a state now, keeps each state's true count, and ends in a row counting the names left out and pointing at `await rlm.status()`. The cap is per state so that a hundred finished children cannot hide the one that needs the parent.

A todo error that lists rows (a wrong label, an ambiguous prefix, several running, a wrong phase) lists at most twelve and ends in a row naming `LIST_ROWS` and `todo {"op": "view"}`. A 40-row list was about 4 KB on every failed call. An open-children list and the evidence echo are left alone.

A refused `answer` echoed its whole draft a second time as numbered rows. It echoes the rows that fit in `ANSWER_ECHO_BYTES` (4,096) now, then a row giving the lines shown over the lines held, the held file and the offset to read from. The file still holds the whole draft, so an `edit` of a line past the cut starts with a `read` at that offset.

The memory block's `+N not loaded` index row is a cut row now: notes loaded over notes indexed, the cap that cut (`LOAD_LINES` at 200 lines, or `LOAD_BYTES` at 25,000 bytes when the bytes cut first) and `await memory.search("words")`. The environment `files:` line ended `…(+N)` with no cap and no call, and skipped every dot-name without saying so; past `FILES_SHOWN` (20) it ends in the same row now, counting the hidden names it skipped and naming `ls -A`. The block-wide byte fit that follows the memory index is `fit` in yi-context and is not touched here: it belongs to the context budget change, which would collide with it.

The restore notice listed every restored name. It lists at most forty (`RESTORE_NAMES_SHOWN`) and names `%who` for the rest. The names that could not be restored stay whole: no call returns names a cap hid, and those are the ones the model has to act on. This is not the whole of kernel-python-15: a variable the snapshot skipped (unpicklable, or over the 16 MiB per-variable or aggregate cap) still vanishes on resume with no row, because the skip list never reaches the restore notice. That needs the snapshot's skip list carried to the resume, which is Rust in the kernel crate and the types, and stays on #1487.
