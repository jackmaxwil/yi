---
issue: Closes #1567
raise: crate console +2
raise: crate runtime +56
raise: crate tui +12
raise: crate types +10
raise: tests +337
decision: a child's token lease is enforced at the reply that crosses it, ending the run as `Failed { class: LeaseSpent }` rather than an interruption | the lease was drawn and settled but never held, so a child could spend a cap its parent reserved for it, and the ending had to be a class of its own because an interruption reads as the parent's choice | reversible in `ChildRecord::enforce` and `SubagentHost::lease_ending`, both in `crates/runtime/src/subagent/`
ledger~: kernel + ipython + rlm() subagents | ; o | ; a child's spend is the fresh fold of its replies, never its cached prefix, and its run ends at the reply that crosses the lease its parent drew (`lease_e2e::a_child_is_stopped_after_the_reply_that_crosses_its_lease`, T1), its context gauge its own last request over its own model's window (`tui_e2e::a_childs_gauge_is_its_last_request_over_its_own_window`, T1); o
---
A child's token lease was drawn from its parent and never held: `rlm.run(tokens=N)` reserved the number, the parent was billed for it, and nothing stopped the child at it — the brief even said so, "(reserved, not enforced)". Worse, the count it was billed for was `usage.total_tokens`, so a child re-sending one 15k cached prefix was charged 15k on every reply; a twelve-turn reader crossed a 1000-token lease it had spent 1200 of.

Both halves are one fact, a child's spend, so they are fixed at one seam. `ChildRecord::charge` folds `Usage::fresh` — input plus cache write plus output, the cache read excluded — into the count, and the record ends the run at the first reply whose fold leaves the lease spent: it aborts the session and `SubagentHost::lease_ending` names the ending `Failed { class: LeaseSpent }` (additive; an older host decodes it as `Other`), with a notice that quotes the cap. The same fold fills `ChildUpdate`'s new `context_tokens` and `context_window`, so a child's context gauge is its own last request over its own model's window instead of its cumulative spend over the root's; `/agents` draws that gauge, and blank where a host named neither.

The brief line now states the cap it enforces, and a child at exactly its lease runs one more reply: the check is `>`, not `>=`.
