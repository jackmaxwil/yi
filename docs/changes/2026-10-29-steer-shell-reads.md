---
issue: Closes #1436
decision: a read-only shell read answers with a `[…]` row naming the dedicated tool, not a refusal | a refusal would take the raw bytes away from calls whose point is those bytes (a binary probe, a format read cannot reproduce, an exit status), and the bash tool already answers `pdftotext`/`pandoc` with a hint rather than a refusal; the row rides the first line of the answer, so it cannot be missed the way a doctrine line was | reversible via deleting the `shell_read_hint` call in `BashTool::execute`
growth: +67 the classifier, its two messages and their tests; the verbs cannot come from `segment_category`, whose `list_files`, `vcs` and `build` arms must not steer
raise: crate tools +69, tests +80, comments +2
---
Dogfood session `1792775630063_01a169c1` read a whole repository through `bash`: `sed -n '1,80p'`,
`head -60`, `grep -rn "unwrap()\|expect(" … | grep -v test | wc -l`, which printed `0` because a line
filter drops the lines holding the word and not the test modules. The doctrine already said to use
`read` and `grep`, and the tool boundary said nothing back.

`crates/tools/src/steer.rs` classifies the command before `BashTool::execute` runs it. Every segment
must be one the permission classifier calls safe and the dedicated tools serve — `cat`, `head`, `tail`,
`find`, `fd`, `tree`, `sed -n` and `grep`/`rg`, with `wc`, `sort`, `uniq` and `cut` allowed after one of them — and the
answer then opens with one row: `[read answers this: path=<file> offset=<line> limit=<n>; …]` for a
file print, `[grep answers this: pattern=<regex> path=<dir> count for per-file hit counts; …]` for a
search. A redirect, a `$(` or backtick substitution, a vcs, build or test segment, or any verb outside
that set keeps the bare output, so `git log -1`, `cargo test 2>&1 | tail -5` and `just check` are
untouched.

A single-file `cat`/`head`/`tail`/`sed` view still carries its `[path#TAG]` header first, where the
hashline bridge puts it; the steering row is the line under it.
