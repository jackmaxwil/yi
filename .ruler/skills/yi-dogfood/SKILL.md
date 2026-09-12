---
name: yi-dogfood
description: Dogfood a change to what the model reads about a Yi tool — the claims ledger, probes on real inputs, and the sections the tool-surface lock makes a PR owe
---

# Dogfooding a tool

The request-budget gate locks what the model reads: every registered tool's description and
schema, each kernel extra and the identity fragment (`scripts/guardrails/baselines/tool_surface.json`).
When it fails with `tool surface changed`, run
`python3 scripts/guardrails/check_request_budget.py --update`, commit the two baselines alone as
a `Ratchet:` commit, and `just pr-body` prints the sections the PR now owes. A green suite is
where this starts: every D171 defect sat beside passing tests.

1. **Claims.** List every sentence the model will read about the change — description,
   schema docs, errors, hints that name a remedy — as `claim · check · result`. Derive the
   claim from the thing where you can (the format list from the wheel), test it where you
   cannot, and run every remedy a hint names in the real venv or shell. This table is the
   PR's `## Claims ledger`, owed for any changed or added key.
2. **Probe.** Every suspicion becomes a minimal probe before it is a finding: N threads on
   one new input, the limit and the limit plus one, one bad part, a content change that keeps
   the timestamp, a cancel, a non-UTF-8 name. Walk the ten classes in
   docs/plans/2026-09-10-dogfood-method-and-mandates.md §3; give each finding its evidence
   and P1 (wrong or unsafe), P2 (costly) or P3 (polish).
3. **Close.** Each finding: a real-producer fixture (never Yi's output, a user's file or a
   benchmark's answer key), a test seen red under a mutation, the fix. An added key also owes
   `## Neighbour matrix` (the tool against read, grep, edit, write, bash and the kernel) and
   `## Dogfood` (the inputs run and what came back).

The corpus sampler, census, scripted player and replay in the plan's §4 are its phase 2, and
the tier-2 journey in §7 is phase 3; neither is built, and there is no `dogfood` recipe yet.
Until they land, `## Dogfood` records the calls you ran by hand. Report only counts, kinds,
timings and the one line that shows each defect.
