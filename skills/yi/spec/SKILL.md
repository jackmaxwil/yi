---
name: spec
description: >
  How to hold a task's numbered requirements to a check that runs: write the
  TAP check, freeze before the first write, run after every write, and treat a
  red or stale spec as unfinished work. Use on any task that states several
  requirements, names a product file, or asks for a metric. Do NOT use it to
  read code (grep, read), to record steps (todo, plan), or to run one command
  you are already reading the output of (bash).
covers: tool:spec
---

# spec

`freeze` before the first write → `run` after every write → every id green → answer

## Why it exists

A task's requirements live in the request, and the request cannot run. The spec
is the one object that turns each sentence of the request into an id whose
state a real check prints, and the stop is judged by that check rather than by
your prose. A stop is refused when the spec is missing on a task that states
requirements, when the last run is older than the last write, when any id is
red, or when a freeze's refute never passed.

## Steps

1. Quote the requirements from the request, one per id, in the request's order:
   `R1` is its first requirement. `f2p` for what the change must make true,
   `p2p` for what must stay true, `metric` for a number the request asks to
   improve.
2. Write the check as a script in the workspace, printing one TAP line per id:
   `ok 1 - R1 <what was verified>` or `not ok 1 - R1 <what is wrong>`. The id
   comes first in the description, one id per requirement. A metric prints
   `# metric name=value`.
3. `freeze` with the requirements, the artifact paths the request names, and
   the command (`python3 check.py`). Freeze runs the check on the untouched
   tree and refuses a requirement that cannot move: an `f2p` already green
   there, a `p2p` red, an id the check never prints, no `f2p` at all, an
   artifact that is the cwd or `/`, a check that prints no TAP, or fewer
   requirements than the request enumerates.
4. After every write, `run`. The harness runs the check, not you: a `bash` call
   naming the checker is not a run and does not clear a red id.
5. `add` a requirement you found after the freeze instead of inventing one
   inside the check.

## Reading the refusal

| Verdict | What to change |
|---|---|
| `no-spec` | the request states requirements and none was frozen: go to step 1 |
| `unfrozen` | `freeze` was refused: the text names every failing fact at once |
| `stale` | a write landed after the last run: `run` |
| `red` | the text names each red id with its first diagnostic line: fix the code, then `run` |
| `refute` | the check cannot tell the fixed tree from the untouched one: make a requirement fail at the baseline |

## The environment line

The environment block carries `spec: R1..Rn green|red, run <age>`, or says the
run is stale. Absent means the session has no spec, which a multi-step request
gets as a prelude instead.
