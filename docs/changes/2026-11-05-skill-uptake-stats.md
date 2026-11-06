---
issue: Closes #2038
growth: +203 the skill-uptake scan, CLI surfaces and regression coverage; weighed for deletion: invalid catalog reads and unemitted start-note/nudge counters were removed, and the existing scan, fetch records and stats output were reused
raise: crate cli +27, crate runtime +176, tests +244, comments +3
ledger+: cross-session stats, observed prices and plan-op counters | `yi stats [id]` and `yi stats --by skills` count served skill reads from `custom{fetch}` rows, every `Relevant:` skill pointer including capped extras, and how many a read of that skill followed within 3 assistant turns; fork-seeded copies count once (`cli_surfaces::stats_counts_a_followed_skill_pointer_in_one_session_and_the_fold`, T2; `rollup::every_address_in_a_capped_reminder_counts_and_can_be_followed`, T0)
---
`yi stats` now counts how the model uses skills. One session's view and the new `yi stats --by skills` fold report skill reads only after durable `custom{fetch}` entries prove the resolver served them, every `Relevant:` pointer including the cap's extras, and how many of those a read of the same skill followed within 3 assistant turns. Fork-seeded reminder copies count once, and the review bot's footer uses the same served-read rule. Before, the 0-of-62 baseline for followed pointers came from a hand scan of the session files, and nothing could show whether the coming start step changes it.

## Seen red

- `rollup::every_address_in_a_capped_reminder_counts_and_can_be_followed`: limiting `skill_names` to one address produced `[1, 1, 0]` instead of `[1, 2, 1]`; the fixture is a synthetic reminder with one visible and one capped address, then a fetch of the capped skill.
- `rollup::a_filesystem_skill_read_counts_only_when_its_local_fetch_arrives`: changing the fetch rule from `local://` to `file://` produced `[0, 1, 0]` instead of `[1, 1, 1]`; the fixture is a synthetic pointer, attempted filesystem read and local `SKILL.md` fetch.
- `rollup::a_reminder_copied_into_a_fork_counts_once`: disabling the reminder dedupe guard produced `[0, 2, 0]` instead of `[0, 1, 0]`; the fixture writes one synthetic reminder into parent and child session files under the recorded root fixture's header.
- `rollup::tests::only_skill_fetches_are_retained_for_dedupe`: before irrelevant fetches were excluded, `local://src/lib.rs` was retained (`left: true`, `right: false`); the fixture is three synthetic fetch entries covering an ordinary file, a local skill and a catalog skill.
