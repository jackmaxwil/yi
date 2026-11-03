---
issue: Closes #1543
growth: +540 a row carries a check whose command is set once, probed at start, handed to the same rules and wall as any other command, and run by done: the red path, the two refusals, the answer sweep and the label-resolution module moved out of the over-cap todo/mod.rs are the new code, and the digest the check stores in the fingerprint is the only new wire field on a todo row
raise: comments +20
raise: crate runtime +405
raise: crate types +71
raise: tests +337
ledger+: todo tool + loop coupling | a row may carry a check (`start` with `check=`), set once and never replaced: `start` probes every program the command names before it writes anything and `arms` hands that command to the same rules, wall and gate as any other, while `done` runs it in the session's cwd with the checker's cleared environment and one deadline, exit 0 closing the row with `check `<cmd>` passed` as its evidence and anything else keeping it running with the output; the red's own digest rides the list's fingerprint, so an unchanged red still climbs the stop ladder and a moved one resets it; `drop` of a red row notes `dropped while red: <cmd>`, `rm` and a `set` that omits, changes or blanks a checked row are refused naming `drop`, and an interactive `answer` runs each open checked row once and lists a red as `still red: <cmd>`
---

A todo row now carries a check. `start` takes `check=`, stores it once and never replaces it;
`done` runs it in the session's working directory under the checker's cleared environment and
closes the row only when it exits 0, so a red leaves the row running with the output in view and
`drop` with a reason is the only way past it.

`start` probes the command before the row is written: every segment's first word must be a program
on `PATH`, so a check built on a typo is refused while nothing it would have run exists on disk.
`arms` reports that command to the same rules, wall and gate as any other call, so a denied command
never runs once, at start or at done. `rm` and a `set` that omits, changes or blanks a checked row
are refused, naming the `drop` that keeps the red visible; a green row is released like any other.

The list's fingerprint appends a red's digest, computed from the output it produced: an unchanged
red still climbs the stop ladder, and a red whose output moved resets it. An interactive `answer`
runs each open checked row once, closes the ones that pass, and lists the others as
`still red: <cmd>` in the turn's run end.

A check runs with a ten-minute deadline of its own; the session's remaining deadline is not
threaded to it yet, because the tool context carries none.
