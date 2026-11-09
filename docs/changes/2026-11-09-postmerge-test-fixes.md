---
issue: Closes #2125
cut: crates/runtime/tests/ipython_faux.rs | (*timestamp, usage.input) = (1, 1_000); | the loop now overwrites a scripted reply time with the real clock, so the journey makes its lapse by resuming an aged session instead
raise: tests +35
---
Two journeys that failed on every push to main now pass (Closes #2125). The reclaim journey for an aged image scripted each reply with a 1 ms timestamp to make the cache look expired. Since the loop stamps every reply with the real clock, that expiry never happened and no result was cut; the journey now runs its script, ages the stored session by six minutes and resumes it, which is how a real lapse arrives. The `long-todo-label` surface scenario listed its three items as bullet lines, which the spec gate counts as three requirements, so the faux model's one-line stop was refused and its missing second reply ended the run with exit 1; the items are now plain lines, word for word the same. No product code changed.

The reclaim journey went red on main before the change with `no image reclaim placeholder`, and the scenario with `FAIL surface_dry: rollouts did not exit 0: ['long-todo-label']`. A third failure on main, the ACP evicted-letter journey, does not reproduce on this Mac or in a Linux root container and is left to its own issue.
