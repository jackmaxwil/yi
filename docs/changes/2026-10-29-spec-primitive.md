---
issue: Closes #334
decision: the spec is the harness's object, not the model's | a stop judged by prose passed 3/18 on the six-task slice while the checks failed on one numbered requirement, and the one object that can say which id is red is a check the harness runs | delete the `spec` tool and its store; the stop rule then has no verdict to consult and the prompt's prelude has nothing to announce
growth: +2535 the spec object, its TAP parse and the freeze that types it, the harness-run stop rule with its refute, the environment line and the skill, and a per-session baseline directory cleared before each freeze; weighed: no artifact or closure gate rebuilt beside it (the config's `gates` key went with D182), no dependency added (the TAP parse is one function over a string), and the first attempt's `bind`, `fanout` and `obligation` stages are left out so the change is the primitive and its stop rule
raise: crate runtime +1920, crate types +520, tests +939, comments +79
---

A stop is judged by a check the harness runs, not by the model's prose. The
`spec` tool (`freeze`, `run`, `add`, `view`) holds requirements quoted from the
request (`R1`..`Rn`), the artifact paths the task names and one check whose
stdout is TAP lines naming requirement ids; `freeze` runs that check on the
untouched tree and types every requirement `f2p` (red there, must turn green)
or `p2p` (green there, must stay green), refusing with every failing fact at
once: an `f2p` already green at the baseline, a `p2p` red there, an id the check
never prints, no `f2p`, an artifact that is the cwd or `/`, a check that prints
no TAP, a metric without its line, and fewer requirements than the request
enumerates. A run is the harness's own: a `bash` call naming the checker is not
a run, so it never clears a red id. The check is a command the model wrote, so
it runs the way `bash` runs one: keyless (no provider key in its environment)
and inside the session sandbox, on the baseline, on every run and on both
refute mutants.

The stop rule refuses a clean stop of a request that enumerated items when no
spec was frozen, when the last run is older than the last write (which every
writer tool's own result records, so a fresh green run cannot outlive an edit),
when any
requirement is red (naming each id with the check's first diagnostic line) and
when the refute — the baseline plus the empty and restored artifacts — shows
the check cannot tell the fixed tree from the untouched one. The refusal
repeats; with `--deadline` the clock is the only other exit, and without one the
same refusal waives after three and records that it did. Todo and plan steps do
not yet bind to requirement ids, and the prompt doctrine is not rewritten: both
are the plan's later stages and land under their own issues.

The environment block carries `spec: R1..Rn green|red, run <age>`, or says the
run is stale; the skill at `skills/yi/spec/` teaches the freeze-run-refute loop.
