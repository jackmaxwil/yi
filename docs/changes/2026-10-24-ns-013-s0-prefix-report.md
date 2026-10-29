---
version: 0.736.0
decisions: D460
issue: Closes #1455, Refs #1442
raise: crate runtime +1, tests +244
decision: the request budget also ratchets each role's first-request prefix in a bare workspace | the gate priced a root prompt no child sends, so a child prefix could grow unseen | delete role_prefix.json and the ROLE_PREFIX reader in check_request_budget.py
---
This is the stage 0 report of Yi's next shape (S0-prefix-report). Every later byte claim in the stack restates its numbers from the rows below, and re-runs the same test to judge itself.

The request-budget harness now measures what each role really sends on its first request. `report_the_prefix_size` keeps its `REQUEST_PREFIX` line and adds `ROLE_PREFIX` rows. Each row comes from a root wired the way `yi` wires one: the root extensions are installed before the runtime attaches, and requests are proxied to a local stand-in that records every body. The root sends one turn, and four children are spawned through the root's host: a plain leaf root child, a leaf root child whose brief trips the orchestrate trigger, a reader and a worker. A row's bytes are the serialized `system` plus `tools` of that role's first Anthropic request body, cache marks included, which is how `REQUEST_PREFIX` counts. The bare workspace is a TB container's root: no instruction file and Yolo, with the unattended slot `yi ask` attaches to a root (children never get it). The lane workspace adds this repo's tracked AGENTS.md (44,234 B), committed, and a Cargo.toml, in Auto. The main workspace adds the checkout's untracked CLAUDE.md when the checkout has one. The scratch HOME holds the pinned ripwire plugin, so the root's plugin card is the same on every machine instead of following the runner's PATH. check_request_budget.py ratchets only the five bare rows, against the new scripts/guardrails/baselines/role_prefix.json. The lane and main rows read the repo's own files at test time, so they are reported and never gated.

Rows measured at this head (bytes, system + tools = total):

| role | bare (Yolo) | lane (Auto, AGENTS.md) | main |
| --- | --- | --- | --- |
| root | 32,560 + 17,673 = 50,233 | 80,430 + 17,673 = 98,103 | same as lane (claude=absent) |
| leaf root child | 30,380 + 15,181 = 45,561 | 78,272 + 15,181 = 93,453 | same as lane |
| leaf root child, orchestrate triggered | 41,871 + 15,181 = 57,052 | 89,763 + 15,181 = 104,944 | same as lane |
| reader | 802 + 3,869 = 4,671 | 4,671 | 4,671 |
| worker | 1,464 + 4,968 = 6,432 | 49,356 + 4,968 = 54,324 | same as lane |

The worktree this ran in has no CLAUDE.md, so its main rows print `claude=absent` and equal the lane rows. CI checks out no CLAUDE.md either. A CLAUDE.md regenerated from .ruler carries AGENTS.md's rules and is deduped by content, so the main rows would equal the lane rows in a checkout whose CLAUDE.md is current; only a stale one adds bytes, as the capture below shows.

Bet 1 (a), zero spend. Reader routing's byte claim survives: the bare reader's fixed prefix is 4,671 B, under the 6,000 B kill line (the strategy estimated 4,484 B plus its partition; the partition rides the first message, not this prefix). The orchestrate trigger costs a leaf root child 11,491 B of request bytes (57,052 - 45,561); the strategy's 11,222 B is the fragment's raw size, and JSON escaping adds the rest. The leaf root child's 45,561 B bare row (93,453 B in a lane) is the baseline the `can_delegate` unit's 3,800 B kill and orchestrate-for-leaf's -11,222 B claim are judged against, by re-running this test on their heads. A worker in a lane reads 54,324 B, almost all of it the AGENTS.md it registers.

Bet 4 (a), zero spend, a recount and not a kill. evals/prefix_capture.py cloned this repo into an empty scratch directory, copied in what a clone omits from the owner's main checkout (its untracked CLAUDE.md and its gitignored skill directories: 12 skills under .agents/skills, 16 under .pi/skills and 16 under .claude/skills, one catalog after dedupe by name), and ran the debug binary on `faux/faux-1` under a scratch HOME, once claiming a lane and once with `--here`. It ran twice: once with a CLAUDE.md carrying AGENTS.md's rules (a copy of AGENTS.md, standing in for one regenerated from .ruler, which writes the same rules to both files), and once with the owner's CLAUDE.md as it sits today (36,003 B, last written 2026-09-26, before .ruler gained its Context gathering section). The opening `ext_state` of each session read:

| slot or source | lane | --here, current CLAUDE.md | --here, stale CLAUDE.md |
| --- | --- | --- | --- |
| identity | 6,120 | 6,120 | 6,120 |
| doctrine | 22,132 | 22,132 | 22,132 |
| permission mode (Auto) | 1,484 | 1,484 | 1,484 |
| unattended | 790 | 790 | 790 |
| lang-rust pack | 1,672 | 1,672 | 1,672 |
| ripwire plugin card | 972 | 972 | 972 |
| AGENTS.md (granted) | 44,234 | 44,234 | 44,234 |
| CLAUDE.md (untrusted) | absent | deduped | 36,003 |
| memory (untrusted) | 1,195 | 1,195 | 1,195 |
| project skills (untrusted) | absent | 1,542 | 1,542 |
| total | 78,599 | 80,141 | 116,144 |

So for the instruction files a `--here` run equals a lane: a current CLAUDE.md collapses into AGENTS.md by content hash, and the lane reads the same tracked AGENTS.md. What `--here` adds is the 1,542 B project-skills catalog from the gitignored skill directories, which a lane never checks out. The 36,003 B the stale run shows is an artifact of a CLAUDE.md that ruler has not rewritten since 2026-09-26, not a cost of `--here`: it is the stale render the strategy's Bet 4 item 1 removes by re-running ruler, and with a copy of AGENTS.md standing in for the re-run, nothing of it is left. Later units that restate the lane against `--here` difference take the 1,542 B skills row; the 36,003 B is the saving of the ruler re-run alone. These are raw text bytes; the request rows above are JSON-encoded, which is why they run larger. The sources show as untrusted because the scratch HOME has granted nothing; in the owner's own HOME they may be granted, which changes their wrapper and not their bytes.

The plan counters' first reading, required by S0-plan-counters: `yi stats --by plan` over the stack's own scratch sessions only (the 13 worktree HOMEs under ~/Development/yi-ns, never the owner's ~/.yi/sessions) found 0 plan ops in 3 session files and 0 journals.

To share the wiring, `key`, `root_with`, `stand_in`, `body_asking` and the `git` helper moved from family_cache.rs and worker_prompt.rs into crates/runtime/tests/support/upstream.rs, which integration.rs and request_budget.rs both include. `root_with` gains two arguments: whether to install the root extensions, and the tool table. The one production line is in the plan-op tally this branch carries: a record stamped before `--since` is dropped before its dedupe key is built, so an old journal no longer pays a key sort and a set entry for every record outside the window.
