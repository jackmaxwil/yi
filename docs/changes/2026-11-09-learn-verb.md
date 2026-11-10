---
version: 0.1021.0
issue: Refs #2129
raise: crate runtime +15, crate tui +4, comments +2, tests +43
ledger+: host-written memory (one file per fact, hook index) | `/learn [focus]`, a slash verb in the TUI, console and ACP `_yi/slash`, sends a prompt that reads `yi://skills/learn` and writes memory notes and at most three refine variants to `~/.yi/refine/queue.jsonl`, each quoting the session line behind it, never a prompt, rule or skill (`session_faux::learn_starts_a_turn_that_opens_its_skill_with_the_users_focus`)
---
`/learn [focus]` turns the session it runs in into records. The verb sends a prompt (in the TUI a steer while a turn runs; over ACP queued behind it) that reads the new `learn` skill and follows it: find the failures, re-reads and corrections in this session's own log, quote the line behind each finding, save what should outlive the session as memory notes, and append at most three refine variants to `~/.yi/refine/queue.jsonl`, which `just refine round` reads. An item whose line cannot be quoted is dropped and named; a full queue (64 rows) takes no more. The verb writes nothing itself, and the skill never edits a prompt, rule or skill: a variant lands only as a pull request the owner merges after a round judges it better.
