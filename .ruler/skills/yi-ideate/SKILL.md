---
name: yi-ideate
description: Brainstorm, pressure-test, grill and write up a Yi design the way the owner wants it — few generic primitives, every feature a composition, decisions taken by structured question rounds, plain standard-CS prose
---

# Ideating and planning Yi

The owner set this standard in the 2026-09-23/24 session that produced the seven-primitives
proposal ("i love it when you use core powerful primitives with composition"). Every
brainstorm, ideation, pressure test and plan follows it.

## The architectural bar

1. Find the fewest generic primitives that make every feature a composition. Write the
   composition table (capability → composed from → ✓ exists / ✚ new). A feature that is not a
   composition must justify a new noun.
2. A new noun pays for itself by absorbing an existing mechanism. Generalize what exists before
   adding beside it: D230's ordered queue became the channel, not a bus next to it.
3. Scale-invariant. The same primitives run at every size, and the small case is the
   degenerate case, never a mode: one computer is a hive of one node.
4. Judge every idea with a verdict: KEEP, MERGE (an option on a primitive), RECIPE (a documented
   composition, no code), FREE (falls out of the others), EXISTS, DEFER, CUT. Cut your own first
   draft hardest; it is always the most overbuilt.
5. Adopt strong precedents by name and map them concept by concept: Temporal, Kafka,
   Erlang/OTP, Plan 9, Kubernetes controllers, systemd socket activation, seccomp. State the one
   deliberate difference and why.
6. Keep the standing laws: the session JSONL is the one ledger and every other store is a view;
   triggers and routing read data; recovery never runs saved source; caps are fuses; start
   permissive within the lease and add scopes when the log shows an overstep.
7. Name things in standard networking, OS, database, distributed-systems and data-science
   terms. An analogy is never a name (biology was rejected). A term the owner coined is never
   renamed: "channel" stays "channel".

## Grounding

- Read the branch before proposing: the types, functions and `file:line` a primitive builds on.
  Mark each piece ✓ or ✚.
- Cite the record: ledger run ids, D-rows, plans, memories. Recount every number against its
  source before stating it, and estimate from realistic values, never from a cap.
- Case studies come from the reference agents under ref/agents/ (a read-only subagent that
  respects the Appendix A excise blocks) and from your own limits as the assistant, observed in
  the session itself.
- In documents, quote the owner verbatim. A paraphrase of their decision is a restatement, and
  restatement is how intent gets lost.

## Session shape

1. Diverge: a wide brain dump — journeys, patterns, pain points, each pain point with evidence.
2. Converge: a pressure test and a ranking with explicit criteria; say what was cut and why.
3. Grill: question rounds (below) until the forks are decided. "One or two more rounds" means
   exactly that.
4. Write up: the full proposal (template below) in the session scratchpad — never in a PR
   worktree or the shared tree — sent with SendUserFile. Offer to commit or publish; do neither
   unasked.

Every reply leads with the answer, then the reasoning, and ends on a recommendation rather
than a survey.

## Grilling

- Rounds of three or four AskUserQuestion questions, each a real fork whose answer changes the
  design. A choice with a conventional default is stated, not asked.
- Open each question with its sharpest evidence: a ledger number, a quoted law, a contradiction
  between two earlier answers.
- Two to four options, each description carrying its trade-off; the recommended option first,
  marked "(Recommended)".
- Open the next round with one line recording the answers, then go after their weak points:
  the cost at the small end, what must exist on day one, what pays for it, who decides.
- When the owner says a question is framed wrong, reframe from first principles instead of
  defending the frame. When they say "overengineering", cut it and list it under not-building.

## Prose

- Plain, direct sentences. Tables for mappings, verdicts and decisions; code only where it shows
  a composition, with ✚ on proposed API.
- Name the failure and its evidence before the fix. Every write-up states its limits and open
  questions.
- Energy comes from composition and concrete walkthroughs (a landing page traced from the
  intent record to the judge's catch), never from adjectives. "Swarms of thousands" or
  "web4.0" without mechanics reads as vaporware.
- Chat replies follow caveman; documents are normal prose.

## The write-up

A status block (date, tree, what it supersedes, the ✓/✚ marks) · a summary with the primitives
table · the problem, with case studies · laws, in the owner's words · each primitive's shape,
verbs and status · the composition table · worked examples · a section per topic · an
exists/new inventory with file refs · what it deletes · the build order, each stage a demo and
a gate, stages that need no new infrastructure and hit the owner's worst pain first · the
decisions log, verbatim · open questions · the D-rows owed · not building.

## What the first session paid for

- The owner's "channel" renamed twice, to "signal" and then "topic".
- Biology nouns kept after "drop the biology".
- A first draft carrying a flow engine, four delivery modes and CloudEvents in the core, which a
  pressure test cut from fourteen ideas to three.
- A binary frame the owner rejected: "personal or scale?".
- Unverified numbers: a miscounted ledger range, an estimate built on an 8-hour cap.
- Scratch docs written into a PR worktree, and a checkout in the shared main tree.
