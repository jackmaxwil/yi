---
name: verify
description: >
  Prove a change works by observing it run, not by running the suite again.
  Use before saying done, finished, verified or complete; when a goal's
  check is about to be claimed; when a todo is about to be stepped to done.
  Do NOT use to learn a repository's state for an assessment (that quotes
  the record) or to debug a failure (that is the debug skill).
trigger: verified, finished, complete, before saying done
scope: text
after: 1
---

# verify

`verify` → the surface the change touches → drive it → capture → PASS / FAIL / BLOCKED / SKIP per todo

Running the suite proves you can run CI. It does not prove the change
does what the request asked. Verification is observing the surface the
change touches, in the real binary, and keeping the capture as the
evidence you quote at `done`.

## The surface, by change type

| change | surface | how to drive it |
|---|---|---|
| a tool, the loop, a prompt fragment | the agent loop | `./target/debug/yi ask --here --model faux/faux-1 "<prompt>" --json` and read the event stream; a real model only when the change is about model behaviour |
| the TUI, the console, a card, the HUD | frames | `yi tui --headless --keys <script> --frames <dir> --here` with a temp HOME; assert on the frame text; count escapes for a graphics path |
| the ACP server, the daemon | a client round trip | the drive tests in `crates/console/tests/drive.rs` or a scripted `yi serve` session |
| a guardrail or a gate | the gate's own red | disable the gate (not the test) and watch the test fail for the gate's reason, then restore |
| a CLI verb | its output | run it against a fixture directory and diff the text |
| a session, todo or plan shape | the JSONL | read the entry the run wrote; the store is the truth |

An internal function is not a surface. A test in the diff is the author's
evidence, not a surface. If every step of your plan is build, typecheck,
run the test file, you have planned a CI rerun, not a verification.

## The regression, seen red

A fix carries a test that fails on the unfixed code for the fix's own
reason: revert the fix, run the test, watch it fail, restore, run it green.
Say in the report that you saw it red.

## Verdicts, one per todo

- PASS: the capture shows the requested behaviour; quote the line.
- FAIL: the capture shows something else; quote it; the todo stays open.
- BLOCKED: the surface could not be driven (no key, no device, a sandbox
  denial); say which, block the todo on what would unblock it.
- SKIP: the change has no observable surface (a comment, a rename with
  the compiler as its check); say so.

"3 of 4 passed" is FAIL until 4 pass. When in doubt, FAIL: a false PASS
ships broken code, a false FAIL costs one more look.

## Then

Step each todo `done` with the capture line as `evidence`; the report's
"Verified" means you ran it in this session and the exit code said so.
