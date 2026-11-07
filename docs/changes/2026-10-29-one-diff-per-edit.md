---
issue: Closes #1532
growth: +427 one line-diff builder replaces four copies per edit; four runtime helpers and two hashline unquoters fold into one each, and the rest is the tests that hold them.
raise: tests +497
raise: crate runtime +246
raise: crate tools +264
raise: comments +16
ledger~: `get_context` orientation packet | r; skeletons ranked by `symbol`, then change heat, then name | r that counts only the layers the call asked for, a layer it did not ask for saying `not asked:` rather than counting as absent; skeletons ranked by `symbol`, then change heat, then name, and reading a declaration word one indent level in, so an `impl`'s methods and a class's defs reach the model
---
A 2,000-line edit paid the same line diff four times: once for the rendered patch, once for the changed lines, once for the line map, and once for the ripwire regions. `EditScript` is now built once per `(before, after)` pair, carried on the section's result, and read by all four, so the quadratic table is computed once. A snapshot's text is an `Arc<str>`, so the clones an edit's reply makes are pointers, and the snapshot lock is released before the syntax checker spawns instead of being held across it.

A multiline grep numbered each hit by counting newlines from the file's start, which made a 4 MB search over about 15,000 matches scan tens of gigabytes; the walk between matches is one pass now and the numbers are unchanged. `rustfmt` is checked on the file's text over stdin rather than by path, because a path makes it follow the root's `mod` declarations and read, and fail on, a child file the edit did not touch.

Repository discovery now remembers a miss as well as a hit, so a start outside a checkout no longer runs the same failing Git probe once for the lane, memory and session stores.

A grep hit on a line past 512 characters was cut with `…` and nothing said so, so the model saw a hit whose matched text was gone and no way back to it. The page now ends with the same row `read` writes for a clipped line: how many lines were cut, the 512-character cap, and the `sed -n` call that prints them whole.

`get_context`'s header counted all six layers whether or not the call asked for them, so a plain call opened `PARTIAL` and read as a broken packet; it counts the layers asked for, and a layer it did not ask for says `not asked:` instead of `absent:`. The same packet's file skeletons now show declarations one indent level in, so an `impl`'s methods and a class's `def`s reach the model instead of only the block that opens them.

Five helpers that each existed twice are one each: the goal check, plan verify's checker and the `exec://` adapter share one shell-check builder that takes the environment policy, the deadline and the stop flag; `content_hash` is `fetch`'s one implementation; `KernelServiceMap::read` and `dump` share their prologue; the two kernel variable cells share their namespace lookup; and hashline's two path unquoters are one function.
