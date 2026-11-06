---
issue: Closes #2038
growth: +169 the skill-uptake scan, CLI surfaces and regression coverage; weighed for deletion: invalid catalog reads and unemitted start-note/nudge counters were removed, and the existing scan, fetch records and stats output were reused
raise: crate cli +28, crate runtime +141, tests +162, comments +4
ledger+: cross-session stats, observed prices and plan-op counters | `yi stats [id]` and `yi stats --by skills` count served skill reads (`custom{fetch}` rows for `yi://skills/<name>` and the files beside it, or a filesystem `SKILL.md` read), `Relevant:` skill pointers and how many a read of that skill followed within 3 assistant turns (`cli_surfaces::stats_counts_a_followed_skill_pointer_in_one_session_and_the_fold`, T2; `rollup::a_read_on_the_windows_last_turn_follows_and_one_past_it_does_not`, T0)
---
`yi stats` now counts how the model uses skills. One session's view and the new `yi stats --by skills` fold report skill reads from durable `custom{fetch}` entries, with a filesystem `SKILL.md` fallback, the `Relevant:` pointers Yi placed, and how many of those a read of the same skill followed within 3 assistant turns. Before, the 0-of-62 baseline for followed pointers came from a hand scan of the session files, and nothing could show whether the coming start step changes it.
