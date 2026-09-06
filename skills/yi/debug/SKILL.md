---
name: debug
description: >
  Reproduce, isolate and root-cause a failure with the commands this
  repository gives you, one hypothesis at a time. Use when a test fails,
  a panic or a compiler error appears in a result, a command behaves
  differently from what the code says, or the user reports a bug. Do NOT
  use to run the suite for a status check (that is the gate skill).
trigger: panicked at, error[E, FAILED, thread ', test result: FAILED
scope: result
---

# debug

`debug` → reproduce → one hypothesis → kill or confirm → root cause across callers → regression seen red

## Reproduce first

The smallest form that still fails, in this order of preference: one test,
one command, one script. Until you have a reproduction you have a report.

```
cargo nextest run -p <crate> -E 'test(<name>)' --no-capture
RUST_BACKTRACE=1 cargo nextest run -p <crate> -E 'test(<name>)' --no-capture
./target/debug/yi ask --model faux/faux-1 "<prompt>" --json     # the loop, offline
yi tui --headless --keys <script> --frames <dir> --here            # the TUI, frames dumped
```

For a TUI failure read the frame dump the run wrote before the diff. For a
runtime failure read the session file the run wrote (`~/.yi/sessions/…`).
For a race, the evidence is in the run, not in the code.

## One hypothesis at a time

Write the hypothesis in a sentence the reproduction can kill. Test it.
Let the result kill or confirm it before the next. Never stack two
speculative fixes; never re-run an identical command hoping for a
different answer; never diagnose from the diff when the run left evidence.

A tool call that failed is read in the tool's own error text before it is
called again: an unknown flag, a filter that matched nothing, a path that
does not exist are all named there.

## Root cause across callers

When the cause is found, `grep` (or `grid uses`) every caller of what you
are about to change. A guard where all callers route through beats a guard
per caller; the report named one symptom of a shared cause, and fixing only
the named path leaves the siblings broken.

## The regression test, seen red

Write the test that fails for the cause's own reason. Revert the fix, run
the test, watch it fail, restore the fix, run it again. A test that passes
on the first try against broken code proved nothing; two shipped this way
in this repository.

## When the gate is red after your change

It was your change. Fix the code, never the baseline, never the test. The
one exception is a gate that measures wall time (startup) under a
concurrent build; re-measure idle before believing it, never to explain
away a number that stays high. A stale-cache theory is tested by
`touch`ing the crate's `lib.rs` and re-running, not by believing it.

## Sandbox denials are not bugs

`PermissionDenied` on a socket bind, a network call or a write outside the
working tree inside auto mode is the sandbox. See the gate skill's list of
tests that cannot pass inside it.
