---
issue: Closes #1481, Refs #1442
raise: tests +82, crate cli +32
---
The unit in this change is E-cli-ask-output.

Plain `yi ask` printed each text delta as it arrived and one newline at the very end, so a run of two turns that each said something printed `Let me look.Done.` and a script could not tell the narration from the answer. A turn that said text now ends with a blank line before the next turn's text is printed. The blank line is written when that next text arrives, so a turn that said nothing between two that did adds none, and the run still ends with the single newline it always had. `--schema` runs print nothing as they go and are unchanged.

`--json` keeps printing every event. A new `--no-deltas` flag, a row in the command table, drops the `message_update` lines from that stream: on the faux provider `yi ask --json hi` prints 2,250 bytes and 1,945 with the flag, and on a real model the dropped lines are one per token. Every other event is kept and the stream still ends in `agent_end`. The flag is read by `ask` alone and only with `--json`: `yi ask --no-deltas` without `--json` exits 2 with ``error: --no-deltas needs `yi ask --json` ``, and `yi stats --json --no-deltas` exits 2 with ``error: --no-deltas is not applied by yi stats; pass it to `yi ask --json` ``, so neither is a flag that silently matched nothing. The bash, zsh and fish completions name it, and the drift test against `yi --help` holds them to it. The Terminal-Bench adapter's `grep -v message_update` is left alone, since it may run an older binary. docs/YI_DESIGN.md section 17.1's `ask` row states both behaviours.
